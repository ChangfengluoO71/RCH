import 'package:app/src/rust/api/source.dart';
import 'package:app/store/library_store.dart';
import 'package:app/store/models.dart';
import 'package:app/ui/book_detail_page.dart';
import 'package:app/ui/cache_manager.dart';
import 'package:app/ui/comic_cover.dart';
import 'package:app/ui/opener.dart';
import 'package:app/ui/source_browser.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

class HomePage extends StatefulWidget {
  const HomePage({super.key});
  @override
  State<HomePage> createState() => _HomePageState();
}

class _HomePageState extends State<HomePage> {
  String _section = 'recent';
  BookSource? _source;
  // ---- 统一搜索状态（筛选模式和全局模式共用） ----
  String _textSearch = '';           // 纯文字部分
  final Set<String> _tags = {};      // 已完成的标签
  String _tagDraft = '';             // 正在输入的标签片段（#后文字）
  final TextEditingController _searchCtrl = TextEditingController();
  bool _globalMode = false;          // false=筛选当前视图, true=跨书源搜索
  String? _detailTag;
  /// 元数据标签各组的展开状态（会话内保持，按类别名索引）。
  final Map<String, bool> _metaExpandedGroups = {};

  @override
  void initState() {
    super.initState();
    LibraryStore.instance.load();
  }

  @override
  void dispose() {
    _searchCtrl.dispose();
    super.dispose();
  }

  void _select(String s, [BookSource? src]) => setState(() {
    _section = s; _source = src;
    _textSearch = ''; _tags.clear(); _tagDraft = '';
    _searchCtrl.clear(); _globalMode = false;
  });

  // ============================================================
  // 统一搜索解析（筛选模式和全局模式共用）
  // ============================================================
  void _onSearch(String raw) {
    // 1. 已完成的 #标签（后面跟空格）
    _tags.clear();
    final re = RegExp(r'#(\S+)\s');
    int consumeEnd = 0;
    for (final m in re.allMatches(raw)) {
      _tags.add(m.group(1)!);
      consumeEnd = m.end;
    }
    // 2. 纯文字
    _textSearch = raw.substring(consumeEnd).replaceAll('#', '').trim();
    // 3. 正在输入的标签草稿
    final lastHash = raw.lastIndexOf('#');
    if (lastHash >= 0 && lastHash >= consumeEnd) {
      final afterHash = raw.substring(lastHash + 1);
      if (!afterHash.contains(' ') && afterHash.isNotEmpty) {
        _tagDraft = afterHash;
      } else {
        _tagDraft = '';
      }
    } else {
      _tagDraft = '';
    }
    setState(() {});
  }

  void _onTapCompletion(String tag) {
    final text = _searchCtrl.text;
    final lastHash = text.lastIndexOf('#');
    final before = text.substring(0, lastHash);
    _searchCtrl.text = '$before#$tag ';
    _searchCtrl.selection = TextSelection.collapsed(offset: _searchCtrl.text.length);
    _onSearch(_searchCtrl.text);
  }

  void _onRemoveTag(String tag) {
    _searchCtrl.text = _searchCtrl.text.replaceFirst('#$tag ', '');
    _searchCtrl.selection = TextSelection.collapsed(offset: _searchCtrl.text.length);
    _onSearch(_searchCtrl.text);
  }

  @override
  Widget build(BuildContext context) => AnimatedBuilder(
      animation: LibraryStore.instance,
      builder: (c, _) => Scaffold(body: Row(children: [
        _buildSidebar(),
        const VerticalDivider(width: 1),
        Expanded(child: _buildContent()),
      ])));

