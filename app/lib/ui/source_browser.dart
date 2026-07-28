import 'package:app/src/rust/api/book.dart';
import 'package:app/src/rust/api/source.dart';
import 'package:app/store/library_store.dart';
import 'package:app/store/models.dart';
import 'package:app/ui/book_detail_page.dart';
import 'package:app/ui/comic_cover.dart';
import 'package:app/ui/common.dart';
import 'package:app/ui/opener.dart';
import 'package:flutter/material.dart';

/// 书源浏览器:浏览某个书源的漫画。
/// 本地 → 海报墙(目录可下钻);WebDAV → 列表(目录可下钻)。
class SourceBrowser extends StatefulWidget {
  final BookSource source;
  final String search;
  final Set<String> selectedTags;

  const SourceBrowser({super.key, required this.source, this.search = '', this.selectedTags = const {}});

  @override
  State<SourceBrowser> createState() => _SourceBrowserState();
}

class _SourceBrowserState extends State<SourceBrowser> {
  late String _path = widget.source.path;
  final List<String> _stack = [];
  List<DirEntry> _entries = [];
  bool _loading = false;
  String? _error;
  BigInt? _session;
  bool _posterMode = true; // true=海报墙, false=简略列表

  @override
  void initState() {
    super.initState();
    _init();
  }

  Future<void> _init() async {
    if (widget.source.isWebDav) {
      try {
        _session = await webdavSessionFor(widget.source);
      } catch (e) {
        if (mounted) setState(() => _error = '连接 WebDAV 失败:$e');
        return;
      }
    }
    await _list(_path);
  }

  Future<void> _list(String path) async {
    setState(() {
      _loading = true;
      _error = null;
    });
    try {
      final list = widget.source.isWebDav
          ? await webdavList(session: _session!, path: path)
          : await listLocalDir(path: path);
      if (!mounted) return;
      setState(() {
        _path = path;
        _entries = list
            .where((e) =>
                e.isDir ||
                ['.cbz', '.zip', '.epub', '.cb7', '.7z', '.cbt', '.tar', '.pdf', '.cbr', '.rar', '.mobi', '.azw', '.azw3'].any((ext) => e.name.toLowerCase().endsWith(ext)))
            .toList();
      });
    } catch (e) {
      if (mounted) setState(() => _error = '$e');
    } finally {
      if (mounted) setState(() => _loading = false);
    }
  }

  void _openDir(String path) {
    _stack.add(_path);
    _list(path);
  }

  void _goUp() {
    if (_stack.isNotEmpty) _list(_stack.removeLast());
  }

  List<DirEntry> get _filtered {
    Iterable<DirEntry> list = _entries;
    // 搜索
    if (widget.search.isNotEmpty) {
      final q = widget.search.toLowerCase();
      list = list.where((e) => e.name.toLowerCase().contains(q));
    }
    // 标签过滤(交集)
    if (widget.selectedTags.isNotEmpty) {
      final store = LibraryStore.instance;
      list = list.where((e) {
        if (e.isDir) return true;
        final newKey = '${widget.source.type}|${widget.source.id}|${e.path}';
        final legacyKey = '${widget.source.id}|${e.path}';
        final meta = store.metas[newKey] ?? store.metas[legacyKey];
        return meta != null && widget.selectedTags.every((t) => meta.tags.contains(t) || meta.metaTags.contains(t));
      });
    }
    return list.toList();
  }

  /// 已选中的漫画路径(用于批量标签)。
  final Set<String> _selectedPaths = {};
  bool _selectMode = false; // true=复选框出现,点击勾选而非进详情

