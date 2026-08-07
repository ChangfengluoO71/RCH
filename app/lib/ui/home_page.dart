import 'package:app/src/rust/api/source.dart';
import 'package:app/src/rust/api/book.dart';
import 'package:app/store/library_store.dart';
import 'package:app/store/models.dart';
import 'package:app/store/netdisk_credentials.dart';
import 'package:app/store/quark_session.dart';
import 'package:app/store/sync_manager.dart';
import 'package:app/ui/book_detail_page.dart';
import 'package:app/ui/cache_manager.dart';
import 'package:app/ui/comic_cover.dart';
import 'package:app/ui/common.dart';
import 'package:app/ui/opener.dart';
import 'package:app/ui/source_browser.dart';
import 'package:app/ui/sync_panel.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:qr_flutter/qr_flutter.dart';
import 'package:url_launcher/url_launcher.dart';

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
    if (defaultTargetPlatform == TargetPlatform.android) {
      SystemChrome.setPreferredOrientations([DeviceOrientation.portraitUp]);
    }
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
      builder: (c, _) {
        if (isCompact(c)) return _buildCompactShell();
        return Scaffold(body: Row(children: [
          _buildSidebar(),
          const VerticalDivider(width: 1),
          Expanded(child: _buildContent()),
        ]));
      });

  Widget _buildCompactShell() {
    return Scaffold(
      appBar: AppBar(
        title: Text(_compactTitle()),
        leading: _section == 'source' && _source != null
            ? BackButton(onPressed: () => _select('source'))
            : null,
      ),
      body: Column(children: [
        _buildSearchHeader(),
        const Divider(height: 1),
        Expanded(child: _buildContent()),
      ]),
      bottomNavigationBar: NavigationBar(
        selectedIndex: _compactNavIndex,
        onDestinationSelected: _onCompactNav,
        destinations: const [
          NavigationDestination(icon: Icon(Icons.history), label: '最近'),
          NavigationDestination(icon: Icon(Icons.whatshot), label: '最多'),
          NavigationDestination(icon: Icon(Icons.label_outline), selectedIcon: Icon(Icons.label), label: '标签'),
          NavigationDestination(icon: Icon(Icons.cloud_outlined), selectedIcon: Icon(Icons.cloud), label: '书源'),
          NavigationDestination(icon: Icon(Icons.settings_outlined), selectedIcon: Icon(Icons.settings), label: '设置'),
        ],
      ),
    );
  }

  int get _compactNavIndex => switch (_section) {
        'recent' => 0,
        'most' => 1,
        'tags' => 2,
        'source' => 3,
        'settings' => 4,
        _ => 0,
      };

  void _onCompactNav(int i) {
    switch (i) {
      case 0:
        _select('recent');
      case 1:
        _select('most');
      case 2:
        _select('tags');
      case 3:
        _select('source');
      case 4:
        _select('settings');
    }
  }

  String _compactTitle() => switch (_section) {
        'recent' => '最近阅读',
        'most' => '最多阅读',
        'tags' => '标签管理',
        'source' => '书源',
        'settings' => '设置',
        _ => 'RCH',
      };

  // ============================================================
  // 侧栏
  // ============================================================
  Widget _buildSidebar() {
    return Material(
      color: Theme.of(context).colorScheme.surfaceContainerLow,
      child: SizedBox(width: 230, child: Column(children: [
        _buildSearchHeader(),
        _nav(Icons.history, '最近阅读', 'recent'),
        _nav(Icons.whatshot, '最多阅读', 'most'),
        _nav(Icons.label, '标签管理', 'tags'),
        const Divider(height: 18),
        _buildSourceList(),
        const Divider(height: 8),
        _nav(Icons.settings, '设置', 'settings'),
      ])),
    );
  }

  Widget _buildSearchHeader() {
    final store = LibraryStore.instance;
    final completions = _tagDraft.isEmpty ? <String>[] :
        store.allTags().where((t) => t.toLowerCase().contains(_tagDraft.toLowerCase())).take(8).toList();
    return Padding(
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
    );
  }

  Widget _buildSourceList() {
    final store = LibraryStore.instance;
    return Column(children: [
      Padding(padding: const EdgeInsets.symmetric(horizontal: 14), child: Row(children: [
        const Text('书源', style: TextStyle(color: Colors.white54, fontSize: 12)), const Spacer(),
        InkWell(onTap: () => showDialog(context: context, builder: (c) => const AddSourceDialog()), borderRadius: BorderRadius.circular(4),
          child: const Padding(padding: EdgeInsets.all(2), child: Icon(Icons.add, size: 18, color: Colors.white70))),
      ])),
      const SizedBox(height: 4),
      Expanded(child: ListView(children: store.sources.where((s) => !s.remoteOnly).map(_sourceTile).toList())),
    ]);
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
    final icon = src.isWebDav
        ? Icons.cloud
        : src.isSftp
            ? Icons.dns
            : src.isSmb
                ? Icons.lan
                : src.isBaidu
                    ? Icons.cloud_queue
                : src.is115
                    ? Icons.cloud_upload
                    : src.isQuark
                        ? Icons.cloud_done
                        : Icons.folder;
    final iconColor = src.isWebDav
        ? Colors.lightBlueAccent
        : src.isSftp
            ? Colors.tealAccent
            : src.isBaidu
                ? Colors.orangeAccent
                : src.is115
                    ? Colors.amber
                    : src.isQuark
                        ? Colors.cyanAccent
                        : Colors.amber;
    return Padding(padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 1), child: ListTile(
      dense: true, leading: Icon(icon, size: 20, color: iconColor),
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
          ? (isCompact(context)
              ? _buildSourceList()
              : const Center(child: Text('请从左侧选择一个书源')))
          : SourceBrowser(key: ValueKey(_source!.id), source: _source!, search: _textSearch, selectedTags: _tags),
      'tags'     => _buildTagManager(),
      'settings' => _buildSettings(),
      _          => const SizedBox(),
    };
  }

  // ---- 全局搜索结果 ----
  Widget _buildGlobalResults() {
    final results = LibraryStore.instance.globalSearch(
      text: _textSearch,
      tags: _tags,
      includeRemoteOnly: SyncManager.instance.crossDeviceSearch,
    );
    return Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
      _buildFilterBar(results.length),
      Expanded(
        child: results.isEmpty
            ? const Center(child: Text('没有匹配的漫画', style: TextStyle(color: Colors.white38)))
            : GridView.builder(padding: const EdgeInsets.all(16), gridDelegate: const SliverGridDelegateWithMaxCrossAxisExtent(maxCrossAxisExtent: 180, childAspectRatio: 0.66, crossAxisSpacing: 12, mainAxisSpacing: 12),
                itemCount: results.length, itemBuilder: (c, i) {
                  final r = results[i];
                  final ghost = r.source.remoteOnly;
                  return ComicCard(
                    source: r.source,
                    path: r.path,
                    title: r.title,
                    subtitle: ghost
                        ? '仅元数据 · 来自${SyncManager.instance.deviceNameOf(r.source.originDeviceId)}'
                        : r.source.name,
                    onTap: ghost
                        ? () => Navigator.of(context).push(MaterialPageRoute(
                            builder: (_) => BookDetailPage(
                                source: r.source, path: r.path, title: r.title)))
                        : () => openBook(context, r.source, r.path, r.title),
                  );
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
                  return ComicCard(source: s, path: r.path, title: r.title, subtitle: '读到 ${r.lastPage + 1} 页 · 看过 ${r.readCount} 次',
                    onTap: () => Navigator.of(context).push(MaterialPageRoute(builder: (_) => BookDetailPage(source: s, path: r.path, title: r.title))));
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
        userCtrl = TextEditingController(text: src.username ?? ''),
        passCtrl = TextEditingController(text: src.password ?? ''),
        portCtrl = TextEditingController(text: src.port?.toString() ?? ''),
        pathCtrl = TextEditingController(text: src.path), noteCtrl = TextEditingController(text: src.note),
        tokenCtrl = TextEditingController(text: src.refreshToken ?? ''),
        appKeyCtrl = TextEditingController(text: src.clientId ?? ''),
        secretCtrl = TextEditingController(text: src.clientSecret ?? ''),
        rootIdCtrl = TextEditingController(text: src.rootId ?? ''),
        cookieCtrl = TextEditingController(text: src.cookie ?? '');
    showDialog(context: context, builder: (ctx) => AlertDialog(title: Text('编辑书源: ${src.name}'),
      content: SingleChildScrollView(child: Column(mainAxisSize: MainAxisSize.min, children: [
        _fd('名称', nameCtrl),
        if (src.isWebDav) ...[_fd('服务器地址', urlCtrl), _fd('用户名', userCtrl), _fdPw('密码', passCtrl), _fd('初始路径', pathCtrl)]
        else if (src.isSftp) ...[_fd('服务器地址', urlCtrl), _fd('端口(默认22)', portCtrl), _fd('用户名', userCtrl), _fdPw('密码', passCtrl), _fd('初始路径(默认/)', pathCtrl)]
        else if (src.isBaidu) ...[_fd('根目录(默认/)', pathCtrl), _fdPw('refresh_token', tokenCtrl), _fd('AppKey(留空用内置)', appKeyCtrl), _fdPw('SecretKey(留空用内置)', secretCtrl)]
        else if (src.is115) ...[_fd('根文件夹 ID', rootIdCtrl), _fdPw('refresh_token', tokenCtrl), _fd('APP ID(留空用内置)', appKeyCtrl)]
        else if (src.isQuark) ...[_fd('根文件夹 ID', rootIdCtrl), _fdPw('Cookie', cookieCtrl)]
        else if (src.isSmb) _fd('共享目录路径(UNC)', pathCtrl)
        else _fd('目录路径', pathCtrl),
        _fd('备注', noteCtrl),
      ])),
      actions: [TextButton(onPressed: () => Navigator.of(ctx).pop(), child: const Text('取消')),
        FilledButton(onPressed: () {
          final port = src.isSftp ? int.tryParse(portCtrl.text.trim()) : null;
          LibraryStore.instance.updateSource(src.id,
              name: nameCtrl.text.trim(),
              url: urlCtrl.text.trim(),
              username: userCtrl.text.trim(),
              password: passCtrl.text.trim(),
              port: port,
              path: pathCtrl.text.trim(),
              refreshToken: tokenCtrl.text.trim().isEmpty ? null : tokenCtrl.text.trim(),
              clientId: appKeyCtrl.text.trim().isEmpty ? null : appKeyCtrl.text.trim(),
              clientSecret: secretCtrl.text.trim().isEmpty ? null : secretCtrl.text.trim(),
              rootId: rootIdCtrl.text.trim().isEmpty ? null : rootIdCtrl.text.trim(),
              cookie: cookieCtrl.text.trim().isEmpty ? null : cookieCtrl.text.trim(),
              note: noteCtrl.text.trim());
          if (src.isQuark) clearQuarkSession(src.id);
          Navigator.of(ctx).pop();
        }, child: const Text('保存'))]));
  }
  Widget _fd(String label, TextEditingController c) => Padding(padding: const EdgeInsets.only(bottom: 8), child: TextField(controller: c, decoration: InputDecoration(labelText: label, border: const OutlineInputBorder(), isDense: true)));
  Widget _fdPw(String label, TextEditingController c) => Padding(padding: const EdgeInsets.only(bottom: 8), child: TextField(controller: c, obscureText: true, decoration: InputDecoration(labelText: label, border: const OutlineInputBorder(), isDense: true)));

  void _showSourceDetail(BookSource src) {
    final ctrl = TextEditingController(text: src.note);
    showDialog(context: context, builder: (ctx) => AlertDialog(title: Text('书源详情: ${src.name}'),
      content: Column(mainAxisSize: MainAxisSize.min, children: [
        Text(_sourceTypeLabel(src), style: Theme.of(context).textTheme.bodySmall),
        Text(src.needsSession
            ? (src.isBaidu || src.is115 || src.isQuark ? src.path : (src.url ?? ''))
            : src.path, maxLines: 1, overflow: TextOverflow.ellipsis, style: Theme.of(context).textTheme.bodySmall),
        const SizedBox(height: 16), const Text('备注', style: TextStyle(fontWeight: FontWeight.w600)), const SizedBox(height: 8),
        TextField(controller: ctrl, maxLines: 3, decoration: const InputDecoration(border: OutlineInputBorder(), isDense: true)),
      ]),
      actions: [TextButton(onPressed: () => Navigator.of(ctx).pop(), child: const Text('取消')),
        FilledButton(onPressed: () { src.note = ctrl.text.trim(); LibraryStore.instance.updateSource(src.id, note: src.note); Navigator.of(ctx).pop(); }, child: const Text('保存'))]));
  }

  String _sourceTypeLabel(BookSource src) => switch (src.type) {
        'webdav' => 'WebDAV',
        'sftp' => 'SFTP',
        'smb' => 'SMB 共享',
        'baidu' => '百度网盘',
        '115' => '115 网盘',
        'quark' => '夸克网盘',
        _ => '本地目录',
      };

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

    if (isCompact(c) && _detailTag != null) {
      return Column(children: [
        Align(alignment: Alignment.centerLeft, child: TextButton.icon(
          onPressed: () => setState(() => _detailTag = null),
          icon: const Icon(Icons.arrow_back, size: 18),
          label: const Text('返回标签列表'),
        )),
        Expanded(child: _buildTagDetail(_detailTag!)),
      ]);
    }
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
      const Text('设置', style: TextStyle(fontSize: 22, fontWeight: FontWeight.bold)), const SizedBox(height: 28), const CacheManagerPanel(), const SizedBox(height: 28), const SyncPanel(), const SizedBox(height: 28),
      _readingDefaults(s), const SizedBox(height: 16), _remoteSources(s), const SizedBox(height: 16), _localComics(s), const SizedBox(height: 16), _keybinds(s), const SizedBox(height: 28), _coverQuality(s), const SizedBox(height: 32), _theme(s),
    ]));
  }

  Widget _localComics(AppSettings s) => Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
    const Text('本地漫画', style: TextStyle(fontSize: 16, fontWeight: FontWeight.w600)), const SizedBox(height: 4),
    SwitchListTile(
      title: const Text('自动转 CBZ'),
      subtitle: const Text('刷新本地 / SMB 书源时，后台将漫画文件夹 / zip 打包为 CBZ（需共享目录有写权限）；转换后与原内容视为同一本漫画（进度/标签保留）'),
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

  Widget _remoteSources(AppSettings s) => Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
    const Text('远程书源', style: TextStyle(fontSize: 16, fontWeight: FontWeight.w600)), const SizedBox(height: 4),
    Text('WebDAV / SFTP 打开漫画时的策略', style: Theme.of(context).textTheme.bodySmall), const SizedBox(height: 10),
    SingleChildScrollView(
      scrollDirection: Axis.horizontal,
      child: SegmentedButton<BookOpenStrategy>(
        segments: BookOpenStrategy.values
            .map((st) => ButtonSegment(value: st, label: Text(st.label)))
            .toList(),
        selected: {s.bookOpenStrategy},
        onSelectionChanged: (vs) {
          s.bookOpenStrategy = vs.first;
          LibraryStore.instance.updateSettings(s);
          setState(() {});
        },
      ),
    ),
    const SizedBox(height: 8),
    Text('自动：先下载整本到缓存（有进度条），失败转流式；下载整本：适合网速快或想离线读；直接流式：即点即读、不占缓存',
        style: Theme.of(context).textTheme.bodySmall),
  ]);

  Widget _readingDefaults(AppSettings s) => Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
    const Text('阅读默认', style: TextStyle(fontSize: 16, fontWeight: FontWeight.w600)), const SizedBox(height: 4),
    Text('新打开的漫画默认使用以下设置', style: Theme.of(context).textTheme.bodySmall), const SizedBox(height: 10),
    Wrap(spacing: 8, runSpacing: 4, crossAxisAlignment: WrapCrossAlignment.center, children: [const Text('默认模式: '), DropdownButton<ReadMode>(value: s.readMode, items: ReadMode.values.map((r) => DropdownMenuItem(value: r, child: Text(r.label))).toList(), onChanged: (v) { if (v != null) { s.readMode = v; LibraryStore.instance.updateSettings(s); setState(() {}); } })]),
    const SizedBox(height: 8), const Text('双页拼接默认为:'), const SizedBox(height: 6),
    SingleChildScrollView(scrollDirection: Axis.horizontal, child: SegmentedButton<DualPageMode>(segments: DualPageMode.values.map((d) => ButtonSegment(value: d, label: Text(d.label))).toList(), selected: {s.dualPageMode}, onSelectionChanged: (vs) { s.dualPageMode = vs.first; LibraryStore.instance.updateSettings(s); setState(() {}); })),
    const SizedBox(height: 8),
    Wrap(spacing: 8, runSpacing: 4, crossAxisAlignment: WrapCrossAlignment.center, children: [const Text('拼接间隙: '), SizedBox(width: 120, child: Slider(value: s.dualPageGap.toDouble(), min: 0, max: 20, divisions: 20, label: '${s.dualPageGap}px', onChanged: (v) { s.dualPageGap = v.toInt(); LibraryStore.instance.updateSettings(s); setState(() {}); })), Text('${s.dualPageGap}px')]),
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
    SingleChildScrollView(scrollDirection: Axis.horizontal, child: SegmentedButton<CoverQuality>(segments: CoverQuality.values.map((q) => ButtonSegment(value: q, label: Text(q.label))).toList(), selected: {s.coverQuality}, onSelectionChanged: (qs) { s.coverQuality = qs.first; LibraryStore.instance.updateSettings(s); ComicCover.clear(); })),
  ]);

  Widget _theme(AppSettings s) => Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
    const Text('主题', style: TextStyle(fontSize: 16, fontWeight: FontWeight.w600)), const SizedBox(height: 10),
    SingleChildScrollView(scrollDirection: Axis.horizontal, child: SegmentedButton<String>(segments: const [ButtonSegment(value: 'dark', label: Text('夜间'), icon: Icon(Icons.dark_mode)), ButtonSegment(value: 'light', label: Text('白天'), icon: Icon(Icons.light_mode))], selected: {s.themeMode}, onSelectionChanged: (ms) { s.themeMode = ms.first; LibraryStore.instance.updateSettings(s); })),
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
  String _type = 'webdav';
  bool _showAdv = false;
  final _a = TextEditingController(), _b = TextEditingController(), _u = TextEditingController(), _p = TextEditingController(), _s = TextEditingController(), _port = TextEditingController(),
      _token = TextEditingController(), _appKey = TextEditingController(), _secret = TextEditingController(), _rootId = TextEditingController(), _cookie = TextEditingController();
  bool _t = false; String? _e;

  String get _baiduKey =>
      _appKey.text.trim().isNotEmpty ? _appKey.text.trim() : kBaiduDefaultAppKey;
  String get _baiduSecret =>
      _secret.text.trim().isNotEmpty ? _secret.text.trim() : kBaiduDefaultSecret;
  String get _appId =>
      _appKey.text.trim().isNotEmpty ? _appKey.text.trim() : kCloud115DefaultAppId;

  Future<void> _submit() async {
    final n = _a.text.trim();
    if (_type == 'webdav') { if (_u.text.trim().isEmpty) { setState(() => _e = '请填写服务器地址'); return; } setState(() { _t = true; _e = null; });
      try { final s = await webdavConnect(url: _u.text.trim(), username: _p.text.trim(), password: _s.text); LibraryStore.instance.addSource(BookSource(id: 'webdav_${DateTime.now().millisecondsSinceEpoch}', type: 'webdav', name: n.isEmpty ? _u.text.trim() : n, path: _b.text.trim().isEmpty ? s.root : _b.text.trim(), url: _u.text.trim(), username: _p.text.trim(), password: _s.text)); if (mounted) Navigator.of(context).pop(); } catch (e) { setState(() => _e = '连接失败:$e'); } finally { if (mounted) setState(() => _t = false); }
    } else if (_type == 'sftp') {
      if (_u.text.trim().isEmpty) { setState(() => _e = '请填写服务器地址'); return; }
      setState(() { _t = true; _e = null; });
      try {
        final (host, port) = _sftpHostPort();
        final s = await sftpConnect(host: host, port: port, username: _p.text.trim(), password: _s.text);
        LibraryStore.instance.addSource(BookSource(id: 'sftp_${DateTime.now().millisecondsSinceEpoch}', type: 'sftp', name: n.isEmpty ? _u.text.trim() : n, path: _b.text.trim().isEmpty ? s.root : _b.text.trim(), url: _u.text.trim(), username: _p.text.trim(), password: _s.text, port: port));
        if (mounted) Navigator.of(context).pop();
      } catch (e) { setState(() => _e = '连接失败:$e'); } finally { if (mounted) setState(() => _t = false); }
    } else if (_type == 'smb') {
      final path = _b.text.trim();
      if (path.isEmpty || !path.startsWith(r'\\')) { setState(() => _e = '请填写 UNC 共享路径（以 \\ 开头，如 \\\\192.168.1.10\\comic）'); return; }
      setState(() { _t = true; _e = null; });
      try {
        await listLocalDir(path: path); // 连通性测试（无权限/路径不存在会抛错）
        LibraryStore.instance.addSource(BookSource(id: 'smb_${DateTime.now().millisecondsSinceEpoch}', type: 'smb', name: n.isEmpty ? path : n, path: path));
        if (mounted) Navigator.of(context).pop();
      } catch (e) { setState(() => _e = '无法访问该共享目录:$e'); } finally { if (mounted) setState(() => _t = false); }
    } else if (_type == 'baidu') {
      await _submitBaidu(n);
    } else if (_type == '115') {
      await _submit115(n);
    } else if (_type == 'quark') {
      await _submitQuark(n);
    } else {
      if (_b.text.trim().isEmpty) { setState(() => _e = '请填写目录路径'); return; }
      LibraryStore.instance.addSource(BookSource(id: 'local_${DateTime.now().millisecondsSinceEpoch}', type: 'local', name: n.isEmpty ? _b.text.trim() : n, path: _b.text.trim())); if (mounted) Navigator.of(context).pop();
    }
  }

  Future<void> _submitBaidu(String n) async {
    final rt = _token.text.trim();
    if (rt.isEmpty) { setState(() => _e = '请先授权登录或粘贴 refresh_token'); return; }
    setState(() { _t = true; _e = null; });
    try {
      final s = await baiduConnect(
          refreshToken: rt,
          appKey: _baiduKey,
          clientSecret: _baiduSecret,
          root: _b.text.trim().isEmpty ? '/' : _b.text.trim());
      LibraryStore.instance.addSource(BookSource(
          id: 'baidu_${DateTime.now().millisecondsSinceEpoch}',
          type: 'baidu',
          name: n.isEmpty ? '百度网盘' : n,
          path: s.root,
          refreshToken: s.refreshToken,
          clientId: _baiduKey,
          clientSecret: _baiduSecret));
      if (mounted) Navigator.of(context).pop();
    } catch (e) { setState(() => _e = '连接失败:$e'); } finally { if (mounted) setState(() => _t = false); }
  }

  Future<void> _submit115(String n) async {
    final rt = _token.text.trim();
    if (rt.isEmpty) { setState(() => _e = '请先扫码授权或粘贴 refresh_token'); return; }
    setState(() { _t = true; _e = null; });
    try {
      final s = await cloud115Connect(
          refreshToken: rt,
          appId: _appId,
          rootId: _rootId.text.trim().isEmpty ? '0' : _rootId.text.trim());
      LibraryStore.instance.addSource(BookSource(
          id: '115_${DateTime.now().millisecondsSinceEpoch}',
          type: '115',
          name: n.isEmpty ? '115 网盘' : n,
          path: s.root,
          refreshToken: s.refreshToken,
          clientId: _appId,
          rootId: s.root));
      if (mounted) Navigator.of(context).pop();
    } catch (e) { setState(() => _e = '连接失败:$e'); } finally { if (mounted) setState(() => _t = false); }
  }

  Future<void> _submitQuark(String n) async {
    final cookie = _cookie.text.trim();
    if (cookie.isEmpty) { setState(() => _e = '请粘贴夸克网盘 Cookie（pan.quark.cn 登录后 F12 复制）'); return; }
    setState(() { _t = true; _e = null; });
    try {
      final s = await quarkConnect(
          cookie: cookie,
          rootId: _rootId.text.trim().isEmpty ? '0' : _rootId.text.trim());
      LibraryStore.instance.addSource(BookSource(
          id: 'quark_${DateTime.now().millisecondsSinceEpoch}',
          type: 'quark',
          name: n.isEmpty ? '夸克网盘' : n,
          path: s.root,
          rootId: s.root,
          cookie: s.cookie));
      if (mounted) Navigator.of(context).pop();
    } catch (e) { setState(() => _e = '连接失败:$e'); } finally { if (mounted) setState(() => _t = false); }
  }

  /// 百度 OAuth：浏览器授权 → 粘贴授权码 → 换 token。
  Future<void> _baiduAuthorize() async {
    if (_baiduKey.isEmpty || _baiduSecret.isEmpty) {
      setState(() => _e = '未配置百度 AppKey/SecretKey（可在高级选项填写）');
      return;
    }
    try {
      final url = await baiduAuthUrl(appKey: _baiduKey);
      await launchUrl(Uri.parse(url),
          mode: LaunchMode.externalApplication);
    } catch (_) {}
    if (!mounted) return;
    final codeCtrl = TextEditingController();
    final ok = await showDialog<bool>(
      context: context,
      builder: (c) => AlertDialog(
        title: const Text('百度授权'),
        content: Column(mainAxisSize: MainAxisSize.min, children: [
          const Text('浏览器已打开百度授权页，登录并同意后，把页面显示的授权码粘贴到这里。',
              style: TextStyle(fontSize: 12)),
          const SizedBox(height: 10),
          TextField(controller: codeCtrl,
              decoration: const InputDecoration(labelText: '授权码', border: OutlineInputBorder(), isDense: true)),
        ]),
        actions: [
          TextButton(onPressed: () => Navigator.of(c).pop(false), child: const Text('取消')),
          FilledButton(onPressed: () => Navigator.of(c).pop(true), child: const Text('换取 token')),
        ],
      ),
    );
    if (ok != true || codeCtrl.text.trim().isEmpty) return;
    try {
      final pair = await baiduExchangeCode(
          appKey: _baiduKey, clientSecret: _baiduSecret, code: codeCtrl.text.trim());
      if (mounted) setState(() { _token.text = pair.refreshToken; _e = null; });
    } catch (e) { if (mounted) setState(() => _e = '授权失败:$e'); }
  }

  /// 115 设备码授权：弹二维码 → 手机扫码 → 自动填 refresh_token。
  Future<void> _cloud115Authorize() async {
    if (_appId.isEmpty) {
      setState(() => _e = '未配置 115 APP ID（可在高级选项填写）');
      return;
    }
    try {
      final qr = await cloud115QrStart(appId: _appId);
      if (!mounted) return;
      await showDialog<void>(
        context: context,
        builder: (c) => _QrScanDialog(
          payload: qr,
          onResult: (status, _, rt) {
            if (mounted) {
              if (status == 2 && rt != null) {
                setState(() { _token.text = rt; _e = null; });
              } else if (status == -1) {
                setState(() => _e = '二维码已过期，请重新获取');
              } else if (status == -2) {
                setState(() => _e = '已取消扫码');
              }
            }
          },
        ),
      );
    } catch (e) { if (mounted) setState(() => _e = '获取二维码失败:$e'); }
  }

  /// 解析 SFTP 服务器地址：`host` / `host:port`，端口缺省取端口字段或 22。
  (String, int) _sftpHostPort() {
    final addr = _u.text.trim();
    if (addr.contains(':')) {
      final idx = addr.lastIndexOf(':');
      final p = int.tryParse(addr.substring(idx + 1));
      if (p != null && p > 0) return (addr.substring(0, idx), p);
    }
    return (addr, int.tryParse(_port.text.trim()) ?? 22);
  }

  @override Widget build(BuildContext c) => AlertDialog(title: const Text('添加书源'), content: ConstrainedBox(constraints: BoxConstraints(maxWidth: dialogMaxWidth(c)), child: SingleChildScrollView(child: Column(mainAxisSize: MainAxisSize.min, children: [
    DropdownMenu<String>(
      initialSelection: _type,
      label: const Text('类型'),
      expandedInsets: EdgeInsets.zero,
      dropdownMenuEntries: const [
        DropdownMenuEntry(value: 'local', label: '本地目录', leadingIcon: Icon(Icons.folder)),
        DropdownMenuEntry(value: 'webdav', label: 'WebDAV', leadingIcon: Icon(Icons.cloud)),
        DropdownMenuEntry(value: 'smb', label: 'SMB', leadingIcon: Icon(Icons.lan)),
        DropdownMenuEntry(value: 'sftp', label: 'SFTP', leadingIcon: Icon(Icons.dns)),
        DropdownMenuEntry(value: 'baidu', label: '百度网盘', leadingIcon: Icon(Icons.cloud_queue)),
        DropdownMenuEntry(value: '115', label: '115 网盘', leadingIcon: Icon(Icons.cloud_upload)),
        DropdownMenuEntry(value: 'quark', label: '夸克网盘', leadingIcon: Icon(Icons.cloud_done)),
      ],
      onSelected: (v) => setState(() { _type = v ?? 'webdav'; _e = null; _showAdv = false; }),
    ),
    const SizedBox(height: 16), TextField(controller: _a, decoration: const InputDecoration(labelText: '名称(可选)', border: OutlineInputBorder(), isDense: true)), const SizedBox(height: 10),
    if (_type == 'local') TextField(controller: _b, decoration: const InputDecoration(labelText: '目录路径', hintText: r'F:\comic\漫畫', border: OutlineInputBorder(), isDense: true))
    else if (_type == 'smb') TextField(controller: _b, decoration: const InputDecoration(labelText: '共享目录路径(UNC)', hintText: r'\\192.168.1.10\comic', border: OutlineInputBorder(), isDense: true))
    else if (_type == 'webdav') ...[
      TextField(controller: _u, decoration: const InputDecoration(labelText: '服务器地址', hintText: 'https://nas:5006/dav', border: OutlineInputBorder(), isDense: true)), const SizedBox(height: 10),
      TextField(controller: _p, decoration: const InputDecoration(labelText: '用户名', border: OutlineInputBorder(), isDense: true)), const SizedBox(height: 10),
      TextField(controller: _s, obscureText: true, decoration: const InputDecoration(labelText: '密码', border: OutlineInputBorder(), isDense: true)), const SizedBox(height: 10),
      TextField(controller: _b, decoration: const InputDecoration(labelText: '初始路径(可选,默认根目录)', border: OutlineInputBorder(), isDense: true)),
    ] else if (_type == 'sftp') ...[
      TextField(controller: _u, decoration: const InputDecoration(labelText: '服务器地址', hintText: '192.168.1.10 或 nas:2222', border: OutlineInputBorder(), isDense: true)), const SizedBox(height: 10),
      TextField(controller: _port, keyboardType: TextInputType.number, decoration: const InputDecoration(labelText: '端口(默认22)', border: OutlineInputBorder(), isDense: true)), const SizedBox(height: 10),
      TextField(controller: _p, decoration: const InputDecoration(labelText: '用户名', border: OutlineInputBorder(), isDense: true)), const SizedBox(height: 10),
      TextField(controller: _s, obscureText: true, decoration: const InputDecoration(labelText: '密码', border: OutlineInputBorder(), isDense: true)), const SizedBox(height: 10),
      TextField(controller: _b, decoration: const InputDecoration(labelText: '初始路径(可选,默认/)', border: OutlineInputBorder(), isDense: true)),
    ] else if (_type == 'baidu') ...[
      TextField(controller: _b, decoration: const InputDecoration(labelText: '根目录(默认/)', hintText: '/漫画', border: OutlineInputBorder(), isDense: true)), const SizedBox(height: 10),
      OutlinedButton.icon(onPressed: _t ? null : _baiduAuthorize, icon: const Icon(Icons.login), label: const Text('授权登录')),
      const SizedBox(height: 10),
      TextField(controller: _token, obscureText: true, decoration: const InputDecoration(labelText: 'refresh_token(授权后自动填入，也可直接粘贴)', border: OutlineInputBorder(), isDense: true)),
      const SizedBox(height: 6),
      TextButton(onPressed: () => setState(() => _showAdv = !_showAdv), child: Text(_showAdv ? '收起高级选项' : '高级选项（自填 AppKey/SecretKey）')),
      if (_showAdv) ...[
        TextField(controller: _appKey, decoration: const InputDecoration(labelText: 'AppKey(留空用内置)', border: OutlineInputBorder(), isDense: true)), const SizedBox(height: 8),
        TextField(controller: _secret, obscureText: true, decoration: const InputDecoration(labelText: 'SecretKey(留空用内置)', border: OutlineInputBorder(), isDense: true)),
      ],
    ] else if (_type == 'quark') ...[
      TextField(controller: _rootId, decoration: const InputDecoration(labelText: '根文件夹 ID(默认 0)', border: OutlineInputBorder(), isDense: true)), const SizedBox(height: 10),
      TextField(controller: _cookie, obscureText: true, decoration: const InputDecoration(labelText: 'Cookie(pan.quark.cn 登录后 F12 复制)', hintText: 'stoken=...; pds=...; __puus=...', border: OutlineInputBorder(), isDense: true)),
    ] else ...[
      TextField(controller: _rootId, decoration: const InputDecoration(labelText: '根文件夹 ID(默认 0)', border: OutlineInputBorder(), isDense: true)), const SizedBox(height: 10),
      OutlinedButton.icon(onPressed: _t ? null : _cloud115Authorize, icon: const Icon(Icons.qr_code), label: const Text('扫码授权')),
      const SizedBox(height: 10),
      TextField(controller: _token, obscureText: true, decoration: const InputDecoration(labelText: 'refresh_token(扫码后自动填入，也可直接粘贴)', border: OutlineInputBorder(), isDense: true)),
      const SizedBox(height: 6),
      TextButton(onPressed: () => setState(() => _showAdv = !_showAdv), child: Text(_showAdv ? '收起高级选项' : '高级选项（自填 APP ID）')),
      if (_showAdv) TextField(controller: _appKey, decoration: const InputDecoration(labelText: 'APP ID(留空用内置)', border: OutlineInputBorder(), isDense: true)),
    ],
    if (_e != null) Padding(padding: const EdgeInsets.only(top: 10), child: Text(_e!, style: const TextStyle(color: Colors.redAccent, fontSize: 12))),
  ]))), actions: [TextButton(onPressed: () => Navigator.of(c).pop(), child: const Text('取消')), FilledButton(onPressed: _t ? null : _submit, child: Text(_t ? '测试中…' : '添加'))]);
}

