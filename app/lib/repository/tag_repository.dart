// Repository 层：标签独立存储 + 搜索/补全（ADR-016/017）。
//
// Single Source of Truth:
//   TagRepository — 所有标签增删改查的唯一入口
//   LibraryStore — 向后兼容包装，内部委托给 TagRepository
//
// library.json 格式升级：
//   { "tags": [...], "book_tags": [...] }  // 新增独立字段
//   旧格式（仅 metas 中的 tags 列表）自动检测并迁移。
//
// 所有 UI 层的标签访问（补全、筛选、重命名）全部走 TagRepository，
// 不再遍历 BookMeta.tags。

import 'dart:convert';
import 'dart:io';

import 'package:flutter/foundation.dart';

import '../src/rust/api/db.dart';
import '../store/models.dart';

/// 标签持久化/查询的统一入口（ADR-016/017）。
class TagRepository extends ChangeNotifier {
  TagRepository._();

  static final TagRepository instance = TagRepository._();

  final Map<String, Tag> _tags = {};        // tagId → Tag
  final Set<BookTag> _bookTags = {};        // BookTag 关联集合

  bool _loaded = false;

  // ---- 加载 / 持久化 ----

  /// 从 JSON 加载/合并标签（[force] 用于启动一次性对账：
  /// SQLite 已加载后仍强制合并 JSON 中缺失的标签，不重置现有数据）。
  Future<void> load(File file, {bool force = false}) async {
    if (_loaded && !force) return;
    _loaded = true;
    if (!await file.exists()) return;
    try {
      final j = jsonDecode(await file.readAsString()) as Map<String, dynamic>;

      // 优先从独立标签表加载（保持未关联任何漫画的标签不丢失）
      final tagsJ = (j['tags'] as List?) ?? [];
      for (final t in tagsJ) {
        final tag = Tag.fromJson(Map<String, dynamic>.from(t));
        _tags[tag.id] = tag;
      }
      final bookTagsJ = (j['book_tags'] as List?) ?? [];
      for (final bt in bookTagsJ) {
        _bookTags.add(BookTag.fromJson(Map<String, dynamic>.from(bt)));
      }

      // 然后从 BookMeta 补充/纠正（metas 是 ground truth）
      final metas = (j['metas'] as Map<String, dynamic>?) ?? {};
      for (final entry in metas.entries) {
        final bookKey = entry.key;
        final metaJ = Map<String, dynamic>.from(entry.value);
        // 普通标签
        final tags = (metaJ['tags'] as List?)?.map((e) => '$e').toList() ?? [];
        for (final tagName in tags) {
          if (tagName.isNotEmpty) _addTagAndLink(tagName, bookKey);
        }
        // 元数据标签 (author/genre/series)
        for (final field in ['author', 'genre', 'series']) {
          final val = (metaJ[field] as String?) ?? '';
          if (val.isNotEmpty) _addTagAndLink(val, bookKey);
        }
      }

      // 向后兼容：旧 hash ID → 新 name ID 合并归一化
      _normalizeTagIds();
    } catch (e) {
      debugPrint('[TagRepository] load failed: $e');
    }
    notifyListeners();
  }

  Map<String, dynamic> toJson() => {
    'tags': _tags.values.map((t) => t.toJson()).toList(),
    'book_tags': _bookTags.map((bt) => bt.toJson()).toList(),
  };

  // ---- SQLite 加载 / 持久化（ADR-013） ----

  /// 从 SQLite 加载标签和关联。
  Future<void> loadFromSqlite() async {
    if (_loaded) return;
    _loaded = true;

    // Tags
    final tagDtos = await dbLoadAllTags();
    for (final dto in tagDtos) {
      _tags[dto.id] = Tag(id: dto.id, name: dto.name, createdAt: dto.createdAt.toInt());
    }

    // BookTags
    final btDtos = await dbLoadAllBookTags();
    for (final dto in btDtos) {
      _bookTags.add(BookTag(bookKey: dto.bookKey, tagId: dto.tagId));
    }

    // 归一化：旧版 hash 算法残留的旧 ID 合并到新 DJB2 ID
    _normalizeTagIds();

    notifyListeners();
  }

