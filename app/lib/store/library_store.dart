import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:path_provider/path_provider.dart';

import '../repository/book_repository.dart';
import '../repository/record_repository.dart';
import '../repository/tag_repository.dart';
import '../src/rust/api/db.dart';
import 'library_index_service.dart';
import 'models.dart';

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

  void removeSourceWithCleanup(String id) {
    final src = sourceById(id);
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

  List<ReadRecord> get recent => _records.recent();
  List<ReadRecord> get mostRead => _records.mostRead();

  /// 清理失效漫画数据：源已删除的记录/元数据、本地文件丢失的记录，以及这些 key 上的标签关联。
  /// 返回 (清理的记录数, 清理的元数据数)。
  (int, int) purgeStaleData() {
    final sourceIds = sources.map((s) => s.id).toSet();

    // 1) 失效阅读记录（源已删除 / 本地文件丢失）
    final staleRecords = _records.purgeStale(sources);

    // 2) 失效元数据（来源已删除）
    final staleMetas = <String>[];
    for (final m in _books.metas.values) {
      final parts = m.key.split('|');
      final sid = parts.length > 1 ? parts[1] : '';
      if (!sourceIds.contains(sid)) staleMetas.add(m.key);
    }

    final removedKeys = <String>{...staleRecords, ...staleMetas};
    if (removedKeys.isEmpty) return (0, 0);

    // 内存清理：元数据 + 失效 key 上的标签关联
    for (final k in staleMetas) {
      _books.metas.remove(k);
    }
    for (final k in removedKeys) {
      TagRepository.instance.setBookTags(k, const []);
    }

    // SQLite 清理：记录逐条删；元数据按来源前缀批量删（连未进内存的残留行一起清）
    for (final k in staleRecords) {
      dbDeleteRecord(key: k);
    }
    final prefixes = <String>{};
    for (final k in staleMetas) {
      final parts = k.split('|');
      if (parts.length >= 2) prefixes.add('${parts[0]}|${parts[1]}|');
    }
    for (final p in prefixes) {
      dbDeleteMetasBySourcePrefix(prefix: p);
    }

    notifyListeners();
    saveToDisk();
    return (staleRecords.length, staleMetas.length);
  }

  // ---- Meta（委托给 BookRepository） ----

  BookMeta metaOf(BookSource source, String path) => _books.metaOf(source, path);

  void updateMeta(BookMeta m) {
    _books.updateMeta(m);
    TagRepository.instance.setBookTags(m.key, m.tags);
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

  List<ReadRecord> recordsByTag(String tag) {
    final result = <ReadRecord>[];
    final seen = <String>{};

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
            title: spath.split('/').last, lastPage: 0, readCount: 0, lastReadAt: 0));
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
            title: spath.split('/').last, lastPage: 0, readCount: 0, lastReadAt: 0));
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
