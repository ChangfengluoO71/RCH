import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:path_provider/path_provider.dart';

import '../repository/book_repository.dart';
import '../repository/record_repository.dart';
import '../repository/tag_repository.dart';
import '../src/rust/api/cache.dart';
import '../src/rust/api/db.dart';
import 'library_index_service.dart';
import 'models.dart';
import 'remote_listing.dart';

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

  // ---- 全量保存（防抖 + 生命周期 flush） ----

  static const Duration _saveDebounceDuration = Duration(milliseconds: 800);
  static const String _jsonReconcileDoneKey = 'json_reconcile_done';

  Timer? _saveTimer;
  Future<void> _saveQueue = Future<void>.value();
  bool _saveDirty = false;
  Completer<void>? _saveWaiter;
  Object? _lastSaveError;

  /// 最近一次持久化失败的原因（无失败为 null）。供 UI 观测。
  Object? get lastSaveError => _lastSaveError;

  /// 加载全部数据（SQLite 优先，空则回退 library.json）。
  /// `force=true`：同步成功后强制重载，避免 Rust 侧已变更而 Dart 内存态过期
  /// （ADR-028 §12.5；repository 的 loadFromSqlite 均先 clear，重载安全）。
  Future<void> load({bool force = false}) async {
    if (_loaded && !force) return;
    _loaded = false;
    try {
      final migrated = await dataIsMigrated();
      if (migrated) {
        await _loadFromSqlite();
        await _reconcileFromJsonIfNeeded();
        _migrateAliasKeys();
        _loaded = true;
        notifyListeners();
        saveToDisk();
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
        final j = jsonDecode(await f.readAsString()) as Map<String, dynamic>;
        _books.loadFromJson(j);
        _records.loadFromJson(j);
        if (j['settings'] != null) {
          settings = AppSettings.fromJson(Map<String, dynamic>.from(j['settings']));
        }
      }
    } catch (e) {
      debugPrint('[LibraryStore] load JSON failed: $e');
    }
    _loaded = true;
    notifyListeners();
    _migrateAliasKeys();
    saveToDisk();
  }

  /// 从 SQLite 加载全量数据。
  Future<void> _loadFromSqlite() async {
    await _books.loadFromSqlite();
    await _records.loadFromSqlite();
    await TagRepository.instance.loadFromSqlite();

    final settingDtos = await dbLoadAllSettings();
    final settingsMap = <String, dynamic>{};
    for (final dto in settingDtos) {
      settingsMap[dto.key] = _tryParseJson(dto.value);
    }
    if (settingsMap.isNotEmpty) {
      settings = AppSettings.fromJson(settingsMap);
    }
  }

  /// 后缀别名归并（zip↔cbz 等视为同一本）：对阅读记录/元数据/标签关联做
  /// key 归一化迁移。幂等，重复执行无副作用；旧 key 行仍留在 SQLite 中，
  /// 每次启动会被再次合并（数据层同步任务将统一清理）。
  void _migrateAliasKeys() {
    final recRepo = _records;
    final bookRepo = _books;

    // 1) 阅读记录：合并到规范化 key，保留最新进度与累计次数。
    final mergedRecords = <String, ReadRecord>{};
    for (final r in recRepo.records.values.toList()) {
      final nk = bookKeyOf(r.sourceType, r.sourceId, r.path);
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
            path: r.path,
            title: r.title,
            lastPage: r.lastPage,
            readCount: r.readCount,
            lastReadAt: r.lastReadAt,
          );
        } else {
          _mergeRecord(existing, r);
        }
        TagRepository.instance.remapBookKey(r.key, nk);
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
      final nk = bookKeyOf(st, sid, p);
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
          _mergeMeta(existing, m);
        }
        TagRepository.instance.remapBookKey(m.key, nk);
      }
    }
    bookRepo.metas
      ..clear()
      ..addAll(mergedMetas);
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
    if ((raw.startsWith('{') || raw.startsWith('[')) && (raw.endsWith('}') || raw.endsWith(']'))) {
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
  Future<void> saveToDisk() {
    _saveDirty = true;
    final waiter = _saveWaiter ??= Completer<void>();
    _saveTimer ??= Timer(_saveDebounceDuration, _kickSave);
    return waiter.future;
  }

  /// 生命周期退出前强制落盘：等待所有排队与执行中的保存完成。
  Future<void> flushPendingSave() async {
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
      final f = await _file();
      final data = {
        ..._books.toJson(),
        ..._records.toJson(),
        'settings': settings.toJson(),
        ...TagRepository.instance.toJson(),
      };
      await f.writeAsString(jsonEncode(data));
    } catch (e, st) {
      debugPrint('[LibraryStore] JSON export failed: $e');
      debugPrintStack(stackTrace: st);
    }
    if (sqliteError != null) {
      throw sqliteError;
    }
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
      final done = (await dbLoadAllSettings())
          .any((d) => d.key == _jsonReconcileDoneKey);
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
            _records.records[key] =
                ReadRecord.fromJson(Map<String, dynamic>.from(entry.value));
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

  void addSource(BookSource s) {
    _books.addSource(s);
    notifyListeners(); saveToDisk();
  }

  void removeSource(String id) {
    _books.removeSource(id);
    notifyListeners(); saveToDisk();
  }

  void updateSource(String id, {String? name, String? url, String? username, String? password, int? port, String? path, String? refreshToken, String? clientId, String? clientSecret, String? rootId, String? cookie, String? note}) {
    _books.updateSource(id, name: name, url: url, username: username, password: password, port: port, path: path, refreshToken: refreshToken, clientId: clientId, clientSecret: clientSecret, rootId: rootId, cookie: cookie, note: note);
    // Phase 6.1 采纳语义：用户编辑保存即视为"本机配置该书源"——
    // 同步进来的远端书源（remote_only）编辑后转为逻辑本地源（归入本机区）。
    final src = sourceById(id);
    if (src != null && src.remoteOnly) {
      src.remoteOnly = false;
      src.originDeviceId = null;
    }
    notifyListeners(); saveToDisk();
  }

  void updateSourceCapability(String id, String label) {
    _books.updateSourceCapability(id, label);
    notifyListeners(); saveToDisk();
  }

  BookSource? sourceById(String id) => _books.sourceById(id);

  Future<void> removeSourceWithCleanup(String id) async {
    final src = sourceById(id);
    // 删除前抓取该源全部阅读记录（用于删除后清理磁盘缓存）
    final affected = _records.records.values.where((r) => r.sourceId == id).toList();
    _books.removeSource(id);
    if (src != null) {
      final prefix = '${src.type}|${src.id}|';
      _records.removeByPrefix(prefix);
      _books.metas.removeWhere((k, _) => k.startsWith(prefix));
      TagRepository.instance.removeBookTagsByPrefix(prefix);
      // SQLite 同步删除：saveToSqlite 只 upsert 不删行，漏删会让书源/记录重启后复活。
      dbDeleteSource(id: id);
      dbDeleteRecordsBySourcePrefix(prefix: prefix);
      dbDeleteMetasBySourcePrefix(prefix: prefix);
    }
    // 磁盘缓存清理：源身份字段删除前已捕获，逐本删除 page/raw/cover 缓存。
    for (final r in affected) {
      try {
        await purgeStaleBookCache(
          sourceType: r.sourceType,
          path: r.path,
          url: src?.url,
          port: src?.port,
          rootPath: src?.path ?? '',
          clientId: src?.clientId,
          rootId: src?.rootId,
          cookieMode: (src?.cookie ?? '').isNotEmpty,
        );
      } catch (e) {
        debugPrint('[LibraryStore] removeSource cache cleanup failed for ${r.key}: $e');
      }
    }
    notifyListeners(); saveToDisk();
  }

  // ---- Record（委托给 RecordRepository） ----

  Future<void> recordRead({
    required BookSource source,
    required String path,
    required String title,
    int? page,
  }) async {
    final key = RecordRepository.keyOf(source.type, source.id, path);
    final r = _records.upsert(source: source, path: path, title: title, page: page);
    TagRepository.instance.link(key, '已读');
    // ADR-029 触及即补：已读的书自动入离线索引（本地 upsert，零网络）
    LibraryIndexService.ensureIndexed(source, path, name: title);
    notifyListeners();
    try {
      // 轻量落盘（B 方案）：只写记录 + 标签关联，不写全量 JSON。
      await _records.saveOneToSqlite(r);
      await TagRepository.instance.persistBookLinks(key);
    } catch (e, st) {
      _lastSaveError = e;
      debugPrint('[LibraryStore] recordRead save failed: $e');
      debugPrintStack(stackTrace: st);
    }
  }

  ReadRecord? recordOf(BookSource source, String path) => _records.of(source, path);

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
    saveToDisk();
  }

  /// 清空阅读统计：所有记录阅读次数归零（保留最近阅读列表与每本书的进度）。
  /// 用户「清空全部缓存 → 仅清空阅读统计」时调用。
  Future<void> resetReadCounts() async {
    for (final r in _records.records.values) {
      r.readCount = 0;
    }
    await dbResetReadCounts();
    notifyListeners();
    saveToDisk();
  }

  List<ReadRecord> get recent => _records.recent();
  List<ReadRecord> get mostRead => _records.mostRead();

  /// 清理失效漫画数据：源已删除的记录/元数据、本地文件丢失或远程已删除
  /// （在线索引对齐 + 离线索引墓碑）的记录，以及这些 key 上的标签关联、
  /// AI 任务与磁盘缓存（page/raw/cover）。
  /// 返回 (清理的记录数, 清理的元数据数, 释放的缓存字节数, 在线核对失败的远程源数)。
  Future<(int, int, int, int)> purgeStaleData() async {
    final sourceIds = sources.map((s) => s.id).toSet();

    // Phase 0：在线索引对齐（仅远程源）。删除感知的前提是把"远程现状"落进
    // library_index：整源重爬后消失的条目被软删（deleted=1）成为墓碑证据。
    // 失败/离线 → 该源跳过，回退到 Phase 1 的存量墓碑证据（保守不清）。
    var alignFailed = 0;
    for (final s in sources) {
      if (s.isLocalFs || s.isGhost) continue;
      final ok = await _alignRemoteIndex(s);
      if (!ok) alignFailed++;
    }

    // 远程失效证据：离线索引中 deleted=1 的条目 = 整源重建/对齐时远程文件已消失的软删墓碑。
    // 注意必须用专用墓碑查询（dbLoadLibraryIndexForSource 的 SQL 过滤 deleted=0，拿不到墓碑）。
    // 只对远程书源收集（本地书源以文件存在性判定）；索引缺失/不可读时跳过该证据。
    final tombstones = <String, Set<String>>{};
    for (final s in sources) {
      if (s.isLocalFs) continue;
      try {
        final gone = await dbLoadLibraryIndexTombstones(sourceId: s.id);
        if (gone.isNotEmpty) tombstones[s.id] = gone.toSet();
      } catch (_) {/* 索引不可读则无墓碑证据，保守不清 */}
    }

    // 1) 失效阅读记录（源已删除 / 本地文件丢失 / 远程墓碑）
    final staleRecords = _records.purgeStale(sources, remoteTombstones: tombstones);

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
    final allMetaKeys = <String>{...staleMetas, ...recordMetaKeys};

    final removedKeys = <String>{...staleRecords.map((r) => r.key), ...staleMetas};
    if (removedKeys.isEmpty) return (0, 0, 0, alignFailed);

    // 内存清理：元数据 + 失效 key 上的标签关联
    for (final k in allMetaKeys) {
      _books.metas.remove(k);
    }
    for (final k in removedKeys) {
      TagRepository.instance.setBookTags(k, const []);
    }

    // SQLite 清理：记录逐条删；失效记录对应的元数据逐条删；
    // 源已删的元数据按来源前缀批量删（连未进内存的残留行一起清）。
    for (final r in staleRecords) {
      dbDeleteRecord(key: r.key);
      dbDeleteMeta(key: r.key);
    }
    final prefixes = <String>{};
    for (final k in staleMetas) {
      final parts = k.split('|');
      if (parts.length >= 2) prefixes.add('${parts[0]}|${parts[1]}|');
    }
    for (final p in prefixes) {
      dbDeleteMetasBySourcePrefix(prefix: p);
    }

    // 磁盘缓存清理：每条失效记录删除其 page/raw/cover 缓存（源身份字段重建命名空间）。
    var freed = BigInt.zero;
    for (final r in staleRecords) {
      final src = sourceById(r.sourceId);
      if (src == null || src.isGhost) continue; // 幽灵书源无可读缓存；源已删由 removeSource 路径处理
      try {
        freed += await purgeStaleBookCache(
          sourceType: r.sourceType,
          path: r.path,
          url: src.url,
          port: src.port,
          rootPath: src.path,
          clientId: src.clientId,
          rootId: src.rootId,
          cookieMode: (src.cookie ?? '').isNotEmpty,
        );
      } catch (e) {
        debugPrint('[LibraryStore] purge stale cache failed for ${r.key}: $e');
      }
    }

    // AI 超分遗留任务（按 book_key 匹配失效记录）一并清理。
    try {
      final tasks = await dbLoadAllAiTasks();
      for (final t in tasks.where((t) => removedKeys.contains(t.bookKey))) {
        await dbDeleteAiTask(id: t.id);
      }
    } catch (_) {}

    notifyListeners();
    saveToDisk();
    return (staleRecords.length, allMetaKeys.length, freed.toInt(), alignFailed);
  }

  /// 对远程书源做一次在线索引对齐（连接 + 全树枚举 → dbReplaceSourceLibraryIndex，
  /// 消失条目墓碑化），为"远程已删除"判定提供证据。失败返回 false（保守跳过）。
  Future<bool> _alignRemoteIndex(BookSource s) async {
    if (!s.needsSession) return false;
    try {
      final session = await remoteSessionFor(s);
      if (session == null) return false;
      await LibraryIndexService.instance.refreshSourceIndex(
        source: s,
        force: true,
        listRemote: (p) => listRemoteDirFor(s, session: session, path: p),
      );
      return true;
    } catch (e) {
      debugPrint('[LibraryStore] 在线索引对齐失败 ${s.id}: $e');
      return false;
    }
  }

  // ---- Meta（委托给 BookRepository） ----

  BookMeta metaOf(BookSource source, String path) => _books.metaOf(source, path);

  void updateMeta(BookMeta m) {
    _books.updateMeta(m);
    // '已读' 是自动/手动维护的独立关联（recordRead 打标、详情页可切换），不在
    // m.tags（手动标签）里；setBookTags 是全量替换，替换前先保留'已读'，
    // 否则编辑元数据标签会把已读状态冲掉（readCount 仍在，界面却显示未读）。
    final hadReadTag = TagRepository.instance.bookKeysForTag('已读').contains(m.key);
    TagRepository.instance.setBookTags(m.key, m.tags);
    if (hadReadTag) TagRepository.instance.link(m.key, '已读');
    for (final mt in m.metaTags) {
      if (mt.isNotEmpty) TagRepository.instance.link(m.key, mt);
    }
    notifyListeners(); saveToDisk();
  }

  // ---- 设置 ----

  void updateSettings(AppSettings s) {
    settings = s;
    notifyListeners(); saveToDisk();
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
    for (final r in records.values) {
      final bookKey = bookKeyOf(r.sourceType, r.sourceId, r.path);
      final bookTags = TagRepository.instance.tagsForBook(bookKey);
      for (final t in bookTags) {
        final prev = map[t] ?? (0, 0);
        map[t] = (prev.$1, prev.$2 + r.readCount);
      }
    }
    for (final m in metas.values) {
      for (final f in [m.author, m.genre, m.series]) {
        if (f.isNotEmpty && !map.containsKey(f)) {
          map[f] = (1, 0);
        }
      }
    }
    final list = map.entries.map((e) => (e.key, e.value.$1, e.value.$2)).toList();
    list.sort((a, b) => b.$2.compareTo(a.$2));
    return list;
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

    final bookKeys = TagRepository.instance.bookKeysForTag(tag);
    for (final bk in bookKeys) {
      final existing = records[bk];
      if (existing != null) {
        result.add(existing);
      } else {
        final parts = bk.split('|');
        final stype = parts.isNotEmpty ? parts[0] : 'local';
        final sid = parts.length > 1 ? parts[1] : '';
        final spath = parts.sublist(2).join('|');
        result.add(ReadRecord(key: bk, sourceType: stype, sourceId: sid, path: spath,
            title: await titleOf(stype, sid, spath), lastPage: 0, readCount: 0, lastReadAt: 0));
      }
      seen.add(bk);
    }

    for (final m in metas.values) {
      if (m.author != tag && m.genre != tag && m.series != tag) continue;
      if (seen.contains(m.key)) continue;
      final existing = records[m.key];
      if (existing != null) {
        result.add(existing);
      } else {
        final parts = m.key.split('|');
        final stype = parts.isNotEmpty ? parts[0] : 'local';
        final sid = parts.length > 1 ? parts[1] : '';
        final spath = parts.sublist(2).join('|');
        result.add(ReadRecord(key: m.key, sourceType: stype, sourceId: sid, path: spath,
            title: await titleOf(stype, sid, spath), lastPage: 0, readCount: 0, lastReadAt: 0));
      }
      seen.add(m.key);
    }

    return result;
  }

  void renameTag(String oldName, String newName) {
    if (newName.isEmpty || oldName == newName) return;
    for (final m in metas.values) {
      if (m.tags.contains(oldName)) { m.tags.remove(oldName); m.tags.add(newName); }
      if (m.author == oldName) { m.author = newName; }
      if (m.genre == oldName)  { m.genre = newName; }
      if (m.series == oldName) { m.series = newName; }
    }
    TagRepository.instance.rename(oldName, newName);
    notifyListeners(); saveToDisk();
  }

  void deleteTag(String name) {
    for (final m in metas.values) {
      m.tags.remove(name);
      if (m.author == name) m.author = '';
      if (m.genre == name)  m.genre = '';
      if (m.series == name) m.series = '';
    }
    TagRepository.instance.delete(name);
    notifyListeners(); saveToDisk();
  }

  // ---- 跨书源搜索 ----

  List<({String bookKey, BookSource source, String path, String title})> globalSearch({
    String text = '',
    Set<String> tags = const {},
    bool includeRemoteOnly = true,
  }) {
    final results = <({String bookKey, BookSource source, String path, String title})>[];
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
        if (text.isNotEmpty && !title.toLowerCase().contains(text.toLowerCase())) continue;
        if (tags.isNotEmpty && !tags.every((t) => meta.tags.contains(t) || meta.metaTags.contains(t))) continue;
        results.add((bookKey: bookKey, source: s, path: path, title: title));
      }
    }
    results.sort((a, b) => a.title.compareTo(b.title));
    return results;
  }

  // ---- 批量操作 ----

  List<String> get metaFields => ['author', 'genre', 'series', '已读'];

  void batchTag(BookSource src, Iterable<String> paths, String tag) {
    if (tag == '已读') {
      for (final p in paths) {
        final key = bookKeyOf(src.type, src.id, p);
        TagRepository.instance.link(key, '已读');
        // ADR-029 触及即补：批量标签的书（含未读）自动入离线索引
        LibraryIndexService.ensureIndexed(src, p);
      }
      notifyListeners(); saveToDisk();
      return;
    }

    String? existingField;
    for (final m in metas.values) {
      if (m.author == tag) { existingField = 'author'; break; }
      if (m.genre == tag)  { existingField = 'genre'; break; }
      if (m.series == tag) { existingField = 'series'; break; }
    }
    for (final p in paths) {
      final m = metaOf(src, p);
      final key = bookKeyOf(src.type, src.id, p);
      LibraryIndexService.ensureIndexed(src, p, name: m.title);
      if (existingField == null) {
        if (!m.tags.contains(tag)) { m.tags.add(tag); TagRepository.instance.link(key, tag); }
      } else {
        switch (existingField) {
          case 'author':
            if (m.author.isEmpty) { m.author = tag; TagRepository.instance.link(key, tag); }
            else if (!m.tags.contains(tag)) { m.tags.add(tag); TagRepository.instance.link(key, tag); }
          case 'genre':
            if (m.genre.isEmpty) { m.genre = tag; TagRepository.instance.link(key, tag); }
            else if (!m.tags.contains(tag)) { m.tags.add(tag); TagRepository.instance.link(key, tag); }
          case 'series':
            if (m.series.isEmpty) { m.series = tag; TagRepository.instance.link(key, tag); }
            else if (!m.tags.contains(tag)) { m.tags.add(tag); TagRepository.instance.link(key, tag); }
        }
      }
    }
    notifyListeners(); saveToDisk();
  }
}
