import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:path_provider/path_provider.dart';

import '../repository/book_repository.dart';
import '../repository/record_repository.dart';
import '../repository/tag_repository.dart';
import '../src/rust/api/cache.dart';
import '../src/rust/api/book.dart' as rust_book;
import '../src/rust/api/db.dart';
import 'folder_snapshot_store.dart';
import 'foreground_work_coordinator.dart';
import 'credential_vault.dart';
import 'library_catalog.dart';
import 'library_index_service.dart';
import 'models.dart';
import 'remote_listing.dart';
import 'remote_cache_cleanup.dart';
import 'remote_scan_coordinator.dart';
import 'random_read_selector.dart';

@visibleForTesting
List<String> staleCachePathsForTombstones(
  Iterable<String> tombstones,
  Set<String> livePaths,
) => tombstones.where((path) => !livePaths.contains(path)).toList();

@visibleForTesting
String remoteAssetLogId(String sourceId, String logicalPath) =>
    Object.hash(sourceId, logicalPath).toUnsigned(32).toRadixString(16);

@visibleForTesting
class PurgeStaleDataBindings {
  const PurgeStaleDataBindings({
    required this.loadVerifiedTombstones,
    required this.loadIndex,
    required this.purgeVerifiedAsset,
    required this.reloadCatalog,
  });

  final Future<List<VerifiedRemoteTombstoneDto>> Function(String sourceId)
  loadVerifiedTombstones;
  final Future<List<LibraryIndexDto>> Function(String sourceId) loadIndex;
  final Future<BigInt> Function(
    String sourceId,
    String logicalPath,
    List<String> dependencyPaths,
  )
  purgeVerifiedAsset;
  final Future<void> Function() reloadCatalog;
}

bool _pathWithinDeletedRoot(String path, Iterable<String> roots) {
  String normalize(String value) {
    final normalized = value
        .replaceAll('\\', '/')
        .replaceAll(RegExp(r'/+$'), '');
    return normalized.isEmpty ? '/' : normalized;
  }

  final normalized = normalize(path);
  return roots.any((root) {
    final candidate = normalize(root);
    return normalized == candidate ||
        (candidate == '/'
            ? normalized.startsWith('/')
            : normalized.startsWith('$candidate/'));
  });
}

enum BatchTagPhase { indexing, persisting, complete }

class BatchTagProgress {
  const BatchTagProgress({
    required this.phase,
    required this.processed,
    required this.total,
    required this.succeeded,
    required this.skipped,
    required this.failed,
  });

  final BatchTagPhase phase;
  final int processed;
  final int total;
  final int succeeded;
  final int skipped;
  final int failed;
}

class BatchTagResult {
  const BatchTagResult({
    required this.total,
    required this.succeeded,
    required this.skipped,
    required this.failed,
  });

  final int total;
  final int succeeded;
  final int skipped;
  final int failed;

  int get processed => succeeded + skipped + failed;
}

/// 应用数据存储 facade（ADR-016/018）。
///
/// LibraryStore 是 ChangeNotifier + 跨 Repository 协调层。
/// 纯数据 CRUD 已下沉到 BookRepository / RecordRepository / TagRepository。
/// UI 通过 LibraryStore 访问所有数据，不直接依赖具体 Repository 实现。
///
/// SQLite 数据层（ADR-013）：启动时优先从 database.db 加载，
/// 若 SQLite 为空则回退到 library.json。
/// 每次变更时写入 SQLite（同步），library.json 仅作备份。
class LibraryStore extends ChangeNotifier {
  LibraryStore._();
  static final LibraryStore instance = LibraryStore._();

  @visibleForTesting
  static bool isUsableRemoteRefreshRevision(String revision) =>
      revision != 'missing-fingerprint' &&
      revision != 'remote-refresh-not-allowed';

  // ---- 委托给 Repository（纯数据持有） ----

  BookRepository get _books => BookRepository.instance;
  RecordRepository get _records => RecordRepository.instance;
  TagRepository get tags => TagRepository.instance;

  // ---- 公开访问器（保持与旧 API 兼容） ----

  List<BookSource> get sources => _books.sources;
  Map<String, ReadRecord> get records => _records.records;
  Map<String, BookMeta> get metas => _books.metas;

  AppSettings settings = AppSettings();
  bool _loaded = false;
  int _revision = 0;
  final Set<VoidCallback> _committedMutationListeners = <VoidCallback>{};

  /// Monotonic in-memory projection revision. UI caches use this to refresh
  /// only when library data actually changed.
  int get revision => _revision;

  void addCommittedMutationListener(VoidCallback listener) {
    _committedMutationListeners.add(listener);
  }

  void removeCommittedMutationListener(VoidCallback listener) {
    _committedMutationListeners.remove(listener);
  }

  void _notifyCommittedMutation() {
    for (final listener in List<VoidCallback>.of(_committedMutationListeners)) {
      listener();
    }
  }

  void _notifyAfterPersistence(Future<void> persistence) {
    unawaited(
      persistence.then<void>((_) {
        if (_lastSaveError == null) _notifyCommittedMutation();
      }, onError: (Object _, StackTrace _) {}),
    );
  }

  @override
  void notifyListeners() {
    _revision++;
    super.notifyListeners();
  }

  // ---- 全量保存（防抖 + 生命周期 flush） ----

  static const Duration _saveDebounceDuration = Duration(milliseconds: 800);
  static const String _jsonReconcileDoneKey = 'json_reconcile_done';

  Timer? _saveTimer;
  Future<void> _saveQueue = Future<void>.value();
  bool _saveDirty = false;
  Completer<void>? _saveWaiter;
  Completer<void>? _batchTagGate;
  Future<void> _batchTagTail = Future<void>.value();
  int _activeLoads = 0;
  Completer<void>? _loadsIdle;
  Object? _lastSaveError;

  /// 最近一次持久化失败的原因（无失败为 null）。供 UI 观测。
  Object? get lastSaveError => _lastSaveError;

  /// 加载全部数据（SQLite 优先，空则回退 library.json）。
  /// `force=true`：同步成功后强制重载，避免 Rust 侧已变更而 Dart 内存态过期
  /// （ADR-028 §12.5；repository 的 loadFromSqlite 均先 clear，重载安全）。
  ///
  /// `persist=false` 用于同步/自动流程从 SQLite 重载内存态：数据库已经是
  /// 权威来源，不应在重载中再次排队一个全量保存。重载前会等待已有保存，
  /// 防止清空 repository Map 时保存协程仍在遍历它们。
  Future<void> load({bool force = false, bool persist = true}) async {
    while (true) {
      final batchTagGate = _batchTagGate;
      if (batchTagGate == null) break;
      await batchTagGate.future;
    }
    if (_activeLoads == 0) _loadsIdle = Completer<void>();
    _activeLoads++;
    try {
      await _loadData(force: force, persist: persist);
    } finally {
      _activeLoads--;
      if (_activeLoads == 0) {
        final loadsIdle = _loadsIdle;
        _loadsIdle = null;
        if (loadsIdle != null && !loadsIdle.isCompleted) loadsIdle.complete();
      }
    }
  }

  Future<void> _loadData({required bool force, required bool persist}) async {
    if (_loaded && !force) return;
    await _waitForPendingSave();
    _loaded = false;
    try {
      final migrated = await dataIsMigrated();
      if (migrated) {
        await _loadFromSqlite();
        await _reconcileFromJsonIfNeeded();
        // JSON reconciliation is intentionally additive for old installs;
        // run the canonical projection migration again afterward so legacy
        // English/namespaced generated tags cannot be reintroduced.
        await TagRepository.instance.normalizeGeneratedTags();
        await _migrateAliasKeys();
        _syncRemoteScanSettingsGate();
        _loaded = true;
        notifyListeners();
        if (persist) await saveToDisk();
        return;
      }
    } catch (e) {
      debugPrint('[LibraryStore] SQLite load failed, fallback to JSON: $e');
    }

    // Fallback：从 library.json 加载
    try {
      final f = await _file();
      if (await f.exists()) {
        await TagRepository.instance.load(f);
        // Keep the JSON fallback on the same generated-tag migration path as
        // SQLite installs so obsolete digital-version tags cannot survive a
        // legacy-library load.
        await TagRepository.instance.normalizeGeneratedTags(persist: false);
        final j = jsonDecode(await f.readAsString()) as Map<String, dynamic>;
        _books.loadFromJson(j);
        _records.loadFromJson(j);
        if (j['settings'] != null) {
          settings = AppSettings.fromJson(
            Map<String, dynamic>.from(j['settings']),
          );
        }
      }
    } catch (e) {
      debugPrint('[LibraryStore] load JSON failed: $e');
    }
    _syncRemoteScanSettingsGate();
    _loaded = true;
    await _migrateAliasKeys();
    notifyListeners();
    if (persist) await saveToDisk();
  }

