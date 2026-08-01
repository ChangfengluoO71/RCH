import 'dart:convert';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:path_provider/path_provider.dart';

import '../repository/book_repository.dart';
import '../repository/record_repository.dart';
import '../repository/tag_repository.dart';
import '../src/rust/api/db.dart';
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

  Future<void> load() async {
    if (_loaded) return;
    try {
      final migrated = await dataIsMigrated();
      if (migrated) {
        await _loadFromSqlite();
        _loaded = true;
        notifyListeners();
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
    _save();
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

  /// 公开的持久化入口：强制全量写入 SQLite + JSON 备份。
  Future<void> saveToDisk() async => _save();

  Future<void> _save() async {
    try {
      await _saveToSqlite();
    } catch (e, st) {
      debugPrint('[LibraryStore] SQLite save failed: $e');
      debugPrintStack(stackTrace: st);
    }
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
      debugPrint('Library JSON save failed: $e');
      debugPrintStack(stackTrace: st);
      rethrow;
    }
  }

  Future<void> _saveToSqlite() async {
    await _books.saveToSqlite();
    await _records.saveToSqlite();
    await TagRepository.instance.saveToSqlite();
    final settingsJson = settings.toJson();
    for (final entry in settingsJson.entries) {
      await dbSaveSetting(key: entry.key, value: entry.value.toString());
    }
  }

  // ---- Source（委托给 BookRepository） ----

  void addSource(BookSource s) {
    _books.addSource(s);
    notifyListeners(); _save();
  }

  void removeSource(String id) {
    _books.removeSource(id);
    notifyListeners(); _save();
  }

  void updateSource(String id, {String? name, String? url, String? username, String? password, String? path, String? note}) {
    _books.updateSource(id, name: name, url: url, username: username, password: password, path: path, note: note);
    notifyListeners(); _save();
  }

  void updateSourceCapability(String id, String label) {
    _books.updateSourceCapability(id, label);
    notifyListeners(); _save();
  }

  BookSource? sourceById(String id) => _books.sourceById(id);

  void removeSourceWithCleanup(String id) {
    final src = sourceById(id);
    _books.removeSource(id);
    if (src != null) {
      final prefix = '${src.type}|${src.id}|';
      _records.removeByPrefix(prefix);
      _books.metas.removeWhere((k, _) => k.startsWith(prefix));
    }
    notifyListeners(); _save();
  }

  // ---- Record（委托给 RecordRepository） ----

  void recordRead({
    required BookSource source,
    required String path,
    required String title,
    int? page,
  }) {
    final r = _records.upsert(source: source, path: path, title: title, page: page);
    TagRepository.instance.link(RecordRepository.keyOf(source.type, source.id, path), '已读');
    notifyListeners();
    _records.saveOneToSqlite(r);
    _saveJsonBackup();
  }

  ReadRecord? recordOf(BookSource source, String path) => _records.of(source, path);

  bool hasAnyRead() => _records.hasAnyRead();

  List<ReadRecord> get recent => _records.recent();
  List<ReadRecord> get mostRead => _records.mostRead();

  int purgeStaleRecords() {
    final removed = _records.purgeStale(sources);
    if (removed > 0) {
      notifyListeners(); _save();
    }
    return removed;
  }

  Future<void> _saveJsonBackup() async {
    try {
      final f = await _file();
      final data = {
        ..._books.toJson(),
        ..._records.toJson(),
        'settings': settings.toJson(),
        ...TagRepository.instance.toJson(),
      };
      await f.writeAsString(jsonEncode(data));
    } catch (e) {
      debugPrint('[LibraryStore] JSON backup failed: $e');
    }
  }

  // ---- Meta（委托给 BookRepository） ----

  BookMeta metaOf(BookSource source, String path) => _books.metaOf(source, path);

  void updateMeta(BookMeta m) {
    _books.updateMeta(m);
    TagRepository.instance.setBookTags(m.key, m.tags);
    for (final mt in m.metaTags) {
      if (mt.isNotEmpty) TagRepository.instance.link(m.key, mt);
    }
    notifyListeners(); _save();
  }

  // ---- 设置 ----

  void updateSettings(AppSettings s) {
    settings = s;
    notifyListeners(); _save();
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
      final bookKey = '${r.sourceType}|${r.sourceId}|${r.path}';
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
    notifyListeners(); _save();
  }

  void deleteTag(String name) {
    for (final m in metas.values) {
      m.tags.remove(name);
      if (m.author == name) m.author = '';
      if (m.genre == name)  m.genre = '';
      if (m.series == name) m.series = '';
    }
    TagRepository.instance.delete(name);
    notifyListeners(); _save();
  }

  // ---- 跨书源搜索 ----

  List<({String bookKey, BookSource source, String path, String title})> globalSearch({
    String text = '',
    Set<String> tags = const {},
  }) {
    final results = <({String bookKey, BookSource source, String path, String title})>[];
    final seen = <String>{};
    for (final s in sources) {
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
        final key = '${src.type}|${src.id}|$p';
        TagRepository.instance.link(key, '已读');
      }
      notifyListeners(); _save();
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
      final key = '${src.type}|${src.id}|$p';
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
    notifyListeners(); _save();
  }
}
