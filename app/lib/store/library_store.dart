import 'dart:convert';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:path_provider/path_provider.dart';

import '../repository/tag_repository.dart';
import '../src/rust/api/db.dart';
import 'models.dart';

/// 应用数据存储：书源 + 阅读记录 + 元数据 + 设置 + 标签。
/// ChangeNotifier：数据变化时通知 UI 重建。
///
/// SQLite 数据层（ADR-013）：启动时优先从 database.db 加载，
/// 若 SQLite 为空则回退到 library.json。
/// 每次变更时写入 SQLite（同步），library.json 也同步备份。
///
/// ADR-016/017：标签相关操作全部委托给 TagRepository（Single Source of Truth）。
class LibraryStore extends ChangeNotifier {
  LibraryStore._();
  static final LibraryStore instance = LibraryStore._();

  final List<BookSource> sources = [];
  final Map<String, ReadRecord> records = {};
  final Map<String, BookMeta> metas = {};
  AppSettings settings = AppSettings();
  bool _loaded = false;

  /// 标签仓库（ADR-016/017）— 所有标签操作的唯一入口。
  TagRepository get tags => TagRepository.instance;

  Future<void> load() async {
    if (_loaded) return;
    try {
      // 尝试从 SQLite 加载
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
        // 先加载 TagRepository（独立标签表）
        await TagRepository.instance.load(f);

        final j = jsonDecode(await f.readAsString()) as Map<String, dynamic>;
        sources
          ..clear()
          ..addAll((j['sources'] as List? ?? [])
              .map((e) => BookSource.fromJson(Map<String, dynamic>.from(e))));
        records
          ..clear()
          ..addEntries((j['records'] as Map? ?? {}).entries.map((e) =>
              MapEntry(e.key, ReadRecord.fromJson(Map<String, dynamic>.from(e.value)))));
        metas
          ..clear()
          ..addEntries((j['metas'] as Map? ?? {}).entries.map((e) =>
              MapEntry(e.key, BookMeta.fromJson(Map<String, dynamic>.from(e.value)))));
        if (j['settings'] != null) {
          settings = AppSettings.fromJson(Map<String, dynamic>.from(j['settings']));
        }
      }
    } catch (e) {
      debugPrint('[LibraryStore] load JSON failed: $e');
    }
    _loaded = true;
    notifyListeners();
    // 将纠正后的标签数据立即写回 JSON（解决旧版本残留碎片标签问题）
    _save();
  }

  /// 从 SQLite 加载全量数据。
  Future<void> _loadFromSqlite() async {
    // Sources
    final srcDtos = await dbLoadAllSources();
    for (final dto in srcDtos) {
      sources.add(BookSource(
        id: dto.id,
        type: dto.type,
        name: dto.name,
        path: dto.path,
        url: dto.url,
        username: dto.username,
        password: dto.password,
        note: dto.note,
        capabilityLabel: dto.capabilityLabel,
      ));
    }

    // Records
    final recDtos = await dbLoadAllRecords();
    for (final dto in recDtos) {
      records[dto.key] = ReadRecord(
        key: dto.key,
        sourceId: dto.sourceId,
        sourceType: dto.sourceType,
        path: dto.path,
        title: dto.title,
        lastPage: dto.lastPage,
        readCount: dto.readCount,
        lastReadAt: dto.lastReadAt.toInt(),
      );
    }

    // Metas
    final metaDtos = await dbLoadAllMetas();
    for (final dto in metaDtos) {
      metas[dto.key] = BookMeta(
        key: dto.key,
        coverPage: dto.coverPage,
        cropX: dto.cropX,
        cropY: dto.cropY,
        cropW: dto.cropW,
        cropH: dto.cropH,
        author: dto.author,
        genre: dto.genre,
        series: dto.series,
        title: dto.title,
        chineseTitle: dto.chineseTitle,
        summary: dto.summary,
        comment: dto.comment,
      );
    }

    // Tags + BookTags
    await TagRepository.instance.loadFromSqlite();

    // Settings
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
    // 先尝试 jsonDecode（数字、布尔、数组、对象）
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
    return raw; // 纯字符串
  }

  /// 返回 library.json 的完整路径（供迁移等使用）。
  Future<String> filePath() async {
    final dir = await getApplicationSupportDirectory();
    return '${dir.path}${Platform.pathSeparator}library.json';
  }

  Future<File> _file() async {
    return File(await filePath());
  }

  /// 公开的持久化入口：强制全量写入 SQLite + JSON 备份。
  /// 供外部组件（如 book_detail_page）在直接操作 TagRepository 后调用。
  Future<void> saveToDisk() async => _save();

  /// 持久化：双写 SQLite（优先） + library.json（备份）。
  ///
  /// 每次变更时同步写入 SQLite，library.json 仅作备份（防 SQLite 损坏）。
  /// 若 SQLite 写入失败，仍继续写 JSON（不丢数据）。
  Future<void> _save() async {
    // 1) 写入 SQLite（新主存储）
    try {
      await _saveToSqlite();
    } catch (e, st) {
      debugPrint('[LibraryStore] SQLite save failed: $e');
      debugPrintStack(stackTrace: st);
      // 不抛异常，继续写 JSON 备份
    }

    // 2) 写入 JSON（备份）
    try {
      final f = await _file();
      final data = {
        'sources': sources.map((e) => e.toJson()).toList(),
        'records': records.map((k, v) => MapEntry(k, v.toJson())),
        'metas': metas.map((k, v) => MapEntry(k, v.toJson())),
        'settings': settings.toJson(),
        // ADR-017: 独立标签表
        ...TagRepository.instance.toJson(),
      };
      await f.writeAsString(jsonEncode(data));
    } catch (e, st) {
      debugPrint('Library JSON save failed: $e');
      debugPrintStack(stackTrace: st);
      rethrow;
    }
  }

  /// 将当前全量数据写入 SQLite（增量 upsert）。
  Future<void> _saveToSqlite() async {
    // Sources
    for (final s in sources) {
      await dbUpsertSource(source: BookSourceDto(
        id: s.id,
        type: s.type,
        name: s.name,
        path: s.path,
        url: s.url,
        username: s.username,
        password: s.password,
        note: s.note,
        capabilityLabel: s.capabilityLabel,
      ));
    }

    // Records
    for (final r in records.values) {
      await dbUpsertRecord(record: ReadRecordDto(
        key: r.key,
        sourceId: r.sourceId,
        sourceType: r.sourceType,
        path: r.path,
        title: r.title,
        lastPage: r.lastPage,
        readCount: r.readCount,
        lastReadAt: r.lastReadAt,
      ));
    }

    // Metas
    for (final m in metas.values) {
      await dbUpsertMeta(meta: BookMetaDto(
        key: m.key,
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
      ));
    }

    // Tags + BookTags
    await TagRepository.instance.saveToSqlite();

    // Settings（每个 key 一行）
    final settingsJson = settings.toJson();
    for (final entry in settingsJson.entries) {
      final v = entry.value.toString();
      await dbSaveSetting(key: entry.key, value: v);
    }
  }

  void addSource(BookSource s) {
    sources.add(s);
    notifyListeners();
    _save();
  }

  void removeSource(String id) {
    sources.removeWhere((s) => s.id == id);
    notifyListeners();
    _save();
  }

  /// 记录一次阅读:page 为 null 表示"打开"(readCount+1),否则更新进度。
  void recordRead({
    required BookSource source,
    required String path,
    required String title,
    int? page,
  }) {
    final key = '${source.type}|${source.id}|$path';
    final r = records[key] ??
        ReadRecord(
          key: key,
          sourceId: source.id,
          sourceType: source.type,
          path: path,
          title: title,
        );
    r.lastReadAt = DateTime.now().millisecondsSinceEpoch;
    if (page == null) {
      r.readCount++;
    } else {
      r.lastPage = page;
    }
    records[key] = r;

    // 自动添加"已读"元数据标签（每本打开过的漫画自动标记）
    TagRepository.instance.link(key, '已读');

    notifyListeners();
    // 直接写入 SQLite（阅读记录高频更新，不做全量 _save）
    _saveRecordToSqlite(r);
    _saveJsonBackup();
  }

  /// 单条记录写入 SQLite（高频更新时避免全量同步）。
  Future<void> _saveRecordToSqlite(ReadRecord r) async {
    try {
      await dbUpsertRecord(record: ReadRecordDto(
        key: r.key,
        sourceId: r.sourceId,
        sourceType: r.sourceType,
        path: r.path,
        title: r.title,
        lastPage: r.lastPage,
        readCount: r.readCount,
        lastReadAt: r.lastReadAt,
      ));
    } catch (e) {
      debugPrint('[LibraryStore] dbUpsertRecord failed: $e');
    }
  }

  /// 仅写 JSON 备份（不含全量 SQLite 同步）。
  Future<void> _saveJsonBackup() async {
    try {
      final f = await _file();
      final data = {
        'sources': sources.map((e) => e.toJson()).toList(),
        'records': records.map((k, v) => MapEntry(k, v.toJson())),
        'metas': metas.map((k, v) => MapEntry(k, v.toJson())),
        'settings': settings.toJson(),
        ...TagRepository.instance.toJson(),
      };
      await f.writeAsString(jsonEncode(data));
    } catch (e) {
      debugPrint('[LibraryStore] JSON backup failed: $e');
    }
  }

  ReadRecord? recordOf(BookSource source, String path) =>
      records['${source.type}|${source.id}|$path'];

  /// 获取(或创建)一本书的元数据。
  BookMeta metaOf(BookSource source, String path) {
    final key = '${source.type}|${source.id}|$path';
    return metas.putIfAbsent(key, () => BookMeta(key: key));
  }

  /// 更新一本书的元数据并持久化。
  /// ADR-016/017: 同时将 BookMeta 的 tags + 元数据标签(author/genre/series)同步到 TagRepository。
  void updateMeta(BookMeta m) {
    metas[m.key] = m;
    // 同步普通标签
    TagRepository.instance.setBookTags(m.key, m.tags);
    // 同步元数据标签（确保 author/genre/series 作为独立标签存在于补全列表）
    for (final mt in m.metaTags) {
      if (mt.isNotEmpty) TagRepository.instance.link(m.key, mt);
    }
    notifyListeners();
    _save();
  }

  /// 更新书源的能力标记。
  void updateSourceCapability(String id, String label) {
    for (final s in sources) {
      if (s.id == id) {
        s.capabilityLabel = label;
      }
    }
    notifyListeners();
    _save();
  }

  /// 更新书源的基本信息。
  void updateSource(String id, {String? name, String? url, String? username, String? password, String? path, String? note}) {
    for (final s in sources) {
      if (s.id == id) {
        if (name != null) s.name = name;
        if (url != null) s.url = url;
        if (username != null) s.username = username;
        if (password != null) s.password = password;
        if (path != null) s.path = path;
        if (note != null) s.note = note;
      }
    }
    notifyListeners(); _save();
  }

  /// 更新书源的备注。
  void removeSourceWithCleanup(String id) {
    final src = sources.cast<BookSource?>().firstWhere((s) => s?.id == id, orElse: () => null);
    sources.removeWhere((s) => s.id == id);
    if (src != null) {
      final prefix = '${src.type}|${src.id}|';
      records.removeWhere((k, _) => k.startsWith(prefix));
      metas.removeWhere((k, _) => k.startsWith(prefix));
    }
    notifyListeners(); _save();
  }

  void updateSettings(AppSettings s) {
    settings = s;
    notifyListeners();
    _save();
  }

  /// 收集所有被用作元数据(作者/类别/系列/已读)的标签名。
  Set<String> metaTagNames() {
    final set = <String>{
      if (hasAnyRead()) '已读',
    };
    for (final m in metas.values) {
      if (m.author.isNotEmpty) set.add(m.author);
      if (m.genre.isNotEmpty) set.add(m.genre);
      if (m.series.isNotEmpty) set.add(m.series);
    }
    return set;
  }

  /// 是否有任何漫画被打开过。
  bool hasAnyRead() => records.values.any((r) => r.readCount > 0);

  /// 跨书源搜索：遍历所有书源下的所有 metas，不限于已打开过的漫画。
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

  /// 元数据栏标签名列表（含"已读"）。
  List<String> get metaFields => ['author', 'genre', 'series', '已读'];

  /// 批量打标签（ADR-016/017: 同步到 TagRepository）。
  /// "已读"作为元数据标签，直接走 TagRepository.link。
  void batchTag(BookSource src, Iterable<String> paths, String tag) {
    // "已读"是元数据标签，不走 BookMeta.tags
    if (tag == '已读') {
      for (final p in paths) {
        final key = '${src.type}|${src.id}|$p';
        TagRepository.instance.link(key, '已读');
      }
      notifyListeners();
      _save();
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
    notifyListeners();
    _save();
  }

  int purgeStaleRecords() {
    int removed = 0;
    final staleKeys = <String>[];
    for (final r in records.values) {
      final src = sourceById(r.sourceId);
      if (src == null) {
        staleKeys.add(r.key);
      } else if (!src.isWebDav) {
        if (!File(r.path).existsSync()) {
          staleKeys.add(r.key);
        }
      }
    }
    for (final k in staleKeys) {
      records.remove(k);
      metas.remove(k);
      removed++;
    }
    if (removed > 0) {
      notifyListeners();
      _save();
    }
    return removed;
  }

  // ============================================================
  // ADR-016/017: 标签相关方法全部委托给 TagRepository
  // ============================================================

  /// 所有标签名（补全列表来源）— 从 TagRepository 取，不再遍历 BookMeta。
  List<String> allTags() => TagRepository.instance.allNames();

  /// 标签统计 — 从 TagRepository 取独立标签表 + 合并阅读记录。
  List<(String, int, int)> tagStats() {
    final tagMap = TagRepository.instance.tagStats(); // tagName → count
    final map = <String, (int, int)>{};
    for (final entry in tagMap.entries) {
      map[entry.key] = (entry.value, 0);
    }
    // 补充阅读次数
    for (final r in records.values) {
      final bookKey = '${r.sourceType}|${r.sourceId}|${r.path}';
      final bookTags = TagRepository.instance.tagsForBook(bookKey);
      for (final t in bookTags) {
        final prev = map[t] ?? (0, 0);
        map[t] = (prev.$1, prev.$2 + r.readCount);
      }
    }
    // 补充元数据标签
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

/// 获取某标签下的所有漫画记录。
/// 同时检查 TagRepository 的 book_tag 关联和 metas 中的元数据标签。
List<ReadRecord> recordsByTag(String tag) {
    final result = <ReadRecord>[];
    final seen = <String>{};

    // 1. TagRepository 中的 book_tag 关联
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

    // 2. metas 中的元数据标签（author/genre/series）
    //    与已读记录的 key 格式不同：metas key = "type|sourceId|path"，records key 一致
    //    修复：有 records 的加进去，没 records 的也合成一条 ReadRecord
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

  /// 重命名标签（ADR-017: TagRepository 一次操作，所有关联自动更新）。
  void renameTag(String oldName, String newName) {
    if (newName.isEmpty || oldName == newName) return;
    // 同步更新 BookMeta 中的 tags
    for (final m in metas.values) {
      if (m.tags.contains(oldName)) { m.tags.remove(oldName); m.tags.add(newName); }
      if (m.author == oldName) { m.author = newName; }
      if (m.genre == oldName)  { m.genre = newName; }
      if (m.series == oldName) { m.series = newName; }
    }
    // TagRepository 层面重命名
    TagRepository.instance.rename(oldName, newName);
    notifyListeners(); _save();
  }

  /// 删除标签（ADR-017）。
  void deleteTag(String name) {
    for (final m in metas.values) {
      m.tags.remove(name);
      if (m.author == name) m.author = '';
      if (m.genre == name)  m.genre = '';
      if (m.series == name) m.series = '';
    }
    TagRepository.instance.delete(name);
    notifyListeners();
    _save();
  }

  /// 最近阅读:按最后阅读时间倒序。
  List<ReadRecord> get recent =>
      records.values.toList()..sort((a, b) => b.lastReadAt.compareTo(a.lastReadAt));

  /// 最多阅读:按打开次数倒序。
  List<ReadRecord> get mostRead =>
      records.values.toList()..sort((a, b) => b.readCount.compareTo(a.readCount));

  BookSource? sourceById(String id) {
    for (final s in sources) {
      if (s.id == id) return s;
    }
    return null;
  }
}