  /// Wait for all queued persistence work before replacing in-memory state.
  /// A load is allowed to clear repository maps, but never while a save is
  /// between two `await`s and iterating those same maps.
  Future<void> _waitForPendingSave() async {
    final timer = _saveTimer;
    if (timer != null) {
      timer.cancel();
      _saveTimer = null;
    }
    if (_saveDirty) _kickSave();
    await _saveQueue;
  }

  /// 从 SQLite 加载全量数据。
  Future<void> _loadFromSqlite() async {
    await _books.loadFromSqlite();
    await _records.loadFromSqlite();
    await TagRepository.instance.loadFromSqlite(force: true);
    await TagRepository.instance.syncMetadataLinks(_books.metas.values);
    await TagRepository.instance.normalizeGeneratedTags();

    final settingDtos = await dbLoadAllSettings();
    final settingsMap = <String, dynamic>{};
    for (final dto in settingDtos) {
      settingsMap[dto.key] = _tryParseJson(dto.value);
    }
    if (settingsMap.isNotEmpty) {
      settings = AppSettings.fromJson(settingsMap);
    }
    _syncRemoteScanSettingsGate();
  }

  /// 后缀别名归并（zip↔cbz 等视为同一本）：对阅读记录/元数据/标签关联做
  /// key 归一化迁移。幂等，重复执行无副作用；旧 key 行仍留在 SQLite 中，
  /// 每次启动会被再次合并（数据层同步任务将统一清理）。
  Future<void> _migrateAliasKeys() async {
    final recRepo = _records;
    final bookRepo = _books;
    final recordAliases = <String, String>{};
    final metaAliases = <String, String>{};

    // 1) 阅读记录：合并到规范化 key，保留最新进度与累计次数。
    final mergedRecords = <String, ReadRecord>{};
    for (final r in recRepo.records.values.toList()) {
      final localPath = normalizeLocalFilesystemPath(
        r.path,
        isLocalFs: r.sourceType == 'local',
        isWindows: Platform.isWindows,
      );
      final nk = bookKeyOf(r.sourceType, r.sourceId, localPath);
      final existing = mergedRecords[nk];
      if (nk == r.key) {
        if (existing == null) {
          mergedRecords[r.key] = r;
        } else {
          _mergeRecord(existing, r);
        }
      } else {
        if (existing == null) {
          mergedRecords[nk] = ReadRecord(
            key: nk,
            sourceId: r.sourceId,
            sourceType: r.sourceType,
            path: localPath,
            title: r.title,
            lastPage: r.lastPage,
            readCount: r.readCount,
            lastReadAt: r.lastReadAt,
          );
        } else {
          _mergeRecord(existing, r);
        }
        TagRepository.instance.remapBookKey(r.key, nk);
        recordAliases[r.key] = nk;
      }
    }
    recRepo.records
      ..clear()
      ..addAll(mergedRecords);

    // 2) 元数据：合并到规范化 key。
    final mergedMetas = <String, BookMeta>{};
    for (final m in bookRepo.metas.values.toList()) {
      final sep1 = m.key.indexOf('|');
      final sep2 = sep1 >= 0 ? m.key.indexOf('|', sep1 + 1) : -1;
      if (sep1 < 0 || sep2 < 0) {
        mergedMetas[m.key] = m;
        continue;
      }
      final st = m.key.substring(0, sep1);
      final sid = m.key.substring(sep1 + 1, sep2);
      final p = m.key.substring(sep2 + 1);
      final localPath = normalizeLocalFilesystemPath(
        p,
        isLocalFs: st == 'local',
        isWindows: Platform.isWindows,
      );
      final nk = bookKeyOf(st, sid, localPath);
      final existing = mergedMetas[nk];
      if (nk == m.key) {
        if (existing == null) {
          mergedMetas[m.key] = m;
        } else {
          _mergeMeta(existing, m);
        }
      } else {
        if (existing == null) {
          mergedMetas[nk] = _copyMetaWithKey(m, nk);
        } else {
          // A non-canonical legacy key is commonly the user's manually named
          // row; let its non-empty fields win over a newer auto-projection.
          _mergeMetaPreferSource(existing, m);
        }
        TagRepository.instance.remapBookKey(m.key, nk);
        metaAliases[m.key] = nk;
      }
    }
    bookRepo.metas
      ..clear()
      ..addAll(mergedMetas);

    // Persist the in-memory key migration immediately. Otherwise an
    // automation run with persist=false would reload the old row on the next
    // pass and show the same work twice again.
    for (final entry in recordAliases.entries) {
      final target = recRepo.records[entry.value];
      if (target != null) {
        await _records.saveOneToSqlite(target);
      }
      await dbDeleteRecord(key: entry.key);
    }
    for (final entry in metaAliases.entries) {
      final target = bookRepo.metas[entry.value];
      if (target != null) {
        await dbUpsertMeta(meta: _metaDto(target));
      }
      await dbDeleteMeta(key: entry.key);
    }
    if (recordAliases.isNotEmpty || metaAliases.isNotEmpty) {
      // TagRepository already normalizes the links in memory; this diff moves
      // the persisted links from legacy path keys to the canonical key.
      await TagRepository.instance.saveToSqlite();
    }
  }

  static void _mergeRecord(ReadRecord target, ReadRecord other) {
    target.readCount += other.readCount;
    if (other.lastPage > target.lastPage) target.lastPage = other.lastPage;
    if (other.lastReadAt > target.lastReadAt) {
      target.lastReadAt = other.lastReadAt;
    }
  }

  static BookMeta _copyMetaWithKey(BookMeta m, String key) => BookMeta(
    key: key,
    coverPage: m.coverPage,
    cropX: m.cropX,
    cropY: m.cropY,
    cropW: m.cropW,
    cropH: m.cropH,
    tags: [...m.tags],
    rotations: {...m.rotations},
    author: m.author,
    genre: m.genre,
    series: m.series,
    volume: m.volume,
    chapter: m.chapter,
    summary: m.summary,
    comment: m.comment,
    title: m.title,
    chineseTitle: m.chineseTitle,
  );

  static void _mergeMeta(BookMeta target, BookMeta other) {
    for (final t in other.tags) {
      if (!target.tags.contains(t)) target.tags.add(t);
    }
    for (final e in other.rotations.entries) {
      target.rotations.putIfAbsent(e.key, () => e.value);
    }
    if (target.author.isEmpty) target.author = other.author;
    if (target.genre.isEmpty) target.genre = other.genre;
    if (target.series.isEmpty) target.series = other.series;
    if (target.volume.isEmpty) target.volume = other.volume;
    if (target.chapter.isEmpty) target.chapter = other.chapter;
    if (target.summary.isEmpty) target.summary = other.summary;
    if (target.comment.isEmpty) target.comment = other.comment;
    if (target.title.isEmpty) target.title = other.title;
    if (target.chineseTitle.isEmpty) target.chineseTitle = other.chineseTitle;
    if (!target.hasCrop && other.hasCrop) {
      target.cropX = other.cropX;
      target.cropY = other.cropY;
      target.cropW = other.cropW;
      target.cropH = other.cropH;
    }
    if (target.coverPage == 0 && other.coverPage != 0) {
      target.coverPage = other.coverPage;
    }
  }

  /// 将 JSON 字符串解析为原始值（bool/int/double/String/List/Map）。
  static Object? _tryParseJson(String raw) {
    if (raw == 'true') return true;
    if (raw == 'false') return false;
    final i = int.tryParse(raw);
    if (i != null) return i;
    final d = double.tryParse(raw);
    if (d != null) return d;
    if ((raw.startsWith('{') || raw.startsWith('[')) &&
        (raw.endsWith('}') || raw.endsWith(']'))) {
      try {
        return jsonDecode(raw);
      } catch (_) {}
    }
    return raw;
  }

  /// 返回 library.json 的完整路径（供迁移等使用）。
  Future<String> filePath() async {
    final dir = await getApplicationSupportDirectory();
    return '${dir.path}${Platform.pathSeparator}library.json';
  }