  /// 将当前标签数据写入 SQLite（增量 upsert）。
  Future<void> saveToSqlite() async {
    // Tags
    for (final t in _tags.values) {
      await dbEnsureTag(name: t.name);
    }
    // BookTags：全量替换（简单但有效；数据量大后可优化为增量）
    // 先收集现有 SQLite 中的所有关联，然后增量同步
    final existingDtos = await dbLoadAllBookTags();
    final existing = <String>{};
    for (final dto in existingDtos) {
      existing.add('${dto.bookKey}\x00${dto.tagId}');
    }
    final current = <String>{};
    for (final bt in _bookTags) {
      current.add('${bt.bookKey}\x00${bt.tagId}');
    }
    // 删除不再存在的
    for (final key in existing) {
      if (!current.contains(key)) {
        final parts = key.split('\x00');
        await dbUnlinkTag(bookKey: parts[0], tagName: _tagNameById(parts[1]));
      }
    }
    // 添加新的
    for (final key in current) {
      if (!existing.contains(key)) {
        final parts = key.split('\x00');
        await dbLinkTag(bookKey: parts[0], tagName: _tagNameById(parts[1]));
      }
    }
  }

  /// 只持久化某本书的标签与关联（翻页热路径用）。
  ///
  /// 与 [saveToSqlite] 的全量 diff 不同，这里不读全表，
  /// 只对目标书做幂等 upsert（`已读` 等关联在 recordRead 后立刻落盘）。
  Future<void> persistBookLinks(String bookKey) async {
    for (final bt in _bookTags) {
      if (bt.bookKey != bookKey) continue;
      final name = _tags[bt.tagId]?.name;
      if (name == null || name.isEmpty) continue;
      await dbEnsureTag(name: name);
      await dbLinkTag(bookKey: bookKey, tagName: name);
    }
  }

  /// 通过 tagId 查找标签名。
  String _tagNameById(String tagId) {
    return _tags[tagId]?.name ?? tagId;
  }

  // ---- 标签 CRUD ----

  /// 所有标签（按名称排序），用于补全列表。
  List<Tag> all() {
    final list = _tags.values.toList();
    list.sort((a, b) => a.name.compareTo(b.name));
    return list;
  }

  /// 所有标签名（向后兼容旧 allTags()）。
  List<String> allNames() => all().map((t) => t.name).toList();

  /// 查找或创建标签（通过名称），返回 tagId。
  /// 不会自动关联到漫画；关联需单独调用 link()。
  String ensure(String name) {
    if (name.isEmpty) return '';
    final id = _tagId(name);
    _tags.putIfAbsent(id, () => Tag(id: id, name: name));
    return id;
  }

  /// 将标签关联到一本书。
  void link(String bookKey, String tagName) {
    if (tagName.isEmpty) return;
    final tagId = ensure(tagName);
    final bt = BookTag(bookKey: bookKey, tagId: tagId);
    if (!_bookTags.contains(bt)) {
      _bookTags.add(bt);
      notifyListeners();
    }
  }

  /// 将标签从一本书移除。
  void unlink(String bookKey, String tagName) {
    final tagId = _tagId(tagName);
    _bookTags.removeWhere((bt) => bt.bookKey == bookKey && bt.tagId == tagId);
    notifyListeners();
  }

  /// 移除某前缀（通常是某书源）下所有漫画的标签关联。
  /// 仅改内存；SQLite 落盘由 [saveToSqlite] 的全量 diff 同步。
  void removeBookTagsByPrefix(String prefix) {
    final before = _bookTags.length;
    _bookTags.removeWhere((bt) => bt.bookKey.startsWith(prefix));
    if (_bookTags.length != before) notifyListeners();
  }

  /// 将一本书的标签关联迁移到新 key（后缀别名归一化后用），合并去重。
  void remapBookKey(String oldKey, String newKey) {
    if (oldKey == newKey) return;
    final moving = _bookTags.where((bt) => bt.bookKey == oldKey).toList();
    if (moving.isEmpty) return;
    _bookTags.removeWhere((bt) => bt.bookKey == oldKey);
    for (final bt in moving) {
      _bookTags.add(BookTag(bookKey: newKey, tagId: bt.tagId));
    }
    notifyListeners();
  }

  /// 设置一本书的标签集（全量替换）。
  void setBookTags(String bookKey, List<String> tagNames) {
    // 移除旧关联
    _bookTags.removeWhere((bt) => bt.bookKey == bookKey);
    // 添加新关联
    for (final name in tagNames) {
      link(bookKey, name);
    }
    notifyListeners();
  }