  // ============================================================
  // 侧栏
  // ============================================================
  Widget _buildSidebar() {
    final store = LibraryStore.instance;
    // 补全候选项
    final completions = _tagDraft.isEmpty ? <String>[] :
        store.allTags().where((t) => t.toLowerCase().contains(_tagDraft.toLowerCase())).take(8).toList();

    return Material(
      color: Theme.of(context).colorScheme.surfaceContainerLow,
      child: SizedBox(width: 230, child: Column(children: [
        // ---- 搜索栏（统一，不分模式） ----
        Padding(
          padding: const EdgeInsets.fromLTRB(10, 10, 10, 0),
          child: Column(mainAxisSize: MainAxisSize.min, children: [
            Row(children: [
              Expanded(
                child: TextField(
                  controller: _searchCtrl,
                  decoration: InputDecoration(
                    hintText: _globalMode ? '文字 / #标签 跨源搜索' : '文字 / #标签 筛选',
                    prefixIcon: Icon(_globalMode ? Icons.search : Icons.filter_list, size: 20),
                    suffixIcon: _searchCtrl.text.isNotEmpty
                        ? IconButton(icon: const Icon(Icons.clear, size: 18), onPressed: () { _searchCtrl.clear(); _onSearch(''); })
                        : null,
                    isDense: true, filled: true, fillColor: Colors.white10,
                    border: OutlineInputBorder(borderRadius: BorderRadius.circular(8), borderSide: BorderSide.none),
                  ),
                  onChanged: _onSearch,
                ),
              ),
              const SizedBox(width: 4),
              Tooltip(
                message: _globalMode ? '切换为视图筛选' : '切换为跨书源搜索',
                child: Material(
                  color: _globalMode ? Colors.amber.shade800 : Colors.transparent,
                  borderRadius: BorderRadius.circular(8),
                  child: InkWell(
                    borderRadius: BorderRadius.circular(8),
                    onTap: () { _searchCtrl.clear(); _textSearch = ''; _tags.clear(); _tagDraft = ''; setState(() => _globalMode = !_globalMode); },
                    child: Padding(
                      padding: const EdgeInsets.all(8),
                      child: Icon(_globalMode ? Icons.public : Icons.public_off, size: 18, color: _globalMode ? Colors.white : Colors.white54),
                    ),
                  ),
                ),
              ),
            ]),
            // 标签补全
            if (completions.isNotEmpty)
              Container(
                margin: const EdgeInsets.only(top: 4),
                decoration: BoxDecoration(color: Theme.of(context).colorScheme.surfaceContainerHighest, borderRadius: BorderRadius.circular(8)),
                constraints: const BoxConstraints(maxHeight: 150),
                child: ListView(padding: EdgeInsets.zero, shrinkWrap: true, children: completions.map((t) => ListTile(
                  dense: true, visualDensity: VisualDensity.compact,
                  contentPadding: const EdgeInsets.symmetric(horizontal: 10),
                  title: Text('#$t', style: const TextStyle(fontSize: 12, color: Colors.amber, fontWeight: FontWeight.w600)),
                  onTap: () => _onTapCompletion(t),
                )).toList()),
              ),
            // 已选标签 Chip
            if (_tags.isNotEmpty)
              Padding(
                padding: const EdgeInsets.only(top: 4),
                child: Wrap(spacing: 4, runSpacing: 2, children: _tags.map((t) => Chip(
                  label: Text('#$t', style: const TextStyle(fontSize: 11)),
                  deleteIcon: const Icon(Icons.close, size: 14),
                  onDeleted: () => _onRemoveTag(t),
                  materialTapTargetSize: MaterialTapTargetSize.shrinkWrap,
                  visualDensity: VisualDensity.compact, padding: EdgeInsets.zero,
                )).toList()),
              ),
          ]),
        ),
        _nav(Icons.history, '最近阅读', 'recent'),
        _nav(Icons.whatshot, '最多阅读', 'most'),
        _nav(Icons.label, '标签管理', 'tags'),
        const Divider(height: 18),
        Padding(padding: const EdgeInsets.symmetric(horizontal: 14), child: Row(children: [
          const Text('书源', style: TextStyle(color: Colors.white54, fontSize: 12)), const Spacer(),
          InkWell(onTap: () => showDialog(context: context, builder: (c) => const AddSourceDialog()), borderRadius: BorderRadius.circular(4),
            child: const Padding(padding: EdgeInsets.all(2), child: Icon(Icons.add, size: 18, color: Colors.white70))),
        ])),
        const SizedBox(height: 4),
        Expanded(child: ListView(children: store.sources.map(_sourceTile).toList())),
        const Divider(height: 8),
        _nav(Icons.settings, '设置', 'settings'),
      ])),
    );
  }