  Future<File> _file() async => File(await filePath());

  /// 公开的全量保存入口（防抖合并）：同一次操作风暴只落盘一次。
  ///
  /// 返回的 Future 在该轮保存真正完成后完成。错误不会抛出，
  /// 而是记录到 [lastSaveError]，调用方 await 后自行检查。
  Future<void> saveToDisk() async {
    final batchTagGate = _batchTagGate;
    if (batchTagGate != null) await batchTagGate.future;
    _saveDirty = true;
    final waiter = _saveWaiter ??= Completer<void>();
    _saveTimer ??= Timer(_saveDebounceDuration, _kickSave);
    await waiter.future;
  }

  /// 生命周期退出前强制落盘：等待所有排队与执行中的保存完成。
  Future<void> flushPendingSave() async {
    final batchTagGate = _batchTagGate;
    if (batchTagGate != null) await batchTagGate.future;
    if (_saveTimer != null) {
      _saveTimer!.cancel();
      _saveTimer = null;
    }
    if (_saveDirty) _kickSave();
    await _saveQueue;
    final waiter = _saveWaiter;
    _saveWaiter = null;
    if (waiter != null && !waiter.isCompleted) {
      waiter.complete();
    }
    _saveDirty = false;
  }

  void _kickSave() {
    _saveTimer = null;
    _saveQueue = _saveQueue.then((_) => _drainSaves());
  }

  /// 执行所有排队中的保存；失败记录到 [lastSaveError] 并完成等待者。
  Future<void> _drainSaves() async {
    while (_saveDirty) {
      _saveDirty = false;
      try {
        await _save();
        _lastSaveError = null;
      } catch (e, st) {
        _lastSaveError = e;
        debugPrint('[LibraryStore] save failed: $e');
        debugPrintStack(stackTrace: st);
      }
    }
    final waiter = _saveWaiter;
    _saveWaiter = null;
    if (waiter == null || waiter.isCompleted) return;
    waiter.complete();
  }

  Future<void> _save() async {
    Object? sqliteError;
    try {
      await _saveToSqlite();
    } catch (e, st) {
      sqliteError = e;
      debugPrint('[LibraryStore] SQLite save failed: $e');
      debugPrintStack(stackTrace: st);
    }
    // JSON 仅作导出备份（best-effort）：失败不影响 SQLite 结果，不抛给调用方。
    try {
      await _writeJsonBackupOnly();
    } catch (e, st) {
      debugPrint('[LibraryStore] JSON export failed: $e');
      debugPrintStack(stackTrace: st);
    }
    if (sqliteError != null) {
      throw sqliteError;
    }
  }

  Future<void> _writeJsonBackupOnly() async {
    final f = await _file();
    final data = {
      ..._books.toJson(
        includeSensitive: includeSensitiveCredentialsInBackup(isWeb: kIsWeb),
      ),
      ..._records.toJson(),
      'settings': settings.toJson(),
      ...TagRepository.instance.toJson(),
    };
    await f.writeAsString(jsonEncode(data));
  }

  Future<void> _saveToSqlite() async {
    await _books.saveToSqlite();
    await _records.saveToSqlite();
    await TagRepository.instance.saveToSqlite();
    final settingsJson = settings.toJson();
    for (final entry in settingsJson.entries) {
      final v = entry.value;
      // Map/List 必须存合法 JSON，否则加载时 _tryParseJson 解析失败
      // 会把整段字符串塞进 settingsMap，导致 AppSettings.fromJson 强转崩溃。
      await dbSaveSetting(
        key: entry.key,
        value: v is Map || v is List ? jsonEncode(v) : v.toString(),
      );
    }
  }

  // ---- 一次性对账：JSON → SQLite（O3-A） ----

  /// SQLite 已迁移后首次启动读一次 library.json，把 SQLite 缺失的
  /// 标签/关联/阅读记录补上并写回；完成后置标记，JSON 不再被自动读取。
  Future<void> _reconcileFromJsonIfNeeded() async {
    try {
      final done = (await dbLoadAllSettings()).any(
        (d) => d.key == _jsonReconcileDoneKey,
      );
      if (done) return;

      final f = await _file();
      if (await f.exists()) {
        final j = jsonDecode(await f.readAsString()) as Map<String, dynamic>;
        // 合并 JSON 中 SQLite 缺失的标签/关联（幂等，含旧 hash ID 归一化）。
        await TagRepository.instance.load(f, force: true);
        await TagRepository.instance.saveToSqlite();

        // 补缺失的阅读记录。
        final recJ = (j['records'] as Map?) ?? {};
        for (final entry in recJ.entries) {
          final key = entry.key;
          if (!_records.records.containsKey(key)) {
            _records.records[key] = ReadRecord.fromJson(
              Map<String, dynamic>.from(entry.value),
            );
            await _records.saveOneToSqlite(_records.records[key]!);
          }
        }
        notifyListeners();
      }
      await dbSaveSetting(key: _jsonReconcileDoneKey, value: 'true');
    } catch (e, st) {
      // 对账失败不置标记，下次启动重试。
      debugPrint('[LibraryStore] JSON reconcile failed: $e');
      debugPrintStack(stackTrace: st);
    }
  }

  // ---- Source（委托给 BookRepository） ----

  Future<void> _persistSourceMutation() async {
    await saveToDisk();
    await LibraryCatalogStore.instance.loadTree();
  }

  Future<void> addSource(BookSource s) =>
      ForegroundWorkCoordinator.instance.runForeground(() => _addSource(s));

  Future<void> _addSource(BookSource s) async {
    _books.addSource(s);
    notifyListeners();
    final total = Stopwatch()..start();
    var persisted = false;
    try {
      var sqliteReady = false;
      final migrationCheck = Stopwatch()..start();
      Object? migrationCheckError;
      try {
        sqliteReady = await dataIsMigrated();
      } catch (error) {
        migrationCheckError = error;
        // Keep the existing full-save path available during database recovery.
      }
      if (kDebugMode || kProfileMode) {
        debugPrint(
          'source_add stage=migration_check type=${s.type} migrated=$sqliteReady${migrationCheckError == null ? '' : ' error_type=${migrationCheckError.runtimeType}'} elapsed_ms=${migrationCheck.elapsedMilliseconds}',
        );
      }
      if (!sqliteReady) {
        final fallbackSave = Stopwatch()..start();
        await _persistSourceMutation();
        persisted = true;
        _notifyCommittedMutation();
        if (kDebugMode || kProfileMode) {
          debugPrint(
            'source_add stage=fallback_save type=${s.type} elapsed_ms=${fallbackSave.elapsedMilliseconds}',
          );
        }
        return;
      }

      final sourceWrite = Stopwatch()..start();
      try {
        await _books.saveSourceToSqlite(s);
      } catch (_) {
        _books.removeSource(s.id);
        notifyListeners();
        rethrow;
      }
      if (kDebugMode || kProfileMode) {
        debugPrint(
          'source_add stage=source_upsert type=${s.type} elapsed_ms=${sourceWrite.elapsedMilliseconds}',
        );
      }

      final treeRefresh = Stopwatch()..start();
      await LibraryCatalogStore.instance.loadTree();
      if (kDebugMode || kProfileMode) {
        debugPrint(
          'source_add stage=catalog_refresh type=${s.type} loaded=${LibraryCatalogStore.instance.loaded} elapsed_ms=${treeRefresh.elapsedMilliseconds}',
        );
      }
      persisted = true;
      _notifyCommittedMutation();
      // SQLite is authoritative after migration. Keep the JSON export current,
      // but do not make source creation wait for a full-library serialization.
      unawaited(saveToDisk());
    } finally {
      if (kDebugMode || kProfileMode) {
        debugPrint(
          'source_add stage=complete type=${s.type} persisted=$persisted elapsed_ms=${total.elapsedMilliseconds}',
        );
      }
    }
  }

  Future<void> removeSource(String id) async {
    _books.removeSource(id);
    notifyListeners();
    await _persistSourceMutation();
    if (_lastSaveError == null) _notifyCommittedMutation();
  }

