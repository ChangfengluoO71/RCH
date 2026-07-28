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

import '../store/models.dart';

/// 标签持久化/查询的统一入口（ADR-016/017）。
class TagRepository extends ChangeNotifier {
  TagRepository._();

  static final TagRepository instance = TagRepository._();

  final Map<String, Tag> _tags = {};        // tagId → Tag
  final Set<BookTag> _bookTags = {};        // BookTag 关联集合

  bool _loaded = false;

  // ---- 加载 / 持久化 ----

  Future<void> load(File file) async {
    if (_loaded) return;
    _loaded = true;
    if (!await file.exists()) return;
    try {
      final j = jsonDecode(await file.readAsString()) as Map<String, dynamic>;

      // 从 BookMeta 重建标签数据（metas 是 ground truth，tags/book_tags 只是缓存）
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
    } catch (e) {
      print('[TagRepository] load failed: $e');
    }
    notifyListeners();
  }

  Map<String, dynamic> toJson() => {
    'tags': _tags.values.map((t) => t.toJson()).toList(),
    'book_tags': _bookTags.map((bt) => bt.toJson()).toList(),
  };

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

  static String _tagId(String name) {
    // 用名称的小写 stable hash 作为 id
    return name.toLowerCase().hashCode.toRadixString(36);
  }
}