  Widget _nav(IconData icon, String label, String s) => Padding(
    padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 1),
    child: ListTile(dense: true, leading: Icon(icon, size: 20), title: Text(label, style: const TextStyle(fontSize: 14)),
      selected: _section == s, selectedTileColor: Colors.white10,
      shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(8)),
      onTap: () => _select(s)),
  );

  Widget _sourceTile(BookSource src) {
    final sel = _section == 'source' && _source?.id == src.id;
    return Padding(padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 1), child: ListTile(
      dense: true, leading: Icon(src.isWebDav ? Icons.cloud : Icons.folder, size: 20, color: src.isWebDav ? Colors.lightBlueAccent : Colors.amber),
      title: Row(mainAxisSize: MainAxisSize.min, children: [
        Text(src.capabilityDisplay.emoji, style: const TextStyle(fontSize: 12)), const SizedBox(width: 4),
        Flexible(child: Text(src.name, maxLines: 1, overflow: TextOverflow.ellipsis, style: const TextStyle(fontSize: 14))),
      ]),
      selected: sel, selectedTileColor: Colors.white10, shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(8)),
      onTap: () => _select('source', src),
      trailing: PopupMenuButton<String>(itemBuilder: (c) => const [
        PopupMenuItem(value: 'edit', child: Text('编辑书源')),
        PopupMenuItem(value: 'detail', child: Text('书源详情')),
        PopupMenuItem(value: 'delete', child: Text('删除书源')),
      ], onSelected: (act) {
        if (act == 'edit') {
          _showEditSource(src);
        } else if (act == 'detail') {
          _showSourceDetail(src);
        } else {
          _deleteSource(src);
        }
      }),
    ));
  }

  // ============================================================
  // 右侧内容区 —— 全局模式 vs 筛选模式共用 _textSearch + _tags
  // ============================================================
  Widget _buildContent() {
    // 全局模式：使用 globalSearch() 跨所有书源搜索
    if (_globalMode) return _buildGlobalResults();
    // 筛选模式：过滤当前视图
    return switch (_section) {
      'recent'   => _buildLocalResults(LibraryStore.instance.recent, '最近阅读'),
      'most'     => _buildLocalResults(LibraryStore.instance.mostRead, '最多阅读'),
      'source'   => _source == null
          ? const Center(child: Text('请从左侧选择一个书源'))
          : SourceBrowser(key: ValueKey(_source!.id), source: _source!, search: _textSearch, selectedTags: _tags),
      'tags'     => _buildTagManager(),
      'settings' => _buildSettings(),
      _          => const SizedBox(),
    };
  }

  // ---- 全局搜索结果 ----
  Widget _buildGlobalResults() {
    final results = LibraryStore.instance.globalSearch(text: _textSearch, tags: _tags);
    return Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
      _buildFilterBar(results.length),
      Expanded(
        child: results.isEmpty
            ? const Center(child: Text('没有匹配的漫画', style: TextStyle(color: Colors.white38)))
            : GridView.builder(padding: const EdgeInsets.all(16), gridDelegate: const SliverGridDelegateWithMaxCrossAxisExtent(maxCrossAxisExtent: 180, childAspectRatio: 0.66, crossAxisSpacing: 12, mainAxisSpacing: 12),
                itemCount: results.length, itemBuilder: (c, i) {
                  final r = results[i];
                  return ComicCard(source: r.source, path: r.path, title: r.title, subtitle: r.source.name, onTap: () => openBook(context, r.source, r.path, r.title));
                }),
      ),
    ]);
  }

  // ---- 本地视图结果（最近/最多） ----
  Widget _buildLocalResults(List<ReadRecord> records, String title) {
    var list = records;
    final store = LibraryStore.instance;
    if (_textSearch.isNotEmpty) {
      final q = _textSearch.toLowerCase();
      list = list.where((r) => r.title.toLowerCase().contains(q)).toList();
    }
    if (_tags.isNotEmpty) {
      list = list.where((r) {
        final key = bookKeyOf(r.sourceType, r.sourceId, r.path);
        final m = store.metas[key] ?? store.metas['${r.sourceId}|${r.path}'];
        return m != null && _tags.every((t) => m.tags.contains(t) || m.metaTags.contains(t));
      }).toList();
    }
    return Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
      Padding(padding: const EdgeInsets.fromLTRB(16, 14, 16, 4), child: Text(title, style: const TextStyle(fontSize: 18, fontWeight: FontWeight.bold))),
      Expanded(
        child: list.isEmpty
            ? const Center(child: Text('暂无记录\n去书源里打开一本漫画吧', textAlign: TextAlign.center, style: TextStyle(color: Colors.white38)))
            : GridView.builder(padding: const EdgeInsets.all(16), gridDelegate: const SliverGridDelegateWithMaxCrossAxisExtent(maxCrossAxisExtent: 180, childAspectRatio: 0.66, crossAxisSpacing: 12, mainAxisSpacing: 12),
                itemCount: list.length, itemBuilder: (c, i) {
                  final r = list[i]; final s = store.sourceById(r.sourceId);
                  if (s == null) return const SizedBox();
                  return ComicCard(source: s, path: r.path, title: r.title, subtitle: '读到 ${r.lastPage + 1} 页 · 看过 ${r.readCount} 次', onTap: () => openBook(context, s, r.path, r.title));
                }),
      ),
    ]);
  }

  // 筛选条件条
  Widget _buildFilterBar(int count) => Container(
    color: Theme.of(context).colorScheme.surfaceContainerHighest.withAlpha(80),
    padding: const EdgeInsets.fromLTRB(16, 14, 16, 14),
    child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
      Row(children: [
        const Icon(Icons.public, size: 16, color: Colors.amber), const SizedBox(width: 8),
        const Text('跨书源搜索', style: TextStyle(fontSize: 16, fontWeight: FontWeight.w600)),
        const Spacer(), Text('$count 本', style: TextStyle(fontSize: 13, color: Colors.white54)),
      ]),
      if (_textSearch.isNotEmpty || _tags.isNotEmpty)
        Padding(padding: const EdgeInsets.only(top: 6), child: Wrap(spacing: 4, runSpacing: 2, children: [
          if (_textSearch.isNotEmpty) Chip(label: Text('"$_textSearch"', style: const TextStyle(fontSize: 11)), deleteIcon: const Icon(Icons.close, size: 14),
            onDeleted: () { _searchCtrl.text = _searchCtrl.text.replaceFirst(_textSearch, '').trim(); _onSearch(_searchCtrl.text); },
            materialTapTargetSize: MaterialTapTargetSize.shrinkWrap, visualDensity: VisualDensity.compact, padding: EdgeInsets.zero),
          ..._tags.map((t) => Chip(label: Text('#$t', style: const TextStyle(fontSize: 11)), deleteIcon: const Icon(Icons.close, size: 14),
            onDeleted: () => _onRemoveTag(t), materialTapTargetSize: MaterialTapTargetSize.shrinkWrap, visualDensity: VisualDensity.compact, padding: EdgeInsets.zero)),
          TextButton.icon(onPressed: () { _searchCtrl.clear(); _onSearch(''); }, icon: const Icon(Icons.clear_all, size: 14), label: const Text('清除', style: TextStyle(fontSize: 11)), style: TextButton.styleFrom(padding: const EdgeInsets.symmetric(horizontal: 6), minimumSize: Size.zero, tapTargetSize: MaterialTapTargetSize.shrinkWrap)),
        ])),
    ]),
  );

  // ============================================================
  // 书源编辑/详情/删除
  // ============================================================
  void _showEditSource(BookSource src) {
    final nameCtrl = TextEditingController(text: src.name), urlCtrl = TextEditingController(text: src.url ?? ''),
        userCtrl = TextEditingController(text: src.username ?? ''), passCtrl = TextEditingController(text: src.password ?? ''),
        pathCtrl = TextEditingController(text: src.path), noteCtrl = TextEditingController(text: src.note);
    showDialog(context: context, builder: (ctx) => AlertDialog(title: Text('编辑书源: ${src.name}'),
      content: SingleChildScrollView(child: Column(mainAxisSize: MainAxisSize.min, children: [
        _fd('名称', nameCtrl),
        if (src.isWebDav) ...[_fd('服务器地址', urlCtrl), _fd('用户名', userCtrl), _fdPw('密码', passCtrl), _fd('初始路径', pathCtrl)] else _fd('目录路径', pathCtrl),
        _fd('备注', noteCtrl),
      ])),
      actions: [TextButton(onPressed: () => Navigator.of(ctx).pop(), child: const Text('取消')),
        FilledButton(onPressed: () { LibraryStore.instance.updateSource(src.id, name: nameCtrl.text.trim(), url: urlCtrl.text.trim(), username: userCtrl.text.trim(), password: passCtrl.text.trim(), path: pathCtrl.text.trim(), note: noteCtrl.text.trim()); Navigator.of(ctx).pop(); }, child: const Text('保存'))]));
  }
  Widget _fd(String label, TextEditingController c) => Padding(padding: const EdgeInsets.only(bottom: 8), child: TextField(controller: c, decoration: InputDecoration(labelText: label, border: const OutlineInputBorder(), isDense: true)));
  Widget _fdPw(String label, TextEditingController c) => Padding(padding: const EdgeInsets.only(bottom: 8), child: TextField(controller: c, obscureText: true, decoration: InputDecoration(labelText: label, border: const OutlineInputBorder(), isDense: true)));

  void _showSourceDetail(BookSource src) {
    final ctrl = TextEditingController(text: src.note);
    showDialog(context: context, builder: (ctx) => AlertDialog(title: Text('书源详情: ${src.name}'),
      content: Column(mainAxisSize: MainAxisSize.min, children: [
        Text(src.isWebDav ? 'WebDAV' : '本地目录', style: Theme.of(context).textTheme.bodySmall),
        Text(src.isWebDav ? (src.url ?? '') : src.path, maxLines: 1, overflow: TextOverflow.ellipsis, style: Theme.of(context).textTheme.bodySmall),
        const SizedBox(height: 16), const Text('备注', style: TextStyle(fontWeight: FontWeight.w600)), const SizedBox(height: 8),
        TextField(controller: ctrl, maxLines: 3, decoration: const InputDecoration(border: OutlineInputBorder(), isDense: true)),
      ]),
      actions: [TextButton(onPressed: () => Navigator.of(ctx).pop(), child: const Text('取消')),
        FilledButton(onPressed: () { src.note = ctrl.text.trim(); LibraryStore.instance.updateSource(src.id, note: src.note); Navigator.of(ctx).pop(); }, child: const Text('保存'))]));
  }

  void _deleteSource(BookSource src) {
    showDialog<bool>(context: context, builder: (ctx) => AlertDialog(title: Text('删除书源"${src.name}"?'), content: const Text('将同时删除该书源下的所有阅读记录和元数据'),
      actions: [TextButton(onPressed: () => Navigator.of(ctx).pop(false), child: const Text('取消')), FilledButton(onPressed: () => Navigator.of(ctx).pop(true), child: const Text('删除'))]))
      .then((ok) { if (ok == true) { LibraryStore.instance.removeSourceWithCleanup(src.id); setState(() {}); } });
  }

  // ============================================================
  // 标签管理
  // ============================================================
  Widget _buildTagManager() => ListenableBuilder(listenable: LibraryStore.instance, builder: (c, _) {
    final store = LibraryStore.instance; final stats = store.tagStats(); final metaSet = store.metaTagNames();
    final q = _textSearch.toLowerCase();
    final filtered = q.isEmpty ? stats : stats.where((s) => s.$1.toLowerCase().contains(q)).toList();
    final metaTags = filtered.where((s) => metaSet.contains(s.$1)).toList();
    final normalTags = filtered.where((s) => !metaSet.contains(s.$1)).toList();

    // 元数据标签按一级类别分组（显示层，不改模型）：作者/类别/系列/状态。
    final metaGroups = <String, List<(String, int, int)>>{};
    for (final t in metaTags) {
      metaGroups.putIfAbsent(_metaTagCategory(t.$1), () => []).add(t);
    }
    final orderedGroups = <(String, List<(String, int, int)>)>[];
    for (final cat in const ['作者', '类别', '系列', 'AI超分', '状态']) {
      final g = metaGroups[cat];
      if (g != null && g.isNotEmpty) orderedGroups.add((cat, g));
    }

    Widget tile(t, bool isMeta) => ListTile(
      leading: Icon(Icons.label, size: 20, color: isMeta ? Colors.redAccent : Colors.amber),
      title: Text('${t.$1} (${t.$2} 本 · ${t.$3} 次阅读)'),
      subtitle: isMeta ? const Text('元数据标签', style: TextStyle(fontSize: 11, color: Colors.redAccent)) : null,
      onTap: () => setState(() => _detailTag = t.$1),
      trailing: PopupMenuButton<String>(itemBuilder: (c) => const [PopupMenuItem(value: 'rename', child: Text('重命名')), PopupMenuItem(value: 'delete', child: Text('删除标签'))],
        onSelected: (act) async {
          if (act == 'rename') { final ctrl = TextEditingController(text: t.$1); final n = await showDialog<String>(context: context, builder: (c) => AlertDialog(title: const Text('重命名标签'), content: TextField(controller: ctrl, autofocus: true, decoration: const InputDecoration(border: OutlineInputBorder())), actions: [TextButton(onPressed: () => Navigator.of(c).pop(), child: const Text('取消')), FilledButton(onPressed: () => Navigator.of(c).pop(ctrl.text.trim()), child: const Text('确认'))])); if (n != null && n.isNotEmpty) LibraryStore.instance.renameTag(t.$1, n); }
          else if (act == 'delete') { final ok = await showDialog<bool>(context: context, builder: (c) => AlertDialog(title: Text('删除标签"${t.$1}"?'), content: const Text('将从所有漫画中移除此标签'), actions: [TextButton(onPressed: () => Navigator.of(c).pop(false), child: const Text('取消')), FilledButton(onPressed: () => Navigator.of(c).pop(true), child: const Text('删除'))])); if (ok == true) LibraryStore.instance.deleteTag(t.$1); }
        }));

    return Row(children: [
      Expanded(flex: 3, child: filtered.isEmpty
        ? const Center(child: Text('暂无标签', style: TextStyle(color: Colors.white38)))
        : ListView(children: [
            ...orderedGroups.map((g) => ExpansionTile(
              key: ValueKey('meta-${g.$1}-$q'),
              initiallyExpanded: q.isNotEmpty ? true : (_metaExpandedGroups[g.$1] ?? false),
              onExpansionChanged: (v) => setState(() => _metaExpandedGroups[g.$1] = v),
              leading: const Icon(Icons.label, color: Colors.redAccent),
              title: Text('${g.$1} (${g.$2.length})', style: const TextStyle(color: Colors.redAccent, fontWeight: FontWeight.w600)),
              children: g.$2.map((t) => tile(t, true)).toList(),
            )),
            ...normalTags.map((t) => tile(t, false)),
          ])),
      if (_detailTag != null) ...[const VerticalDivider(width: 1), Expanded(flex: 5, child: _buildTagDetail(_detailTag!))],
    ]);
  });

  /// 元数据标签的一级类别（显示层分组，不改模型）：
  /// 按"所属书籍最多的字段"归类，平局取 author > genre > series；已读 → 状态。
  String _metaTagCategory(String tag) {
    if (tag == '已读') return '状态';
    // AI超分 是独立元数据标签（超分完成时打标），不归属于作者/类别/系列字段。
    if (tag == 'AI超分') return 'AI超分';
    var author = 0, genre = 0, series = 0;
    for (final m in LibraryStore.instance.metas.values) {
      if (m.author == tag) author++;
      if (m.genre == tag) genre++;
      if (m.series == tag) series++;
    }
    if (author >= genre && author >= series) return '作者';
    if (genre >= series) return '类别';
    return '系列';
  }

  Widget _buildTagDetail(String tag) {
    final list = LibraryStore.instance.recordsByTag(tag);
    return Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
      Padding(padding: const EdgeInsets.fromLTRB(16, 14, 16, 4), child: Row(children: [
        Text('标签: $tag', style: const TextStyle(fontSize: 16, fontWeight: FontWeight.w600)), const Spacer(),
        IconButton(icon: const Icon(Icons.close), onPressed: () => setState(() => _detailTag = null)),
      ])),
      Expanded(child: list.isEmpty
        ? const Center(child: Text('没有匹配的漫画', style: TextStyle(color: Colors.white38)))
        : GridView.builder(padding: const EdgeInsets.all(16), gridDelegate: const SliverGridDelegateWithMaxCrossAxisExtent(maxCrossAxisExtent: 180, childAspectRatio: 0.66, crossAxisSpacing: 12, mainAxisSpacing: 12), itemCount: list.length, itemBuilder: (c, i) {
            final r = list[i]; final s = LibraryStore.instance.sourceById(r.sourceId);
            if (s == null) return const SizedBox();
            return ComicCard(source: s, path: r.path, title: r.title, subtitle: '读到 ${r.lastPage + 1} 页 · 看过 ${r.readCount} 次',
              onTap: () => Navigator.of(context).push(MaterialPageRoute(builder: (_) => BookDetailPage(source: s, path: r.path, title: r.title))));
          })),
    ]);
  }

  // ============================================================
  // 设置
  // ============================================================
  Widget _buildSettings() {
    final s = LibraryStore.instance.settings;
    return ListenableBuilder(listenable: LibraryStore.instance, builder: (c, _) => ListView(padding: const EdgeInsets.all(24), children: [
      const Text('设置', style: TextStyle(fontSize: 22, fontWeight: FontWeight.bold)), const SizedBox(height: 28), const CacheManagerPanel(), const SizedBox(height: 28),
      _readingDefaults(s), const SizedBox(height: 16), _localComics(s), const SizedBox(height: 16), _keybinds(s), const SizedBox(height: 28), _coverQuality(s), const SizedBox(height: 32), _theme(s),
    ]));
  }

  Widget _localComics(AppSettings s) => Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
    const Text('本地漫画', style: TextStyle(fontSize: 16, fontWeight: FontWeight.w600)), const SizedBox(height: 4),
    SwitchListTile(
      title: const Text('自动转 CBZ'),
      subtitle: const Text('刷新本地书源时，后台将漫画文件夹 / zip 打包为 CBZ；转换后与原内容视为同一本漫画（进度/标签保留）'),
      dense: true,
      contentPadding: EdgeInsets.zero,
      value: s.autoConvertCbz,
      onChanged: (v) {
        s.autoConvertCbz = v;
        LibraryStore.instance.updateSettings(s);
        setState(() {});
      },
    ),
  ]);

  Widget _readingDefaults(AppSettings s) => Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
    const Text('阅读默认', style: TextStyle(fontSize: 16, fontWeight: FontWeight.w600)), const SizedBox(height: 4),
    Text('新打开的漫画默认使用以下设置', style: Theme.of(context).textTheme.bodySmall), const SizedBox(height: 10),
    Row(children: [const Text('默认模式: '), DropdownButton<ReadMode>(value: s.readMode, items: ReadMode.values.map((r) => DropdownMenuItem(value: r, child: Text(r.label))).toList(), onChanged: (v) { if (v != null) { s.readMode = v; LibraryStore.instance.updateSettings(s); setState(() {}); } })]),
    const SizedBox(height: 8), const Text('双页拼接默认为:'), const SizedBox(height: 6),
    SegmentedButton<DualPageMode>(segments: DualPageMode.values.map((d) => ButtonSegment(value: d, label: Text(d.label))).toList(), selected: {s.dualPageMode}, onSelectionChanged: (vs) { s.dualPageMode = vs.first; LibraryStore.instance.updateSettings(s); setState(() {}); }),
    const SizedBox(height: 8),
    Row(children: [const Text('拼接间隙: '), SizedBox(width: 120, child: Slider(value: s.dualPageGap.toDouble(), min: 0, max: 20, divisions: 20, label: '${s.dualPageGap}px', onChanged: (v) { s.dualPageGap = v.toInt(); LibraryStore.instance.updateSettings(s); setState(() {}); })), Text('${s.dualPageGap}px')]),
    const SizedBox(height: 8),
    Row(children: [const Text('首页单独显示'), const Spacer(), Switch(value: s.skipFrontCover, onChanged: (v) { s.skipFrontCover = v; LibraryStore.instance.updateSettings(s); setState(() {}); })]),
  ]);

  Widget _keybinds(AppSettings s) => Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
    const Text('自定义按键', style: TextStyle(fontSize: 16, fontWeight: FontWeight.w600)), const SizedBox(height: 4),
    Text('点击右侧按键标签后,按下新按键即可更改', style: Theme.of(context).textTheme.bodySmall), const SizedBox(height: 8),
    _keyRow('前进', s.keys.forwardKey, (k) { s.keys.forward = k.keyId; LibraryStore.instance.updateSettings(s); setState(() {}); }),
    _keyRow('后退', s.keys.backKey, (k) { s.keys.back = k.keyId; LibraryStore.instance.updateSettings(s); setState(() {}); }),
    _keyRow('放大', s.keys.zoomInKey, (k) { s.keys.zoomIn = k.keyId; LibraryStore.instance.updateSettings(s); setState(() {}); }),
    _keyRow('缩小', s.keys.zoomOutKey, (k) { s.keys.zoomOut = k.keyId; LibraryStore.instance.updateSettings(s); setState(() {}); }),
    _keyRow('缩放还原', s.keys.zoomResetKey, (k) { s.keys.zoomReset = k.keyId; LibraryStore.instance.updateSettings(s); setState(() {}); }),
  ]);

  Widget _keyRow(String label, LogicalKeyboardKey current, ValueChanged<LogicalKeyboardKey> fn) => Padding(padding: const EdgeInsets.symmetric(vertical: 3), child: Row(children: [
    Text(label), const Spacer(),
    GestureDetector(onTap: () async { final k = await showDialog<LogicalKeyboardKey>(context: context, builder: (c) => _KeyCaptureDialog(current: current)); if (k != null) fn(k); },
      child: Container(padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 5), decoration: BoxDecoration(border: Border.all(color: Colors.white30), borderRadius: BorderRadius.circular(6)), child: Text(current.debugName ?? '?', style: const TextStyle(fontSize: 13)))),
  ]));

  Widget _coverQuality(AppSettings s) => Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
    const Text('封面质量', style: TextStyle(fontSize: 16, fontWeight: FontWeight.w600)), const SizedBox(height: 4),
    Text('影响书架封面的扫描速度与清晰度', style: Theme.of(context).textTheme.bodySmall), const SizedBox(height: 10),
    SegmentedButton<CoverQuality>(segments: CoverQuality.values.map((q) => ButtonSegment(value: q, label: Text(q.label))).toList(), selected: {s.coverQuality}, onSelectionChanged: (qs) { s.coverQuality = qs.first; LibraryStore.instance.updateSettings(s); ComicCover.clear(); }),
  ]);

  Widget _theme(AppSettings s) => Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
    const Text('主题', style: TextStyle(fontSize: 16, fontWeight: FontWeight.w600)), const SizedBox(height: 10),
    SegmentedButton<String>(segments: const [ButtonSegment(value: 'dark', label: Text('夜间'), icon: Icon(Icons.dark_mode)), ButtonSegment(value: 'light', label: Text('白天'), icon: Icon(Icons.light_mode))], selected: {s.themeMode}, onSelectionChanged: (ms) { s.themeMode = ms.first; LibraryStore.instance.updateSettings(s); }),
  ]);
}