/// 115 扫码授权对话框：渲染二维码并轮询状态。
class _QrScanDialog extends StatefulWidget {
  final Cloud115QrPayload payload;
  final void Function(int status, String? accessToken, String? refreshToken) onResult;
  const _QrScanDialog({required this.payload, required this.onResult});
  @override State<_QrScanDialog> createState() => _QrScanDialogState();
}

class _QrScanDialogState extends State<_QrScanDialog> {
  String _status = '请用 115 APP 扫码';

  @override
  void initState() {
    super.initState();
    _poll();
  }

  Future<void> _poll() async {
    while (mounted) {
      await Future.delayed(const Duration(seconds: 2));
      if (!mounted) return;
      try {
        final r = await cloud115QrPoll(
            uid: widget.payload.uid,
            time: widget.payload.time,
            sign: widget.payload.sign);
        if (!mounted) return;
        if (r.status == 2) {
          widget.onResult(2, r.accessToken, r.refreshToken);
          Navigator.of(context).pop();
          return;
        }
        if (r.status == 1) {
          setState(() => _status = '已扫码，请在手机上确认');
        } else if (r.status == -1 || r.status == -2) {
          widget.onResult(r.status, null, null);
          Navigator.of(context).pop();
          return;
        }
      } catch (_) {}
    }
  }

  @override
  Widget build(BuildContext c) => AlertDialog(
        title: const Text('115 扫码授权'),
        content: Column(mainAxisSize: MainAxisSize.min, children: [
          QrImageView(data: widget.payload.qrcode, size: 220),
          const SizedBox(height: 10),
          Text(_status, style: const TextStyle(fontSize: 12)),
        ]),
        actions: [
          TextButton(onPressed: () => Navigator.of(c).pop(), child: const Text('关闭')),
        ],
      );
}