  /// 递归收集目录及其子目录下所有漫画文件路径。
  Future<List<String>> _collectComicsRecursive(String dirPath) async {
    final result = <String>[];
    final pending = <String>[dirPath];
    while (pending.isNotEmpty) {
      final p = pending.removeAt(0);
      try {
        final list = widget.source.isWebDav
            ? await webdavList(session: _session!, path: p)
            : await listLocalDir(path: p);
        for (final e in list) {
          if (e.isDir) {
            pending.add(e.path);
          } else if (e.name.toLowerCase().endsWith('.cbz') ||
              e.name.toLowerCase().endsWith('.zip')) {
            result.add(e.path);
          }
        }
      } catch (_) {
        // 跳过无法访问的目录
      }
    }
    return result;
  }

  /// 批量标签操作:解析所有选中路径(含递归展开的文件夹),弹出标签对话框。
  Future<void> _batchTagFromSelection() async {
    if (_selectedPaths.isEmpty) {
      if (mounted) {
        ScaffoldMessenger.of(context)
            .showSnackBar(const SnackBar(content: Text('请先勾选漫画或文件夹')));
      }
      return;
    }
    // 收集所有要打标签的漫画路径(文件夹递归展开)
    final store = LibraryStore.instance;
    final expanded = <String>[];
    for (final p in _selectedPaths) {
      // 判断是文件夹还是漫画文件
      final isDir = _entries.any((e) => e.path == p && e.isDir);
      if (isDir || (!_entries.any((e) => e.path == p) && p != widget.source.path)) {
        expanded.addAll(await _collectComicsRecursive(p));
      } else {
        expanded.add(p);
      }
    }
    if (expanded.isEmpty) {
      if (mounted) {
        ScaffoldMessenger.of(context)
            .showSnackBar(const SnackBar(content: Text('所选路径下没有漫画文件')));
      }
      return;
    }
    final metaSet = store.metaTagNames();
    final all = store.allTags();
    final ctrl = TextEditingController();
    final tag = await showDialog<String>(
      context: context,
      builder: (c) => StatefulBuilder(
        builder: (c, ss) => AlertDialog(
          title: const Text('批量打标签'),
          content: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              Autocomplete<String>(
                optionsBuilder: (v) {
                  final q = v.text.toLowerCase();
                  if (q.isEmpty) return const <String>[];
                  return all.where((t) => t.toLowerCase().contains(q)).toList();
                },
                fieldViewBuilder: (c, fieldCtrl, fn, _) => TextField(
                  controller: fieldCtrl,
                  focusNode: fn,
                  autofocus: true,
                  decoration: const InputDecoration(
                      hintText: '输入标签名', border: OutlineInputBorder()),
                  onSubmitted: (v) {
                    if (v.trim().isNotEmpty) Navigator.of(c).pop(v.trim());
                  },
                ),
                optionsViewBuilder: (c, fn, opts) => Align(
                  alignment: Alignment.topLeft,
                  child: Material(
                    child: ConstrainedBox(
                      constraints: const BoxConstraints(maxHeight: 200),
                      child: ListView(
                        padding: EdgeInsets.zero,
                        shrinkWrap: true,
                        children: opts
                            .map((o) => ListTile(
                                  dense: true,
                                  leading: Icon(Icons.label,
                                      size: 16,
                                      color: metaSet.contains(o)
                                          ? Colors.redAccent
                                          : Colors.amber),
                                  title: Text(o,
                                      style: TextStyle(
                                          fontSize: 14,
                                          color: metaSet.contains(o)
                                              ? Colors.redAccent
                                              : null)),
                                  onTap: () => fn(o),
                                ))
                            .toList(),
                      ),
                    ),
                  ),
                ),
                onSelected: (v) => Navigator.of(c).pop(v),
              ),
            ],
          ),
          actions: [
            TextButton(onPressed: () => Navigator.of(c).pop(), child: const Text('取消')),
            FilledButton(
                onPressed: () {
                  final t = ctrl.text.trim();
                  if (t.isNotEmpty) Navigator.of(c).pop(t);
                },
                child: const Text('确认')),
          ],
        ),
      ),
    );
    if (tag != null && tag.isNotEmpty) {
      LibraryStore.instance.batchTag(widget.source, expanded, tag);
      _selectedPaths.clear();
      _selectMode = false;
      setState(() {});
    }
  }

  @override
  Widget build(BuildContext context) {
    return ListenableBuilder(
      listenable: LibraryStore.instance,
      builder: (context, _) => Column(
      children: [
        Material(
          color: Colors.black26,
          child: ListTile(
            dense: true,
            leading: IconButton(
              icon: const Icon(Icons.arrow_upward),
              tooltip: '上级目录',
              onPressed: _stack.isEmpty ? null : _goUp,
            ),
            title: _selectMode ? Text('已选 ${_selectedPaths.length} 项', maxLines: 1) : Text(_path, maxLines: 1, overflow: TextOverflow.ellipsis),
            trailing: Row(mainAxisSize: MainAxisSize.min, children: [
              if (_selectMode) ...[
                TextButton(onPressed: () => setState(() { _selectedPaths.clear(); }), child: const Text('取消全选')),
                TextButton(onPressed: () => setState(() { for (var e in _filtered) _selectedPaths.add(e.path); }), child: const Text('全选')),
                IconButton(icon: const Icon(Icons.label), tooltip: '批量打标签', onPressed: _batchTagFromSelection),
                IconButton(icon: const Icon(Icons.close), tooltip: '退出选择', onPressed: () => setState(() { _selectedPaths.clear(); _selectMode = false; })),
              ] else ...[
                IconButton(
                  icon: const Icon(Icons.checklist),
                  tooltip: '进入选择模式',
                  onPressed: () => setState(() => _selectMode = true),
                ),
              ],
              IconButton(icon: Icon(_posterMode ? Icons.view_list : Icons.grid_view), tooltip: _posterMode ? '切换为简略列表' : '切换为海报墙', onPressed: () => setState(() => _posterMode = !_posterMode)),
              IconButton(icon: const Icon(Icons.refresh), tooltip: '刷新', onPressed: () => _list(_path)),
            ]),
          ),
        ),
        if (_error != null) Padding(padding: const EdgeInsets.all(8), child: Text(_error!, style: const TextStyle(color: Colors.redAccent))),
        Expanded(child: _loading ? const Center(child: CircularProgressIndicator()) : _posterMode ? _gridView() : _listView()),
      ],
      ),
    );
  }

  /// 本地:海报墙(目录为文件夹卡片)。
  Widget _gridView() {
    final entries = _filtered;
    if (entries.isEmpty) return const Center(child: Text('(此目录无漫画)'));
    return GridView.builder(
      padding: const EdgeInsets.all(12),
      gridDelegate: const SliverGridDelegateWithMaxCrossAxisExtent(
        maxCrossAxisExtent: 180,
        childAspectRatio: 0.66,
        crossAxisSpacing: 12,
        mainAxisSpacing: 12,
      ),
      itemCount: entries.length,
      itemBuilder: (context, i) {
        final e = entries[i];
        if (e.isDir) {
          final sel = _selectMode && _selectedPaths.contains(e.path);
          // 选择模式下:文件夹卡片不响应自身点击,由外层 InkWell 接管;
          // 单击=勾选,双击=进入目录
          Widget folderCard = _FolderCard(
            name: e.name,
            onTap: _selectMode ? null : () => _openDir(e.path),
          );
          if (!_selectMode) return folderCard;
          // 使用 GestureDetector 替代 Stack+InkWell,更精确控制手势
          return Stack(children: [
            folderCard,
            Positioned(right: 6, top: 6, child: IgnorePointer(child: Container(
              width: 22, height: 22,
              decoration: BoxDecoration(color: sel ? Colors.blue : Colors.black45, shape: BoxShape.circle, border: Border.all(color: Colors.white38)),
              child: sel ? const Icon(Icons.check, size: 16, color: Colors.white) : null,
            ))),
            Positioned.fill(child: Material(color: Colors.transparent, child: InkWell(
              onTap: () => setState(() => sel ? _selectedPaths.remove(e.path) : _selectedPaths.add(e.path)),
              onDoubleTap: () => _openDir(e.path),
            ))),
          ]);
        }
        final sel = _selectedPaths.contains(e.path);
        final card = ComicCard(
          source: widget.source, path: e.path, title: e.name, subtitle: fmtSize(e.size),
          onTap: _selectMode ? () {} : () => Navigator.of(context).push(MaterialPageRoute(builder: (_) => BookDetailPage(source: widget.source, path: e.path, title: e.name))),
        );
        if (!_selectMode) return card;
        return Stack(children: [
          card,
          Positioned(right: 6, top: 6, child: IgnorePointer(child: Container(
            width: 22, height: 22,
            decoration: BoxDecoration(color: sel ? Colors.blue : Colors.black45, shape: BoxShape.circle, border: Border.all(color: Colors.white38)),
            child: sel ? const Icon(Icons.check, size: 16, color: Colors.white) : null,
          ))),
          Positioned.fill(child: Material(color: Colors.transparent, child: InkWell(
            onTap: () => setState(() => sel ? _selectedPaths.remove(e.path) : _selectedPaths.add(e.path)),
          ))),
        ]);
      },
    );
  }

  /// WebDAV:列表(选择模式下有复选框)。
  Widget _listView() {
    final entries = _filtered;
    if (entries.isEmpty) return const Center(child: Text('(此目录无漫画)'));
    return ListView.builder(
      itemCount: entries.length,
      itemBuilder: (context, i) {
        final e = entries[i];
        final sel = _selectedPaths.contains(e.path);
        if (e.isDir) {
          if (!_selectMode) return ListTile(leading: const Icon(Icons.folder, color: Colors.amber), title: Text(e.name), onTap: () => _openDir(e.path));
          return ListTile(
            leading: Checkbox(value: sel, onChanged: (v) => setState(() => v == true ? _selectedPaths.add(e.path) : _selectedPaths.remove(e.path))),
            title: Text(e.name, maxLines: 1, overflow: TextOverflow.ellipsis),
            trailing: IconButton(icon: const Icon(Icons.arrow_forward_ios, size: 16), onPressed: () => _openDir(e.path)),
            onTap: () => setState(() => sel ? _selectedPaths.remove(e.path) : _selectedPaths.add(e.path)),
          );
        }
        return ListTile(
          leading: _selectMode ? Checkbox(value: sel, onChanged: (v) => setState(() => v == true ? _selectedPaths.add(e.path) : _selectedPaths.remove(e.path))) : Icon(Icons.menu_book),
          title: Text(e.name, maxLines: 1, overflow: TextOverflow.ellipsis),
          subtitle: Text(fmtSize(e.size)),
          onTap: _selectMode ? () => setState(() => sel ? _selectedPaths.remove(e.path) : _selectedPaths.add(e.path)) : () => Navigator.of(context).push(MaterialPageRoute(builder: (_) => BookDetailPage(source: widget.source, path: e.path, title: e.name))),
        );
      },
    );
  }
}

/// 文件夹卡片(海报墙内)。
class _FolderCard extends StatelessWidget {
  final String name;
  final VoidCallback? onTap;

  const _FolderCard({required this.name, this.onTap});

  @override
  Widget build(BuildContext context) {
    return Card(
      clipBehavior: Clip.antiAlias,
      elevation: 3,
      shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(10)),
      child: InkWell(
        onTap: onTap,
        child: Column(
          mainAxisAlignment: MainAxisAlignment.center,
          children: [
            const Icon(Icons.folder, size: 56, color: Colors.amber),
            const SizedBox(height: 8),
            Padding(
              padding: const EdgeInsets.symmetric(horizontal: 6),
              child: Text(name,
                  maxLines: 2, overflow: TextOverflow.ellipsis, textAlign: TextAlign.center),
            ),
          ],
        ),
      ),
    );
  }
}