class _KeyCaptureDialog extends StatefulWidget { final LogicalKeyboardKey current; const _KeyCaptureDialog({required this.current}); @override State<_KeyCaptureDialog> createState() => _KeyCaptureDialogState(); }
class _KeyCaptureDialogState extends State<_KeyCaptureDialog> {
  LogicalKeyboardKey? _k;
  @override void initState() { super.initState(); _k = widget.current; }
  @override Widget build(BuildContext c) => AlertDialog(title: const Text('按下新按键'), content: Focus(autofocus: true, onKeyEvent: (n, e) { if (e is KeyDownEvent) { setState(() => _k = e.logicalKey); return KeyEventResult.handled; } return KeyEventResult.ignored; }, child: Column(mainAxisSize: MainAxisSize.min, children: [Text('当前: ${_k?.debugName ?? '?'}', textAlign: TextAlign.center), const SizedBox(height: 8), const Text('按下任意键绑定', style: TextStyle(fontSize: 12, color: Colors.white54))])), actions: [TextButton(onPressed: () => Navigator.of(c).pop(), child: const Text('取消')), FilledButton(onPressed: () => Navigator.of(c).pop(_k ?? widget.current), child: const Text('确认'))]);
}

class AddSourceDialog extends StatefulWidget { const AddSourceDialog({super.key}); @override State<AddSourceDialog> createState() => _AddDialogState(); }
class _AddDialogState extends State<AddSourceDialog> {
  bool _w = true;
  final _a = TextEditingController(), _b = TextEditingController(), _u = TextEditingController(), _p = TextEditingController(), _s = TextEditingController();
  bool _t = false; String? _e;
  Future<void> _submit() async {
    final n = _a.text.trim();
    if (_w) { if (_u.text.trim().isEmpty) { setState(() => _e = '请填写服务器地址'); return; } setState(() { _t = true; _e = null; });
      try { final s = await webdavConnect(url: _u.text.trim(), username: _p.text.trim(), password: _s.text); LibraryStore.instance.addSource(BookSource(id: 'webdav_${DateTime.now().millisecondsSinceEpoch}', type: 'webdav', name: n.isEmpty ? _u.text.trim() : n, path: _b.text.trim().isEmpty ? s.root : _b.text.trim(), url: _u.text.trim(), username: _p.text.trim(), password: _s.text)); if (mounted) Navigator.of(context).pop(); } catch (e) { setState(() => _e = '连接失败:$e'); } finally { if (mounted) setState(() => _t = false); }
    } else { if (_b.text.trim().isEmpty) { setState(() => _e = '请填写目录路径'); return; } LibraryStore.instance.addSource(BookSource(id: 'local_${DateTime.now().millisecondsSinceEpoch}', type: 'local', name: n.isEmpty ? _b.text.trim() : n, path: _b.text.trim())); if (mounted) Navigator.of(context).pop(); }
  }
  @override Widget build(BuildContext c) => AlertDialog(title: const Text('添加书源'), content: SizedBox(width: 420, child: Column(mainAxisSize: MainAxisSize.min, children: [
    SegmentedButton<bool>(segments: const [ButtonSegment(value: true, label: Text('WebDAV'), icon: Icon(Icons.cloud)), ButtonSegment(value: false, label: Text('本地目录'), icon: Icon(Icons.folder))], selected: {_w}, onSelectionChanged: (s) => setState(() => _w = s.first)),
    const SizedBox(height: 16), TextField(controller: _a, decoration: const InputDecoration(labelText: '名称(可选)', border: OutlineInputBorder(), isDense: true)), const SizedBox(height: 10),
    if (_w) ...[TextField(controller: _u, decoration: const InputDecoration(labelText: '服务器地址', hintText: 'https://nas:5006/dav', border: OutlineInputBorder(), isDense: true)), const SizedBox(height: 10), TextField(controller: _p, decoration: const InputDecoration(labelText: '用户名', border: OutlineInputBorder(), isDense: true)), const SizedBox(height: 10), TextField(controller: _s, obscureText: true, decoration: const InputDecoration(labelText: '密码', border: OutlineInputBorder(), isDense: true)), const SizedBox(height: 10), TextField(controller: _b, decoration: const InputDecoration(labelText: '初始路径(可选,默认根目录)', border: OutlineInputBorder(), isDense: true))] else TextField(controller: _b, decoration: const InputDecoration(labelText: '目录路径', hintText: r'F:\comic\漫畫', border: OutlineInputBorder(), isDense: true)),
    if (_e != null) Padding(padding: const EdgeInsets.only(top: 10), child: Text(_e!, style: const TextStyle(color: Colors.redAccent, fontSize: 12))),
  ])), actions: [TextButton(onPressed: () => Navigator.of(c).pop(), child: const Text('取消')), FilledButton(onPressed: _t ? null : _submit, child: Text(_t ? '测试中…' : '添加'))]);
}