  Future<void> updateSource(
    String id, {
    String? name,
    String? url,
    String? username,
    String? password,
    int? port,
    String? path,
    String? refreshToken,
    String? clientId,
    String? clientSecret,
    String? rootId,
    String? cookie,
    String? credentialRef,
    String? hostKeyFingerprint,
    String? hostKeyEndpoint,
    String? sftpAuthMethod,
    String? privateKey,
    String? privateKeyPassphrase,
    String? note,
  }) async {
    final current = sourceById(id);
    final isRootIdSource = current?.is115 == true || current?.isQuark == true;
    final rootCandidate = rootId ?? path ?? current?.effectiveRootPath ?? '0';
    final normalizedRoot = isRootIdSource
        ? (rootCandidate.trim().isEmpty ? '0' : rootCandidate.trim())
        : rootId;
    final normalizedPath = isRootIdSource ? normalizedRoot : path;
    _books.updateSource(
      id,
      name: name,
      url: url,
      username: username,
      password: password,
      port: port,
      path: normalizedPath,
      refreshToken: refreshToken,
      clientId: clientId,
      clientSecret: clientSecret,
      rootId: normalizedRoot,
      cookie: cookie,
      credentialRef: credentialRef,
      hostKeyFingerprint: hostKeyFingerprint,
      hostKeyEndpoint: hostKeyEndpoint,
      sftpAuthMethod: sftpAuthMethod,
      privateKey: privateKey,
      privateKeyPassphrase: privateKeyPassphrase,
      note: note,
    );
    // Phase 6.1 采纳语义：用户编辑保存即视为"本机配置该书源"——
    // 同步进来的远端书源（remote_only）编辑后转为逻辑本地源（归入本机区）。
    final src = sourceById(id);
    if (src != null && src.remoteOnly) {
      src.remoteOnly = false;
      src.originDeviceId = null;
    }
    notifyListeners();
    await _persistSourceMutation();
    if (_lastSaveError == null) _notifyCommittedMutation();
  }

  void updateSourceCapability(String id, String label) {
    _books.updateSourceCapability(id, label);
    notifyListeners();
    _notifyAfterPersistence(saveToDisk());
  }

  BookSource? sourceById(String id) => _books.sourceById(id);

  Future<void> removeSourceWithCleanup(String id) async {
    final src = sourceById(id);
    // 删除前抓取该源全部阅读记录（用于删除后清理磁盘缓存）
    final affected = _records.records.values
        .where((r) => r.sourceId == id)
        .toList();
    final cachedPaths = <String>{...affected.map((r) => r.path)};
    if (src != null) {
      try {
        final indexed = await dbLoadLibraryIndexForSource(sourceId: id);
        cachedPaths.addAll(indexed.map((e) => e.path));
      } catch (_) {}
    }
    _books.removeSource(id);
    if (src != null) {
      final prefix = '${src.type}|${src.id}|';
      _records.removeByPrefix(prefix);
      _books.metas.removeWhere((k, _) => k.startsWith(prefix));
      await TagRepository.instance.removeBookTagsByPrefixAndPrune(prefix);
      // SQLite 同步删除：saveToSqlite 只 upsert 不删行，漏删会让书源/记录重启后复活。
      await dbDeleteSource(id: id);
      try {
        await _books.deleteSourceCredentials(src);
      } catch (error) {
        debugPrint(
          '[LibraryStore] secure credential cleanup failed: ${error.runtimeType}',
        );
      }
      await dbDeleteRecordsBySourcePrefix(prefix: prefix);
      await dbDeleteMetasBySourcePrefix(prefix: prefix);
    }
    // 磁盘缓存清理：源身份字段删除前已捕获，逐本删除 page/raw/cover 缓存。
    for (final path in cachedPaths) {
      try {
        await purgeStaleBookCache(
          sourceType: src?.type ?? 'local',
          path: path,
          url: src?.url,
          port: src?.port,
          rootPath: src?.effectiveRootPath ?? '',
          clientId: src?.clientId,
          rootId: src?.rootId,
          cookieMode: (src?.cookie ?? '').isNotEmpty,
        );
      } catch (e) {
        debugPrint(
          '[LibraryStore] removeSource cache cleanup failed for $path: $e',
        );
      }
    }
    notifyListeners();
    await saveToDisk();
    await LibraryCatalogStore.instance.loadTree();
    _notifyCommittedMutation();
  }

  // ---- Record（委托给 RecordRepository） ----

  Future<void> recordRead({
    required BookSource source,
    required String path,
    required String title,
    int? page,
  }) async {
    final localPath = normalizeLocalFilesystemPath(
      path,
      isLocalFs: source.isLocalFs,
      isWindows: Platform.isWindows,
    );
    final key = RecordRepository.keyOf(source.type, source.id, localPath);
    final r = _records.upsert(
      source: source,
      path: localPath,
      title: title,
      page: page,
    );
    TagRepository.instance.link(key, '已读');
    // ADR-029 触及即补：已读的书自动入离线索引（本地 upsert，零网络）
    LibraryIndexService.ensureIndexed(source, localPath, name: title);
    notifyListeners();
    try {
      // 轻量落盘（B 方案）：只写记录 + 标签关联，不写全量 JSON。
      await _records.saveOneToSqlite(r);
      await TagRepository.instance.persistBookLinks(key);
      _notifyCommittedMutation();
    } catch (e, st) {
      _lastSaveError = e;
      debugPrint('[LibraryStore] recordRead save failed: $e');
      debugPrintStack(stackTrace: st);
    }
  }

  ReadRecord? recordOf(BookSource source, String path) =>
      _records.of(source, path);

  bool hasAnyRead() => _records.hasAnyRead();

  /// 清空最近阅读 / 最多阅读记录（含每本书的阅读进度）。
  /// 用户「清空全部缓存」时一并执行：内存 + SQLite 逐条删（与正常删除同路径，
  /// 保留同步墓碑语义）；不影响书架元数据、标签、书源。
  Future<void> clearReadRecords() async {
    final keys = _records.records.keys.toList();
    _records.clearAll();
    for (final k in keys) {
      await dbDeleteRecord(key: k);
    }
    notifyListeners();
    await saveToDisk();
    _notifyCommittedMutation();
  }

  /// 清空阅读统计：所有记录阅读次数归零（保留最近阅读列表与每本书的进度）。
  /// 用户「清空全部缓存 → 仅清空阅读统计」时调用。
  Future<void> resetReadCounts() async {
    for (final r in _records.records.values) {
      r.readCount = 0;
    }
    await dbResetReadCounts();
    notifyListeners();
    await saveToDisk();
    _notifyCommittedMutation();
  }

  List<ReadRecord> get recent => _records.recent();
  List<ReadRecord> get mostRead => _records.mostRead();

