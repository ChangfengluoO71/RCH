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

  final Map<String, Tag> _tags = {}; // tagId → Tag
  final Set<BookTag> _bookTags = {}; // BookTag 关联集合

  bool _loaded = false;
  int _revision = 0;

  /// Monotonic projection revision used by UI projections to avoid rebuilding
  /// tag-detail futures for unrelated Flutter frames.
  int get revision => _revision;

  @override
  void notifyListeners() {
    _revision++;
    super.notifyListeners();
  }

  /// Whether a tag belongs in the user-facing tag manager. Resource semantics
  /// with stable user value (for example `Chinese` and `无修正`) remain visible;
  /// Obsolete delivery markers such as `数字版` stay hidden until the startup
  /// migration removes them.  Stable quality semantics such as `高清` remain
  /// visible. A same-named metadata value wins over the generated-name rule.
  static bool isVisibleInTagManager(
    String name, {
    Set<String> metadataNames = const <String>{},
  }) {
    final trimmed = name.trim();
    if (trimmed.isEmpty || metadataNames.contains(trimmed)) return true;
    final lower = trimmed.toLowerCase();
    if (lower.startsWith('resource:') ||
        lower.startsWith('sequence:') ||
        lower.startsWith('publication:') ||
        lower.startsWith('release:') ||
        lower.startsWith('release-group:') ||
        lower.startsWith('release_group:') ||
        lower.startsWith('provider:') ||
        lower.startsWith('source:') ||
        lower.startsWith('language:') ||
        lower.startsWith('translation:') ||
        lower.startsWith('translation-method:') ||
        lower.startsWith('translation_method:') ||
        lower.startsWith('edition:') ||
        lower.startsWith('censorship:') ||
        lower.startsWith('color:') ||
        lower.startsWith('completeness:') ||
        lower.startsWith('medium:') ||
        lower.startsWith('scan:') ||
        lower.startsWith('tag:') ||
        lower.startsWith('汉化组：') ||
        lower.startsWith('汉化组:')) {
      return false;
    }
    return !_hiddenGeneratedTagNames.contains(lower);
  }

  static const Set<String> _hiddenGeneratedTagNames = <String>{
    '中文',
    '英文',
    '日文',
    '中文翻译',
    '未翻译',
    '机翻',
    '人工翻译',
    '有修正',
    '彩漫',
    '彩页',
    '黑白漫',
    '合集',
    '未完结',
    '数字版',
    '实体版',
    '全本扫描',
    '无广告',
    '单行本',
    '连载',
    // English/namespaced values can exist briefly before startup migration.
    'translated',
    'untranslated',
    'machine',
    'digital',
    'dl',
    'ebook',
    'translation',
    'chinese_translation',
    'machine_translation',
    'human_translation',
    'unmodified',
    'modified',
    'full_scan',
    'no_ads',
    'monochrome',
    'collection',
    'completed',
    'partial',
    'uncensored',
    'censored',
    'full_color',
    'color_pages',
    'complete',
    'incomplete',
  };

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
        final parsed = BookTag.fromJson(Map<String, dynamic>.from(bt));
        _bookTags.add(
          BookTag(
            bookKey: normalizePersistedBookKey(parsed.bookKey),
            tagId: parsed.tagId,
          ),
        );
      }

      // 然后从 BookMeta 补充/纠正（metas 是 ground truth）
      final metas = (j['metas'] as Map<String, dynamic>?) ?? {};
      for (final entry in metas.entries) {
        final bookKey = normalizePersistedBookKey(entry.key);
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
    // JSON export is synchronous, but taking snapshots keeps the contract
    // explicit and avoids exposing live collection iterators to encoders.
    'tags': _tags.values
        .toList(growable: false)
        .map((t) => t.toJson())
        .toList(),
    'book_tags': _bookTags
        .toList(growable: false)
        .map((bt) => bt.toJson())
        .toList(),
  };

  // ---- SQLite 加载 / 持久化（ADR-013） ----

  /// 从 SQLite 加载标签和关联。
  Future<void> loadFromSqlite({bool force = false}) async {
    if (_loaded && !force) return;
    if (force) {
      _tags.clear();
      _bookTags.clear();
    }
    _loaded = true;

    // Tags
    final tagDtos = await dbLoadAllTags();
    for (final dto in tagDtos) {
      _tags[dto.id] = Tag(
        id: dto.id,
        name: dto.name,
        createdAt: dto.createdAt.toInt(),
      );
    }

    // BookTags
    final btDtos = await dbLoadAllBookTags();
    for (final dto in btDtos) {
      _bookTags.add(
        BookTag(
          bookKey: normalizePersistedBookKey(dto.bookKey),
          tagId: dto.tagId,
        ),
      );
    }

    // 归一化：旧版 hash 算法残留的旧 ID 合并到新 DJB2 ID
    _normalizeTagIds();

    notifyListeners();
  }

  /// Rebuild the tag links that are derived from canonical metadata fields.
  ///
  /// The tag table is an independent projection, so loading `book_metas`
  /// alone is not enough to make author/genre/series tags queryable. This
  /// method is additive: manual/resource tags are preserved, while every
  /// current metadata value gets a stable link under the normalized book key.
  /// Newly created links are persisted when [persist] is true.
  Future<void> syncMetadataLinks(
    Iterable<BookMeta> metadata, {
    bool persist = true,
  }) async {
    final rows = <({String bookKey, String tagName})>{};
    for (final meta in metadata.toList(growable: false)) {
      final bookKey = normalizePersistedBookKey(meta.key);
      for (final tagName in meta.metaTags) {
        final trimmed = tagName.trim();
        if (trimmed.isNotEmpty) {
          rows.add((bookKey: bookKey, tagName: trimmed));
        }
      }
    }

    var changed = false;
    for (final row in rows) {
      final tagId = ensure(row.tagName);
      final added = _bookTags.add(BookTag(bookKey: row.bookKey, tagId: tagId));
      changed = changed || added;
      if (persist && added) {
        await dbEnsureTag(name: row.tagName);
        await dbLinkTag(bookKey: row.bookKey, tagName: row.tagName);
      }
    }
    if (changed) notifyListeners();
  }

  /// 将旧版刮削标签迁移到面向用户的中文语义标签。
  ///
  /// 解析器的内部字段仍保存在 proposal/provenance 中；标签表只保留
  /// 对用户有稳定含义的资源属性、合集状态和汉化组。迁移是幂等的，
  /// 且逐条使用数据库的幂等 link/delete 接口，不触碰远程书源。
  Future<void> normalizeGeneratedTags({bool persist = true}) async {
    final moves = <({String oldName, String? newName})>[];
    for (final tag in _tags.values.toList()) {
      final mapped = _canonicalGeneratedTag(tag.name);
      if (mapped == null || mapped == tag.name) continue;
      moves.add((oldName: tag.name, newName: mapped.isEmpty ? null : mapped));
    }
    if (moves.isEmpty) return;

    for (final move in moves) {
      final oldId = _tagId(move.oldName);
      final links = _bookTags.where((bt) => bt.tagId == oldId).toList();
      if (move.newName != null && move.newName!.isNotEmpty) {
        final newId = ensure(move.newName!);
        for (final link in links) {
          _bookTags.add(BookTag(bookKey: link.bookKey, tagId: newId));
          if (persist) {
            await dbLinkTag(bookKey: link.bookKey, tagName: move.newName!);
          }
        }
      }
      _bookTags.removeWhere((bt) => bt.tagId == oldId);
      _tags.remove(oldId);
      if (persist) await dbDeleteTag(name: move.oldName);
    }
    notifyListeners();
  }

  /// 将当前标签数据写入 SQLite（增量 upsert）。
  Future<void> saveToSqlite() async {
    // Every DB operation awaits. Snapshot all mutable collections first so
    // catalog reloads or UI tag edits cannot invalidate an active iterator.
    final tagSnapshot = _tags.values.toList(growable: false);
    final bookTagSnapshot = _bookTags.toList(growable: false);
    final tagNames = <String, String>{
      for (final tag in tagSnapshot) tag.id: tag.name,
    };
    // Tags
    for (final t in tagSnapshot) {
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
    for (final bt in bookTagSnapshot) {
      current.add('${bt.bookKey}\x00${bt.tagId}');
    }
    // 删除不再存在的
    for (final key in existing) {
      if (!current.contains(key)) {
        final parts = key.split('\x00');
        await dbUnlinkTag(
          bookKey: parts[0],
          tagName: tagNames[parts[1]] ?? parts[1],
        );
      }
    }
    // 添加新的
    for (final key in current) {
      if (!existing.contains(key)) {
        final parts = key.split('\x00');
        await dbLinkTag(
          bookKey: parts[0],
          tagName: tagNames[parts[1]] ?? _tagNameById(parts[1]),
        );
      }
    }
  }

  /// 只持久化某本书的标签与关联（翻页热路径用）。
  ///
  /// 与 [saveToSqlite] 的全量 diff 不同，这里不读全表，
  /// 只对目标书做幂等 upsert（`已读` 等关联在 recordRead 后立刻落盘）。
  Future<void> persistBookLinks(String bookKey) async {
    bookKey = normalizePersistedBookKey(bookKey);
    final links = _bookTags
        .where((bt) => bt.bookKey == bookKey)
        .toList(growable: false);
    for (final bt in links) {
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
    bookKey = normalizePersistedBookKey(bookKey);
    final tagId = ensure(tagName);
    final bt = BookTag(bookKey: bookKey, tagId: tagId);
    if (!_bookTags.contains(bt)) {
      _bookTags.add(bt);
      notifyListeners();
    }
  }

  /// 将标签从一本书移除。
  void unlink(String bookKey, String tagName) {
    bookKey = normalizePersistedBookKey(bookKey);
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
    oldKey = normalizePersistedBookKey(oldKey);
    newKey = normalizePersistedBookKey(newKey);
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
    bookKey = normalizePersistedBookKey(bookKey);
    // 移除旧关联
    _bookTags.removeWhere((bt) => bt.bookKey == bookKey);
    // 添加新关联
    for (final name in tagNames) {
      link(bookKey, name);
    }
    notifyListeners();
  }

  /// Remove all links for one stale book and delete tag entities that no longer
  /// have any live book link. The DB delete is intentionally explicit: a later
  /// full tag snapshot must not re-create an orphan tag with `dbEnsureTag`.
  ///
  /// [persist] is false only for in-memory/unit-test projections; production
  /// cleanup keeps it true and the caller still persists the remaining links.
  Future<void> removeBookTagsAndPrune(String bookKey, {bool persist = true}) =>
      _removeLinksAndPrune(
        (bt) => bt.bookKey == normalizePersistedBookKey(bookKey),
        persist: persist,
      );

  /// Source deletion variant of [removeBookTagsAndPrune].
  Future<void> removeBookTagsByPrefixAndPrune(
    String prefix, {
    bool persist = true,
  }) => _removeLinksAndPrune(
    (bt) => bt.bookKey.startsWith(prefix),
    persist: persist,
  );

  Future<void> _removeLinksAndPrune(
    bool Function(BookTag) matches, {
    required bool persist,
  }) async {
    final removed = _bookTags.where(matches).toList();
    if (removed.isEmpty) return;
    final candidateIds = removed.map((bt) => bt.tagId).toSet();
    _bookTags.removeWhere(matches);
    final linkedIds = _bookTags.map((bt) => bt.tagId).toSet();
    final orphanIds = candidateIds.difference(linkedIds);
    final orphanNames = <String>[];
    for (final id in orphanIds) {
      final tag = _tags.remove(id);
      if (tag != null && tag.name.trim().isNotEmpty) {
        orphanNames.add(tag.name);
      }
    }
    if (persist) {
      for (final name in orphanNames) {
        await dbDeleteTag(name: name);
      }
    }
    notifyListeners();
  }

  /// 获取某本书的标签名列表。
  List<String> tagsForBook(String bookKey) {
    bookKey = normalizePersistedBookKey(bookKey);
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

  /// Count read totals by tag in one pass over book-tag links. This avoids
  /// repeatedly scanning all links once per record when the tag manager
  /// rebuilds after a notification.
  Map<String, int> readCountsByTag(Map<String, int> readCountsByBookKey) {
    final map = <String, int>{};
    for (final bt in _bookTags) {
      final name = _tags[bt.tagId]?.name ?? bt.tagId;
      if (name.isEmpty) continue;
      final key = normalizePersistedBookKey(bt.bookKey);
      map[name] = (map[name] ?? 0) + (readCountsByBookKey[key] ?? 0);
    }
    return map;
  }

  /// 获取某标签下的所有 bookKey。
  List<String> bookKeysForTag(String tagName) {
    final id = _tagId(tagName);
    return _bookTags
        .where((bt) => bt.tagId == id)
        .map((bt) => normalizePersistedBookKey(bt.bookKey))
        .toList();
  }

  // ---- internal ----

  /// 回填时用的辅助：创建标签 + 建立关联。
  void _addTagAndLink(String name, String bookKey) {
    final tagId = _tagId(name);
    _tags.putIfAbsent(tagId, () => Tag(id: tagId, name: name));
    _bookTags.add(
      BookTag(bookKey: normalizePersistedBookKey(bookKey), tagId: tagId),
    );
  }

  /// Returns null for a user/manual tag, an empty string for an obsolete
  /// generated tag, or its canonical display name for a generated synonym.
  static String? _canonicalGeneratedTag(String name) {
    final trimmed = name.trim();
    final lower = trimmed.toLowerCase();
    final plain = switch (lower) {
      'uncensored' || 'unmodified' => '无修正',
      'censored' || 'modified' => '有修正',
      'full_color' || 'full colour' || 'colorized' || 'colourized' => '彩漫',
      'color_pages' || 'colored_pages' || 'colour_pages' => '彩页',
      'complete' || 'completed' || 'collection' => '合集',
      'incomplete' || 'partial' => '未完结',
      '中文' ||
      '中文翻译' ||
      'chinese' ||
      'translated' ||
      'translation' ||
      'chinese_translation' => 'Chinese',
      'machine' || 'mtl' || 'machine_translation' || 'ai_translation' => '机翻',
      'human_translation' || 'manual_translation' => '人工翻译',
      '数字版' || 'digital' || 'ebook' || 'electronic' => '',
      'dl' || 'hd' || 'high_quality' || 'high_definition' => '高清',
      'full_scan' || 'cover_to_cover' => '全本扫描',
      'no_ads' || 'clean' => '无广告',
      'monochrome' || 'black_and_white' => '黑白漫',
      _ => null,
    };
    if (plain != null) return plain;
    final generated =
        lower.startsWith('resource:') ||
        lower.startsWith('sequence:') ||
        lower.startsWith('publication:') ||
        lower.startsWith('release:') ||
        lower.startsWith('release-group:') ||
        lower.startsWith('release_group:') ||
        lower.startsWith('language:') ||
        lower.startsWith('translation:') ||
        lower.startsWith('translation-method:') ||
        lower.startsWith('translation_method:') ||
        lower.startsWith('edition:') ||
        lower.startsWith('censorship:') ||
        lower.startsWith('color:') ||
        lower.startsWith('completeness:') ||
        lower.startsWith('medium:') ||
        lower.startsWith('scan:') ||
        lower.startsWith('tag:');
    if (!generated) return null;

    if (lower.startsWith('release-group:')) {
      final group = trimmed.substring('release-group:'.length).trim();
      return group.isEmpty ? '' : '汉化组：$group';
    }

    String? canonical(String value) {
      switch (value) {
        case 'language:zh':
        case 'language:cn':
        case 'language:chinese':
          return 'Chinese';
        case 'language:en':
          return '英文';
        case 'language:ja':
        case 'language:jp':
          return '日文';
        case 'translation:translated':
        case 'tag:translated':
        case 'tag:translation':
        case 'tag:chinese_translation':
          return 'Chinese';
        case 'translation:untranslated':
        case 'tag:untranslated':
          return '未翻译';
        case 'translation-method:machine':
        case 'translation_method:machine':
        case 'tag:machine':
        case 'tag:mtl':
        case 'tag:machine_translation':
        case 'tag:ai_translation':
          return '机翻';
        case 'translation-method:human':
        case 'translation_method:human':
        case 'tag:human_translation':
          return '人工翻译';
        case 'edition:digital':
        case 'edition:dl':
        case 'edition:ebook':
        case 'edition:electronic':
        case 'tag:digital':
        case 'tag:ebook':
        case 'resource:edition:digital':
        case 'resource:edition:dl':
        case 'resource:edition:ebook':
        case 'resource:edition:electronic':
        case 'resource:tag:digital':
        case 'resource:tag:ebook':
          return '';
        case 'tag:dl':
        case 'tag:hd':
        case 'tag:high_quality':
        case 'tag:high_definition':
        case 'resource:tag:dl':
        case 'resource:tag:hd':
        case 'resource:tag:high_quality':
        case 'resource:tag:high_definition':
          return '高清';
        case 'edition:print':
          return '实体版';
        case 'censorship:uncensored':
        case 'tag:uncensored':
        case 'tag:unmodified':
          return '无修正';
        case 'censorship:censored':
        case 'tag:censored':
        case 'tag:modified':
          return '有修正';
        case 'color:full_color':
        case 'color:colorized':
        case 'tag:full_color':
        case 'tag:colorized':
        case 'tag:color':
          return '彩漫';
        case 'color:color_pages':
        case 'tag:color_pages':
          return '彩页';
        case 'tag:monochrome':
        case 'tag:black_and_white':
          return '黑白漫';
        case 'completeness:complete':
        case 'completeness:collection':
        case 'tag:complete':
        case 'tag:completed':
        case 'tag:collection':
          return '合集';
        case 'completeness:incomplete':
        case 'completeness:partial':
        case 'tag:incomplete':
        case 'tag:partial':
          return '未完结';
        case 'medium:tankoubon':
        case 'medium:volume':
        case 'tag:tankoubon':
          return '单行本';
        case 'medium:serial':
        case 'tag:serial':
          return '连载';
        case 'scan:complete':
        case 'scan:cover_to_cover':
        case 'tag:full_scan':
        case 'tag:cover_to_cover':
          return '全本扫描';
        case 'scan:no_ads':
        case 'tag:no_ads':
        case 'tag:clean':
          return '无广告';
        default:
          return null;
      }
    }

    // Explicitly recognized generated tags are mapped; all other generated
    // namespaces are implementation details and should be removed.
    return canonical(lower) ?? '';
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
          _tags[newId] = Tag(
            id: newId,
            name: existing.name,
            createdAt: oldTag.createdAt,
          );
        }
      } else if (oldTag != null) {
        _tags[newId] = Tag(
          id: newId,
          name: oldTag.name,
          createdAt: oldTag.createdAt,
        );
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