  /// 获取某本书的标签名列表。
  List<String> tagsForBook(String bookKey) {
    return _bookTags
        .where((bt) => bt.bookKey == bookKey)
        .map((bt) => _tags[bt.tagId]?.name ?? bt.tagId)
        .where((n) => n.isNotEmpty)
        .toList();
  }

  /// 模糊搜索标签（用于补全）。
  List<Tag> search(String query) {
    final q = query.toLowerCase();
    return all().where((t) => t.name.toLowerCase().contains(q)).toList();
  }

  /// 标签重命名（所有关联自动更新）。
  void rename(String oldName, String newName) {
    if (oldName == newName || newName.isEmpty) return;
    final oldId = _tagId(oldName);
    final tag = _tags.remove(oldId);
    if (tag == null) return;
    // 创建新标签
    final newId = _tagId(newName);
    _tags[newId] = Tag(id: newId, name: newName);
    // 迁移所有关联
    final affected = _bookTags.where((bt) => bt.tagId == oldId).toList();
    for (final bt in affected) {
      _bookTags.remove(bt);
      _bookTags.add(BookTag(bookKey: bt.bookKey, tagId: newId));
    }
    notifyListeners();
  }

  /// 删除标签及所有关联。
  void delete(String name) {
    final id = _tagId(name);
    _tags.remove(id);
    _bookTags.removeWhere((bt) => bt.tagId == id);
    notifyListeners();
  }

  /// 标签统计：每个标签关联的漫画数。
  Map<String, int> tagStats() {
    final map = <String, int>{};
    for (final bt in _bookTags) {
      final name = _tags[bt.tagId]?.name ?? bt.tagId;
      map[name] = (map[name] ?? 0) + 1;
    }
    return map;
  }

  /// 获取某标签下的所有 bookKey。
  List<String> bookKeysForTag(String tagName) {
    final id = _tagId(tagName);
    return _bookTags.where((bt) => bt.tagId == id).map((bt) => bt.bookKey).toList();
  }

  // ---- internal ----

  /// 回填时用的辅助：创建标签 + 建立关联。
  void _addTagAndLink(String name, String bookKey) {
    final tagId = _tagId(name);
    _tags.putIfAbsent(tagId, () => Tag(id: tagId, name: name));
    _bookTags.add(BookTag(bookKey: bookKey, tagId: tagId));
  }

  /// 迁移 / 加载后归一化：旧版 hash 算法残留的旧 ID 合并到新 DJB2 ID。
  ///
  /// 背景：v0.1 DVD 使用 `hashCode → base36` 生成 tag ID，v0.2 改用 DJB2，
  /// 直接迁移过来的旧 ID 会导致 `bookKeysForTag()` 用新 ID 查不到旧关联。
  ///
  /// 算法：遍历所有标签，对每个标签名重算新 ID。若新旧 ID 不同，
  /// 则将旧 ID 下的 Tag 和 BookTag 关联全部合并到新 ID。
  void _normalizeTagIds() {
    final renames = <String, String>{}; // oldId → newId
    for (final t in _tags.values.toList()) {
      final newId = _tagId(t.name);
      if (t.id != newId) {
        renames[t.id] = newId;
      }
    }
    if (renames.isEmpty) return;

    // 合并 Tag 实体
    for (final entry in renames.entries) {
      final oldId = entry.key;
      final newId = entry.value;
      // 保留旧 tag 的 createdAt（如果有的话），新 tag 用更早的时间
      final oldTag = _tags.remove(oldId);
      final existing = _tags[newId];
      if (existing != null && oldTag != null) {
        // 两个都存在：保留更早的 createdAt
        if (oldTag.createdAt < existing.createdAt) {
          _tags[newId] = Tag(id: newId, name: existing.name, createdAt: oldTag.createdAt);
        }
      } else if (oldTag != null) {
        _tags[newId] = Tag(id: newId, name: oldTag.name, createdAt: oldTag.createdAt);
      }
      // 迁移 BookTag 关联
      final affected = _bookTags.where((bt) => bt.tagId == oldId).toList();
      for (final bt in affected) {
        _bookTags.remove(bt);
        _bookTags.add(BookTag(bookKey: bt.bookKey, tagId: newId));
      }
    }
  }

  /// 标签名即 ID — trim 后小写，与 Rust `db::tag_id()` 完全一致。
  static String _tagId(String name) => name.trim().toLowerCase();
}