  /// 清理失效漫画数据：源已删除的记录/元数据、本地文件丢失或远程已删除
  /// （在线索引对齐 + 离线索引墓碑）的记录，以及这些 key 上的标签关联、
  /// AI 任务与磁盘缓存（page/raw/cover）。
  /// 返回 (清理的记录数, 清理的元数据数, 释放的缓存字节数, 在线核对失败的远程源数)。
  Future<(int, int, int, int)> purgeStaleData({
    bool alignRemote = true,
    bool background = false,
    @visibleForTesting PurgeStaleDataBindings? bindings,
  }) async {
    Future<void> maintenanceBoundary() async {
      if (background) {
        await ForegroundWorkCoordinator.instance.waitForMaintenanceTurn();
      }
    }

    final loadVerifiedTombstones =
        bindings?.loadVerifiedTombstones ??
        (String sourceId) => dbLoadVerifiedRemoteTombstones(sourceId: sourceId);
    final loadIndex =
        bindings?.loadIndex ??
        (String sourceId) => dbLoadLibraryIndexForSource(sourceId: sourceId);
    final purgeVerifiedAsset =
        bindings?.purgeVerifiedAsset ??
        (String sourceId, String logicalPath, List<String> dependencies) =>
            purgeVerifiedRemoteAsset(
              sourceId: sourceId,
              logicalPath: logicalPath,
              dependencyPaths: dependencies,
            );
    final reloadCatalog =
        bindings?.reloadCatalog ?? LibraryCatalogStore.instance.loadTree;
    final sourceIds = sources.map((s) => s.id).toSet();

    // Phase 0：在线索引对齐（仅远程源）。只有 RemoteScanEngine 成功发布的
    // 完整 generation 才能产生下方可消费的墓碑；失败/离线不会推进证明。
    var alignFailed = 0;
    if (alignRemote) {
      for (final s in sources) {
        await maintenanceBoundary();
        if (s.isLocalFs || s.isGhost) continue;
        final ok = await _alignRemoteIndex(s);
        if (!ok) alignFailed++;
      }
    }

    // 远程只消费当前成功 generation + 完整父目录 listing 证明的墓碑；
    // 本地仍使用普通离线索引墓碑。目录墓碑安全扩展到其已索引后代。
    final tombstones = <String, Set<String>>{};
    final verifiedRemoteCandidates = <String, List<VerifiedRemoteTombstone>>{};
    final verifiedRemoteTombstones = <String, List<VerifiedRemoteTombstone>>{};
    for (final s in sources) {
      await maintenanceBoundary();
      try {
        if (s.needsSession) {
          final verified = await loadVerifiedTombstones(s.id);
          if (verified.isEmpty) continue;
          final typed = verified
              .map(
                (row) => VerifiedRemoteTombstone(
                  logicalPath: row.logicalPath,
                  dependencyPaths: row.dependencyPaths,
                ),
              )
              .toList(growable: false);
          verifiedRemoteCandidates[s.id] = typed;
        } else {
          final gone = await dbLoadLibraryIndexTombstones(sourceId: s.id);
          if (gone.isNotEmpty) tombstones[s.id] = gone.toSet();
        }
      } catch (_) {
        /* 索引不可读则无墓碑证据，保守不清 */
      }
    }

    var verifiedRemoteFreed = BigInt.zero;
    for (final s in sources.where((source) => source.needsSession)) {
      await maintenanceBoundary();
      final candidates = verifiedRemoteCandidates[s.id];
      if (candidates == null || candidates.isEmpty) continue;
      try {
        final result = await purgeVerifiedRemoteTombstonesSafely(
          sourceId: s.id,
          tombstones: candidates,
          cleanup: purgeVerifiedAsset,
          onError: (tombstone, error) => debugPrint(
            '[LibraryStore] verified remote cache cleanup rejected ${s.id} '
            'asset=${remoteAssetLogId(s.id, tombstone.logicalPath)}: $error',
          ),
        );
        verifiedRemoteFreed += result.freedBytes;
        if (result.verifiedTombstones.isEmpty) continue;
        final roots = result.verifiedTombstones
            .map((row) => row.logicalPath)
            .toList(growable: false);
        final gone = roots.toSet();
        for (final row in await loadIndex(s.id)) {
          if (_pathWithinDeletedRoot(row.path, roots)) gone.add(row.path);
        }
        verifiedRemoteTombstones[s.id] = result.verifiedTombstones;
        tombstones[s.id] = gone;
      } catch (e) {
        debugPrint(
          '[LibraryStore] verified remote cache cleanup failed ${s.id}: $e',
        );
      }
    }

    // 只有当同一逻辑作品没有任何 live archive alias 时，才将墓碑提升
    // 为作品级删除；删除 .zip 而仍保留同名 .cbz 不应清掉元数据/标签。
    final deletedCatalogKeys = <String>{};
    final logicalTombstones = <String, Set<String>>{
      for (final s in sources.where((source) => !source.needsSession))
        if (tombstones[s.id]?.isNotEmpty ?? false) s.id: tombstones[s.id]!,
    };
    for (final s in sources) {
      await maintenanceBoundary();
      final gone = tombstones[s.id];
      if (gone == null || gone.isEmpty) continue;
      try {
        final deletionRoots =
            verifiedRemoteTombstones[s.id]
                ?.map((row) => row.logicalPath)
                .toList() ??
            const <String>[];
        final liveKeys = (await loadIndex(s.id))
            .where(
              (e) =>
                  !e.deleted && !_pathWithinDeletedRoot(e.path, deletionRoots),
            )
            .map((e) => bookKeyOf(s.type, s.id, e.path))
            .toSet();
        for (final path in gone) {
          final key = bookKeyOf(s.type, s.id, path);
          if (!liveKeys.contains(key)) {
            deletedCatalogKeys.add(key);
            (logicalTombstones[s.id] ??= <String>{}).add(path);
          }
        }
      } catch (_) {}
    }

    // 1) 失效阅读记录（源已删除 / 本地文件丢失 / 远程墓碑）
    final staleRecords = _records.purgeStale(
      sources,
      remoteTombstones: logicalTombstones,
    );

    // 2) 失效元数据（来源已删除）
    final staleMetas = <String>[];
    for (final m in _books.metas.values) {
      final parts = m.key.split('|');
      final sid = parts.length > 1 ? parts[1] : '';
      if (!sourceIds.contains(sid)) staleMetas.add(m.key);
    }

    // 2b) 失效记录对应的元数据（远程墓碑 / 本地文件缺失等：meta key 与记录 key 同构，
    // 一一对应）。仅清"源已删"的 meta 不够——本地删了文件、远程删了漫画时源仍在，
    // 必须随失效记录逐条删 meta，否则书架/标签/封面残留。
    final recordMetaKeys = staleRecords
        .where((r) => _books.metas.containsKey(r.key))
        .map((r) => r.key)
        .toList();
    final allMetaKeys = <String>{
      ...staleMetas,
      ...recordMetaKeys,
      ...deletedCatalogKeys,
    };

    final removedKeys = <String>{
      ...staleRecords.map((r) => r.key),
      ...staleMetas,
      ...deletedCatalogKeys,
    };
    if (removedKeys.isEmpty) {
      if (alignRemote || tombstones.isNotEmpty) {
        await reloadCatalog();
      }
      return (0, 0, verifiedRemoteFreed.toInt(), alignFailed);
    }

    // 内存清理：元数据 + 失效 key 上的标签关联
    for (final k in allMetaKeys) {
      _books.metas.remove(k);
    }
    var cleanupItems = 0;
    for (final k in removedKeys) {
      await TagRepository.instance.removeBookTagsAndPrune(k);
      cleanupItems++;
      if (cleanupItems % 50 == 0) await maintenanceBoundary();
    }

    // SQLite 清理：记录逐条删；失效记录对应的元数据逐条删；
    // 源已删的元数据按来源前缀批量删（连未进内存的残留行一起清）。
    // 这些调用必须等待完成，否则自动流程紧接着 reload 时可能读到旧行，
    // 让刚删除的阅读记录/元数据在本次运行或重启后短暂复活。
    cleanupItems = 0;
    for (final r in staleRecords) {
      await dbDeleteRecord(key: r.key);
      await dbDeleteMeta(key: r.key);
      cleanupItems++;
      if (cleanupItems % 50 == 0) await maintenanceBoundary();
    }
    final deletedRecordKeys = staleRecords.map((r) => r.key).toSet();
    for (final key in removedKeys.difference(deletedRecordKeys)) {
      // Covers catalog tombstones and metadata-only/tag-only assets that had
      // no in-memory ReadRecord. The DB helpers are idempotent and also leave
      // the normal sync tombstone behind.
      await dbDeleteRecord(key: key);
      await dbDeleteMeta(key: key);
      cleanupItems++;
      if (cleanupItems % 50 == 0) await maintenanceBoundary();
    }
    final prefixes = <String>{};
    for (final k in staleMetas) {
      final parts = k.split('|');
      if (parts.length >= 2) {
        prefixes.add('${parts[0]}|${parts[1]}|');
      }
    }
    for (final p in prefixes) {
      await dbDeleteMetasBySourcePrefix(prefix: p);
    }

    // 磁盘缓存清理：阅读记录之外，纯标签/元数据漫画也可能已经产生
    // cover/raw 缓存，因此从目录墓碑补充物理路径，确保最后一个资产删除
    // 后不会留下可见的漫画缓存。
    final cacheTargets = <(BookSource, String)>[];
    final cacheTargetKeys = <String>{};
    void addCacheTarget(BookSource source, String path) {
      final dedupe = '${source.id}\u0000$path';
      if (cacheTargetKeys.add(dedupe)) cacheTargets.add((source, path));
    }

    for (final r in staleRecords) {
      final src = sourceById(r.sourceId);
      if (src != null && !src.needsSession) addCacheTarget(src, r.path);
    }
    var freed = verifiedRemoteFreed;
    cleanupItems = 0;
    for (final target in cacheTargets) {
      await maintenanceBoundary();
      final src = target.$1;
      if (src.isGhost) continue; // 幽灵书源无可读缓存；源已删由 removeSource 路径处理
      try {
        freed += await purgeStaleBookCache(
          sourceType: src.type,
          path: target.$2,
          url: src.url,
          port: src.port,
          rootPath: src.effectiveRootPath,
          clientId: src.clientId,
          rootId: src.rootId,
          cookieMode: (src.cookie ?? '').isNotEmpty,
        );
      } catch (e) {
        debugPrint(
          '[LibraryStore] purge stale cache failed for ${target.$2}: $e',
        );
      }
      cleanupItems++;
      if (cleanupItems % 25 == 0) await maintenanceBoundary();
    }

    // AI 超分遗留任务（按 book_key 匹配失效记录）一并清理。
    try {
      final tasks = await dbLoadAllAiTasks();
      cleanupItems = 0;
      for (final t in tasks.where((t) => removedKeys.contains(t.bookKey))) {
        await maintenanceBoundary();
        await dbDeleteAiTask(id: t.id);
        cleanupItems++;
        if (cleanupItems % 50 == 0) await maintenanceBoundary();
      }
    } catch (_) {}

    notifyListeners();
    await saveToDisk();
    await reloadCatalog();
    return (
      staleRecords.length,
      allMetaKeys.length,
      freed.toInt(),
      alignFailed,
    );
  }

  /// 通过 RemoteScanEngine 做一次完整对齐。只有 complete 终态才发布 generation；
  /// 失败、取消、认证/权限/网络错误均返回 false 并保留旧证明。
  Future<bool> _alignRemoteIndex(BookSource s) async {
    if (!s.needsSession) return false;
    try {
      final session = await remoteSessionFor(s);
      if (session == null) return false;
      final result = await RemoteScanCoordinator.instance.rescan(
        s,
        session,
        'full',
      );
      return result.status == 'complete';
    } catch (e) {
      debugPrint('[LibraryStore] 在线索引对齐失败 ${s.id}: $e');
      return false;
    }
  }

  // ---- Meta（委托给 BookRepository） ----

  BookMeta metaOf(BookSource source, String path) =>
      _books.metaOf(source, path);

  void updateMeta(BookMeta m) {
    _books.updateMeta(m);
    // 标签关联是独立的投影表；编辑标题/作者时不能用 m.tags 做全量替换，
    // 否则会把刮削标签、已读和 AI 标签一并冲掉。旧 JSON 中仍有手动 tags
    // 时只做幂等补链，删除操作由 TagRepository.unlink 单独处理。
    for (final tag in m.tags) {
      if (tag.trim().isNotEmpty) TagRepository.instance.link(m.key, tag.trim());
    }
    for (final mt in m.metaTags) {
      if (mt.isNotEmpty) TagRepository.instance.link(m.key, mt);
    }
    notifyListeners();
    _notifyAfterPersistence(saveToDisk());
  }

  // ---- 设置 ----

  void updateSettings(AppSettings s) {
    settings = s;
    _syncRemoteScanSettingsGate();
    notifyListeners();
    saveToDisk();
  }

  /// Per-page wide-page decisions stay local and never enter BookMeta sync.
  Future<bool?> readerPageSplitOverride(String bookKey, int pageIndex) =>
      rust_book.getReaderPageSplitOverride(
        bookKey: bookKey,
        pageIndex: pageIndex,
      );

  Future<void> setReaderPageSplitOverride(
    String bookKey,
    int pageIndex,
    bool? split,
  ) => rust_book.setReaderPageSplitOverride(
    bookKey: bookKey,
    pageIndex: pageIndex,
    split: split,
  );

  /// Persist the recent random-reading history without changing read progress.
  void recordRandomSelection(String bookKey) {
    final history = [...settings.recentRandomBookKeys]
      ..removeWhere((key) => key == bookKey)
      ..add(bookKey);
    if (history.length > RandomReadSelector.recentHistoryLimit) {
      history.removeRange(
        0,
        history.length - RandomReadSelector.recentHistoryLimit,
      );
    }
    settings.recentRandomBookKeys = history;
    updateSettings(settings);
  }

  void _syncRemoteScanSettingsGate() {
    RemoteScanCoordinator.automaticStartsEnabled = () =>
        settings.remoteBackgroundScanEnabled;
  }

  // ---- 标签相关（委托给 TagRepository + 跨模块协调） ----

  Set<String> metaTagNames() {
    final set = _books.metaTagNames(hasAnyRead: _records.hasAnyRead());
    // "AI超分" 是元数据标签 — 只要有任何漫画关联了它，就出现在元数据标签区
    final allBookKeys = TagRepository.instance.bookKeysForTag('AI超分');
    if (allBookKeys.isNotEmpty) set.add('AI超分');
    return set;
  }

  List<String> allTags() => TagRepository.instance.allNames();

  List<(String, int, int)> tagStats() {
    final tagMap = TagRepository.instance.tagStats();
    final map = <String, (int, int)>{};
    for (final entry in tagMap.entries) {
      map[entry.key] = (entry.value, 0);
    }
    final readCountsByBookKey = <String, int>{};
    for (final r in records.values) {
      final bookKey = bookKeyOf(r.sourceType, r.sourceId, r.path);
      readCountsByBookKey[bookKey] =
          (readCountsByBookKey[bookKey] ?? 0) + r.readCount;
    }
    final readCountsByTag = TagRepository.instance.readCountsByTag(
      readCountsByBookKey,
    );
    for (final entry in readCountsByTag.entries) {
      final prev = map[entry.key] ?? (0, 0);
      map[entry.key] = (prev.$1, entry.value);
    }
    for (final m in metas.values) {
      for (final f in [m.author, m.genre, m.series]) {
        if (f.isNotEmpty && !map.containsKey(f)) {
          map[f] = (1, 0);
        }
      }
    }
    final list = map.entries
        .map((e) => (e.key, e.value.$1, e.value.$2))
        .toList();
    list.sort((a, b) => b.$2.compareTo(a.$2));
    return list;
  }

  static void _mergeMetaPreferSource(BookMeta target, BookMeta source) {
    _mergeMeta(target, source);
    if (source.author.trim().isNotEmpty) target.author = source.author;
    if (source.genre.trim().isNotEmpty) target.genre = source.genre;
    if (source.series.trim().isNotEmpty) target.series = source.series;
    if (source.title.trim().isNotEmpty) target.title = source.title;
    if (source.chineseTitle.trim().isNotEmpty) {
      target.chineseTitle = source.chineseTitle;
    }
  }

  static BookMetaDto _metaDto(BookMeta m) => BookMetaDto(
    key: m.key,
    volume: m.volume,
    chapter: m.chapter,
    coverPage: m.coverPage,
    cropX: m.cropX,
    cropY: m.cropY,
    cropW: m.cropW,
    cropH: m.cropH,
    author: m.author,
    genre: m.genre,
    series: m.series,
    title: m.title,
    chineseTitle: m.chineseTitle,
    summary: m.summary,
    comment: m.comment,
    rotations: jsonEncode(m.rotations.map((k, v) => MapEntry('$k', v))),
  );

  /// Classify metadata tags in one pass. The tag manager uses this projection
  /// instead of scanning every BookMeta once per tag tile.
  Map<String, String> metaTagCategories() {
    final counts = <String, ({int author, int genre, int series})>{};
    for (final meta in metas.values) {
      void add(String value, String field) {
        if (value.isEmpty) return;
        final current = counts[value] ?? (author: 0, genre: 0, series: 0);
        counts[value] = switch (field) {
          'author' => (
            author: current.author + 1,
            genre: current.genre,
            series: current.series,
          ),
          'genre' => (
            author: current.author,
            genre: current.genre + 1,
            series: current.series,
          ),
          _ => (
            author: current.author,
            genre: current.genre,
            series: current.series + 1,
          ),
        };
      }

      add(meta.author, 'author');
      add(meta.genre, 'genre');
      add(meta.series, 'series');
    }
    final result = <String, String>{};
    for (final entry in counts.entries) {
      final c = entry.value;
      result[entry.key] = c.author >= c.genre && c.author >= c.series
          ? '作者'
          : c.genre >= c.series
          ? '类别'
          : '系列';
    }
    if (TagRepository.instance.bookKeysForTag('AI超分').isNotEmpty) {
      result['AI超分'] = 'AI超分';
    }
    if (_records.hasAnyRead()) result['已读'] = '状态';
    return result;
  }

  /// Resolve a logical book key back to a currently indexed physical asset.
  ///
  /// Archive aliases intentionally share a logical key (zip/cbz/etc.), but a
  /// reader still needs the real path including its extension. The resolver
  /// consults only the persisted local catalog; it never lists or stats a
  /// remote source. A null result means the asset is no longer present in the
  /// local catalog and callers should keep it non-readable.
  Future<String?> resolveLiveAssetPath(String bookKey) async {
    final parts = bookKey.split('|');
    if (parts.length < 3) return null;
    final sourceType = parts[0];
    final sourceId = parts[1];
    final entries = await dbLoadLibraryIndexForSource(sourceId: sourceId);
    for (final entry in entries) {
      if (entry.deleted) continue;
      if (entry.entryType != 'file' && entry.entryType != 'dir') continue;
      if (bookKeyOf(sourceType, sourceId, entry.path) == bookKey) {
        return entry.path;
      }
    }

    // A freshly-created local source may not have completed its first index
    // yet. It is safe to check local filesystem candidates, but never do this
    // fallback for cloud/remote sources (which would violate local-first).
    if (sourceType == 'local') {
      final raw = parts.sublist(2).join('|');
      final candidates = <String>[raw];
      for (final ext in const [
        '.cbz',
        '.zip',
        '.cbr',
        '.rar',
        '.pdf',
        '.epub',
        '.mobi',
      ]) {
        candidates.add('$raw$ext');
      }
      for (final candidate in candidates) {
        if (await File(candidate).exists()) return candidate;
      }
    }
    return null;
  }

  /// 标签详情书目。有读记录的书用记录标题；没有记录的书（仅标签/元数据关联）
  /// 标题从离线索引取**真实文件名**——不能直接取 path 尾段，因为 quark/115 等
  /// id-path 源的 path 是 32hex 素材 id / pick_code，会显示成一串乱码。
  Future<List<ReadRecord>> recordsByTag(String tag) async {
    final result = <ReadRecord>[];
    final seen = <String>{};
    // sourceId -> {path: name}：离线索引真实文件名（按需惰性加载一次）。
    final indexNames = <String, Map<String, String>>{};

    Future<String> titleOf(String stype, String sid, String spath) async {
      // 本地 / WebDAV / SFTP 等层级路径：尾段即文件名。
      if (spath.contains('/')) return spath.split('/').last;
      final names = indexNames[sid] ??= <String, String>{
        for (final e in await dbLoadLibraryIndexForSource(sourceId: sid))
          e.path: e.name,
      };
      return names[spath] ?? spath.split('/').last;
    }

    Future<ReadRecord> withLivePath(ReadRecord record) async {
      final livePath = await resolveLiveAssetPath(record.key);
      if (livePath == null || livePath == record.path) return record;
      return ReadRecord(
        key: record.key,
        sourceType: record.sourceType,
        sourceId: record.sourceId,
        path: livePath,
        title: await titleOf(record.sourceType, record.sourceId, livePath),
        lastPage: record.lastPage,
        readCount: record.readCount,
        lastReadAt: record.lastReadAt,
      );
    }

    final bookKeys = TagRepository.instance.bookKeysForTag(tag);
    for (final bk in bookKeys) {
      final existing = records[bk];
      if (existing != null) {
        result.add(await withLivePath(existing));
      } else {
        final parts = bk.split('|');
        final stype = parts.isNotEmpty ? parts[0] : 'local';
        final sid = parts.length > 1 ? parts[1] : '';
        final spath = parts.sublist(2).join('|');
        final livePath = await resolveLiveAssetPath(bk) ?? spath;
        result.add(
          ReadRecord(
            key: bk,
            sourceType: stype,
            sourceId: sid,
            path: livePath,
            title: await titleOf(stype, sid, livePath),
            lastPage: 0,
            readCount: 0,
            lastReadAt: 0,
          ),
        );
      }
      seen.add(bk);
    }

    for (final m in metas.values) {
      if (m.author != tag && m.genre != tag && m.series != tag) continue;
      if (seen.contains(m.key)) continue;
      final existing = records[m.key];
      if (existing != null) {
        result.add(await withLivePath(existing));
      } else {
        final parts = m.key.split('|');
        final stype = parts.isNotEmpty ? parts[0] : 'local';
        final sid = parts.length > 1 ? parts[1] : '';
        final spath = parts.sublist(2).join('|');
        final livePath = await resolveLiveAssetPath(m.key) ?? spath;
        result.add(
          ReadRecord(
            key: m.key,
            sourceType: stype,
            sourceId: sid,
            path: livePath,
            title: await titleOf(stype, sid, livePath),
            lastPage: 0,
            readCount: 0,
            lastReadAt: 0,
          ),
        );
      }
      seen.add(m.key);
    }

    return result;
  }

  void renameTag(String oldName, String newName) {
    if (newName.isEmpty || oldName == newName) return;
    for (final m in metas.values) {
      if (m.tags.contains(oldName)) {
        m.tags.remove(oldName);
        m.tags.add(newName);
      }
      if (m.author == oldName) {
        m.author = newName;
      }
      if (m.genre == oldName) {
        m.genre = newName;
      }
      if (m.series == oldName) {
        m.series = newName;
      }
    }
    TagRepository.instance.rename(oldName, newName);
    notifyListeners();
    _notifyAfterPersistence(saveToDisk());
  }

  void deleteTag(String name) {
    for (final m in metas.values) {
      m.tags.remove(name);
      if (m.author == name) m.author = '';
      if (m.genre == name) m.genre = '';
      if (m.series == name) m.series = '';
    }
    TagRepository.instance.delete(name);
    notifyListeners();
    _notifyAfterPersistence(saveToDisk());
  }

  // ---- 跨书源搜索 ----

  List<({String bookKey, BookSource source, String path, String title})>
  globalSearch({
    String text = '',
    Set<String> tags = const {},
    bool includeRemoteOnly = true,
  }) {
    final results =
        <({String bookKey, BookSource source, String path, String title})>[];
    final seen = <String>{};
    for (final s in sources) {
      if (!includeRemoteOnly && s.remoteOnly) continue;
      for (final entry in metas.entries) {
        final bookKey = entry.key;
        if (!bookKey.startsWith('${s.type}|${s.id}|')) continue;
        if (seen.contains(bookKey)) continue;
        seen.add(bookKey);
        final meta = entry.value;
        final path = bookKey.substring('${s.type}|${s.id}|'.length);
        final record = records[bookKey];
        final title = record?.title ?? path.split('/').last;
        if (text.isNotEmpty &&
            !title.toLowerCase().contains(text.toLowerCase())) {
          continue;
        }
        if (tags.isNotEmpty) {
          final assigned = <String>{
            ...meta.tags,
            ...meta.metaTags,
            ...TagRepository.instance.tagsForBook(bookKey),
          };
          if (!tags.every(assigned.contains)) continue;
        }
        results.add((bookKey: bookKey, source: s, path: path, title: title));
      }
    }
    results.sort((a, b) => a.title.compareTo(b.title));
    return results;
  }

  // ---- 批量操作 ----

  List<String> get metaFields => ['author', 'genre', 'series', '已读'];

  Future<BatchTagResult> batchTag(
    BookSource src,
    Iterable<BatchTagTarget> targets,
    String tag, {
    void Function(BatchTagProgress progress)? onProgress,
  }) => ForegroundWorkCoordinator.instance.runForeground(
    () => _batchTag(src, targets, tag, onProgress: onProgress),
  );

  Future<BatchTagResult> _batchTag(
    BookSource src,
    Iterable<BatchTagTarget> targets,
    String tag, {
    void Function(BatchTagProgress progress)? onProgress,
  }) async {
    final previousBatch = _batchTagTail;
    final batchSlot = Completer<void>();
    _batchTagTail = batchSlot.future;
    Completer<void>? batchGate;
    final totalStopwatch = Stopwatch()..start();
    final indexStopwatch = Stopwatch();
    final persistStopwatch = Stopwatch();

    final supplied = targets.toList(growable: false);
    final uniqueTargets = <String, BatchTagTarget>{};
    for (final target in supplied) {
      final identity =
          '${target.entryType}\u0000${LibraryIndexService.normalizeIndexPath(target.path)}';
      uniqueTargets.putIfAbsent(identity, () => target);
    }
    final targetList = uniqueTargets.values.toList(growable: false);
    final duplicateCount = supplied.length - targetList.length;
    final normalizedTag = tag.trim();
    var succeeded = 0;
    var skipped = duplicateCount;
    var failed = 0;
    var tagsChanged = false;
    var indexCommitted = false;
    var mutationFinalized = false;

    void publish(
      BatchTagPhase phase, {
      required int processed,
      required int total,
    }) {
      try {
        onProgress?.call(
          BatchTagProgress(
            phase: phase,
            processed: processed,
            total: total,
            succeeded: succeeded,
            skipped: skipped,
            failed: failed,
          ),
        );
      } catch (_) {
        // Progress observers must not turn a committed chunk into a failure.
      }
    }

    Future<void> finalizeCommittedChanges() async {
      if (mutationFinalized) return;
      mutationFinalized = true;
      if (succeeded > 0) {
        if (tagsChanged) TagRepository.instance.notifyBatchMutation();
        notifyListeners();
      }
      if (indexCommitted || succeeded > 0) _notifyCommittedMutation();
      if (succeeded == 0) return;
      try {
        await _writeJsonBackupOnly();
      } catch (error) {
        debugPrint(
          '[LibraryStore] batch JSON export failed: ${error.runtimeType}',
        );
      }
    }

    try {
      await previousBatch;
      while (_activeLoads > 0) {
        final loadsIdle = _loadsIdle;
        if (loadsIdle == null) break;
        await loadsIdle.future;
      }
      batchGate = Completer<void>();
      _batchTagGate = batchGate;
      // Keep any older full-library save ahead of this targeted write. While
      // this gate is held, later save/load requests wait until all committed
      // chunks have also been applied to the in-memory repositories.
      await _waitForPendingSave();
      if (normalizedTag.isEmpty) {
        skipped += targetList.length;
        publish(
          BatchTagPhase.complete,
          processed: supplied.length,
          total: supplied.length,
        );
        return BatchTagResult(
          total: supplied.length,
          succeeded: succeeded,
          skipped: skipped,
          failed: failed,
        );
      }

      const chunkSize = 100;
      final indexedTargets = <BatchTagTarget>[];
      publish(BatchTagPhase.indexing, processed: 0, total: targetList.length);
      indexStopwatch.start();
      for (var start = 0; start < targetList.length; start += chunkSize) {
        final end = (start + chunkSize).clamp(0, targetList.length).toInt();
        final chunk = targetList.sublist(start, end);
        final entries = <IndexEntryInput>[];
        for (final target in chunk) {
          final key = bookKeyOf(src.type, src.id, target.path);
          final existingMeta = metas[key];
          final fallbackName = target.path.split(RegExp(r'[\\/]')).last;
          final name = existingMeta?.title.trim().isNotEmpty == true
              ? existingMeta!.title
              : fallbackName;
          entries.add(
            IndexEntryInput(
              path: target.path,
              entryType: target.entryType,
              name: name,
              parentPath: FolderSnapshotStore.instance.parentDirOf(
                src,
                target.path,
              ),
            ),
          );
        }
        try {
          await dbEnsureIndexEntries(sourceId: src.id, entries: entries);
          indexedTargets.addAll(chunk);
          indexCommitted = true;
        } catch (error) {
          failed += chunk.length;
          debugPrint(
            '[LibraryStore] batch index chunk failed: ${error.runtimeType}',
          );
        }
        publish(
          BatchTagPhase.indexing,
          processed: end,
          total: targetList.length,
        );
      }
      indexStopwatch.stop();

      String? existingField;
      if (normalizedTag != '已读') {
        for (final meta in metas.values) {
          if (meta.author == normalizedTag) {
            existingField = 'author';
            break;
          }
          if (meta.genre == normalizedTag) {
            existingField = 'genre';
            break;
          }
          if (meta.series == normalizedTag) {
            existingField = 'series';
            break;
          }
        }
      }

      publish(
        BatchTagPhase.persisting,
        processed: 0,
        total: indexedTargets.length,
      );
      persistStopwatch.start();
      for (var start = 0; start < indexedTargets.length; start += chunkSize) {
        final end = (start + chunkSize).clamp(0, indexedTargets.length).toInt();
        final chunk = indexedTargets.sublist(start, end);
        final writes = <BatchTagWriteDto>[];
        final stagedMetas = <String, BookMeta>{};
        final links = <({String bookKey, String tagName})>[];

        for (final target in chunk) {
          final key = bookKeyOf(src.type, src.id, target.path);
          final current = metas[key];
          BookMeta? updated;
          BookMetaDto? metaDto;
          if (normalizedTag != '已读') {
            updated = current == null
                ? BookMeta(key: key)
                : BookMeta.fromJson(current.toJson());
            final beforeAuthor = updated.author;
            final beforeGenre = updated.genre;
            final beforeSeries = updated.series;
            final beforeTags = List<String>.of(updated.tags);
            switch (existingField) {
              case 'author':
                if (updated.author.isEmpty) {
                  updated.author = normalizedTag;
                } else if (!updated.tags.contains(normalizedTag)) {
                  updated.tags.add(normalizedTag);
                }
              case 'genre':
                if (updated.genre.isEmpty) {
                  updated.genre = normalizedTag;
                } else if (!updated.tags.contains(normalizedTag)) {
                  updated.tags.add(normalizedTag);
                }
              case 'series':
                if (updated.series.isEmpty) {
                  updated.series = normalizedTag;
                } else if (!updated.tags.contains(normalizedTag)) {
                  updated.tags.add(normalizedTag);
                }
              default:
                if (!updated.tags.contains(normalizedTag)) {
                  updated.tags.add(normalizedTag);
                }
            }
            final changed =
                current == null ||
                beforeAuthor != updated.author ||
                beforeGenre != updated.genre ||
                beforeSeries != updated.series ||
                !listEquals(beforeTags, updated.tags);
            if (changed) {
              metaDto = BookMetaDto(
                key: updated.key,
                coverPage: updated.coverPage,
                cropX: updated.cropX,
                cropY: updated.cropY,
                cropW: updated.cropW,
                cropH: updated.cropH,
                author: updated.author,
                genre: updated.genre,
                series: updated.series,
                title: updated.title,
                chineseTitle: updated.chineseTitle,
                summary: updated.summary,
                comment: updated.comment,
                rotations: jsonEncode(
                  updated.rotations.map(
                    (key, value) => MapEntry('$key', value),
                  ),
                ),
                volume: updated.volume,
                chapter: updated.chapter,
              );
              stagedMetas[key] = updated;
            }
          }
          writes.add(
            BatchTagWriteDto(
              bookKey: key,
              tagName: normalizedTag,
              meta: metaDto,
            ),
          );
          links.add((bookKey: key, tagName: normalizedTag));
        }

        try {
          await dbApplyBatchTagWrites(writes: writes);
          for (final entry in stagedMetas.entries) {
            _books.updateMeta(entry.value);
          }
          tagsChanged =
              TagRepository.instance.linkMany(links, notify: false) ||
              tagsChanged;
          succeeded += chunk.length;
        } catch (error) {
          failed += chunk.length;
          debugPrint(
            '[LibraryStore] batch tag chunk failed: ${error.runtimeType}',
          );
        }
        publish(
          BatchTagPhase.persisting,
          processed: end,
          total: indexedTargets.length,
        );
      }
      persistStopwatch.stop();

      await finalizeCommittedChanges();
      publish(
        BatchTagPhase.complete,
        processed: supplied.length,
        total: supplied.length,
      );
      return BatchTagResult(
        total: supplied.length,
        succeeded: succeeded,
        skipped: skipped,
        failed: failed,
      );
    } catch (error) {
      await finalizeCommittedChanges();
      final unaccounted = supplied.length - succeeded - skipped - failed;
      if (unaccounted > 0) failed += unaccounted;
      debugPrint('[LibraryStore] batch tag failed: ${error.runtimeType}');
      publish(
        BatchTagPhase.complete,
        processed: supplied.length,
        total: supplied.length,
      );
      return BatchTagResult(
        total: supplied.length,
        succeeded: succeeded,
        skipped: skipped,
        failed: failed,
      );
    } finally {
      if (batchGate != null) {
        if (identical(_batchTagGate, batchGate)) _batchTagGate = null;
        if (!batchGate.isCompleted) batchGate.complete();
      }
      if (!batchSlot.isCompleted) batchSlot.complete();
      if (kProfileMode) {
        debugPrint(
          '[performance] phase=batch_tag elapsed_ms=${totalStopwatch.elapsedMilliseconds}'
          ' index_elapsed_ms=${indexStopwatch.elapsedMilliseconds}'
          ' persist_elapsed_ms=${persistStopwatch.elapsedMilliseconds}'
          ' entries=${supplied.length} succeeded=$succeeded skipped=$skipped failed=$failed',
        );
      }
    }
  }
}
