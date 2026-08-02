import 'dart:async';
import 'dart:io';

import 'package:app/src/rust/api/book.dart';
import 'package:app/src/rust/api/export.dart';
import 'package:app/src/rust/api/source.dart';
import 'package:app/store/library_store.dart';
import 'package:app/store/models.dart';
import 'package:app/ui/book_detail_page.dart';
import 'package:app/ui/comic_cover.dart';
import 'package:app/ui/common.dart';
import 'package:app/store/webdav_session.dart';
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
  String _sort = 'alpha'; // alpha=按字母, added=按加入时间(最新在前)
  bool _converting = false; // 自动转 CBZ 执行中（防并发触发）
  bool _showConvertProgress = false; // 底部转换进度条可见
  int _convertDone = 0;
  int _convertTotal = 0;
  String _convertCurrent = '';
  bool _convertCancelled = false; // 用户点击取消

  /// 漫画文件夹检测结果：path → true 表示该目录是漫画文件夹。
  /// 本地模式使用；WebDAV 暂不检测（避免大量网络请求）。
  final Map<String, bool> _comicDirs = {};

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
      _comicDirs.clear();
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
      // 本地模式：异步检测子目录是否为漫画文件夹
      if (!widget.source.isWebDav) {
        _detectComicFolders();
      }
    } catch (e) {
      if (mounted) setState(() => _error = '$e');
    } finally {
      if (mounted) setState(() => _loading = false);
    }
  }

  /// 异步检测当前列表中的子目录是否为漫画文件夹。
  void _detectComicFolders() {
    for (final e in _entries) {
      if (!e.isDir) continue;
      // 逐目录检测（非并发，避免 IO 抖动）
      unawaited(isComicFolder(dirPath: e.path).then((isComic) {
        if (!mounted) return;
        setState(() => _comicDirs[e.path] = isComic);
      }));
    }
  }

  void _openDir(String path) {
    _stack.add(_path);
    _list(path);
  }

  void _goUp() {
    if (_stack.isNotEmpty) _list(_stack.removeLast());
  }

  /// 刷新：重新列出目录；本地来源且开启"自动转 CBZ"时，后台转换后再次列出。
  Future<void> _refresh() async {
    await _list(_path);
    if (!mounted || widget.source.isWebDav) return;
    await _autoConvertToCbz();
    if (mounted) await _list(_path);
  }

  /// 后台自动转 CBZ：漫画文件夹 → `name.cbz`；zip → `stem.cbz`。
  /// 转换产物与原路径 key 一致（见 normalizeComicPath），进度/标签自动延续。
  Future<void> _autoConvertToCbz() async {
    if (_converting) return;
    if (!LibraryStore.instance.settings.autoConvertCbz) return;
    _converting = true;
    _convertCancelled = false;
    try {
      final tasks = <String>[]; // 目标 cbz 路径
      final targets = <String, String>{}; // 目标 → 源
      for (final e in _entries) {
        final dirTarget = '${e.path}.cbz';
        if (e.isDir) {
          if (!await isComicFolder(dirPath: e.path)) continue;
          if (!File(dirTarget).existsSync()) {
            tasks.add(dirTarget);
            targets[dirTarget] = e.path;
          }
        } else if (e.name.toLowerCase().endsWith('.zip')) {
          final target = '${e.path.substring(0, e.path.length - 4)}.cbz';
          if (!File(target).existsSync()) {
            tasks.add(target);
            targets[target] = e.path;
          }
        }
      }
      if (tasks.isEmpty) return;
      _convertTotal = tasks.length;
      _convertDone = 0;
      if (mounted) setState(() => _showConvertProgress = true);
      for (final target in tasks) {
        if (_convertCancelled) break;
        _convertCurrent = target.split(Platform.pathSeparator).last;
        if (mounted) setState(() {});
        try {
          final src = targets[target]!;
          if (Directory(src).existsSync()) {
            await exportFolderToCbz(srcDir: src, outPath: target);
          } else {
            await exportZipAsCbz(srcPath: src, outPath: target);
          }
        } catch (_) {
          // 单个失败不中断其余转换
        }
        _convertDone++;
        if (mounted) setState(() {});
      }
      if (mounted) {
        setState(() => _showConvertProgress = false);
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(
            content: Text(_convertCancelled
                ? '已取消转换（完成 $_convertDone/$_convertTotal 项）'
                : 'CBZ 转换完成：$_convertDone/$_convertTotal 项'),
          ),
        );
      }
    } finally {
      _converting = false;
    }
  }

  /// 底部转换进度条（非模态，转换期间不阻塞浏览）。
  Widget _convertProgressBar() => Material(
        color: Colors.black87,
        elevation: 6,
        child: Padding(
          padding: const EdgeInsets.fromLTRB(16, 10, 8, 10),
          child: Row(children: [
            Expanded(
              child: Column(
                mainAxisSize: MainAxisSize.min,
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    '正在转 CBZ：$_convertDone/$_convertTotal（$_convertCurrent）',
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: const TextStyle(fontSize: 13),
                  ),
                  const SizedBox(height: 6),
                  LinearProgressIndicator(
                    value: _convertTotal == 0
                        ? null
                        : _convertDone / _convertTotal,
                  ),
                ],
              ),
            ),
            IconButton(
              icon: const Icon(Icons.close, size: 18),
              tooltip: '取消转换',
              onPressed: () => setState(() {
                _convertCancelled = true;
                _showConvertProgress = false;
              }),
            ),
          ]),
        ),
      );

  /// 已被同名 .cbz 取代的源条目（文件夹 / zip）不再显示，避免书架重复。
  bool _isConvertedOriginal(DirEntry e) {
    if (e.isDir) {
      if (_comicDirs[e.path] != true) return false;
      return File('${e.path}.cbz').existsSync();
    }
    if (e.name.toLowerCase().endsWith('.zip')) {
      return File('${e.path.substring(0, e.path.length - 4)}.cbz').existsSync();
    }
    return false;
  }

  /// 漫画文件夹也作为漫画条目参与过滤。
  bool _isComicDir(String path) => _comicDirs[path] == true;

  /// 排序：目录保持在前；按字母用自然序（数字感知），按加入时间用 mtime 降序（最新在前，
  /// mtime=0 视为最旧排最后，同值按名称兜底）。
  int _compareEntries(DirEntry a, DirEntry b) {
    if (a.isDir != b.isDir) return a.isDir ? -1 : 1;
    if (_sort == 'added') {
      final ma = a.mtime > 0 ? a.mtime : -1;
      final mb = b.mtime > 0 ? b.mtime : -1;
      if (ma != mb) return mb.compareTo(ma);
    }
    return _naturalCompare(a.name, b.name);
  }

  List<DirEntry> get _filtered {
    Iterable<DirEntry> list = _entries;
    // 隐藏已被自动转换为同名 .cbz 的源条目
    list = list.where((e) => !_isConvertedOriginal(e));
    // 搜索
    if (widget.search.isNotEmpty) {
      final q = widget.search.toLowerCase();
      list = list.where((e) => e.name.toLowerCase().contains(q));
    }
    // 标签过滤(交集) — 仅对漫画条目生效（普通文件夹不受影响）
    if (widget.selectedTags.isNotEmpty) {
      final store = LibraryStore.instance;
      list = list.where((e) {
        // 普通文件夹（非漫画）不过滤
        if (e.isDir && !_isComicDir(e.path)) return true;
        final newKey = bookKeyOf(widget.source.type, widget.source.id, e.path);
        final legacyKey = '${widget.source.id}|${e.path}';
        final meta = store.metas[newKey] ?? store.metas[legacyKey];
        return meta != null && widget.selectedTags.every((t) => meta.tags.contains(t) || meta.metaTags.contains(t));
      });
    }
    return list.toList()..sort(_compareEntries);
  }

  /// 名称自然排序（数字感知）：file2 < file10。
  static int _naturalCompare(String a, String b) {
    final ra = _naturalSegments(a.toLowerCase());
    final rb = _naturalSegments(b.toLowerCase());
    final n = ra.length < rb.length ? ra.length : rb.length;
    for (var i = 0; i < n; i++) {
      final sa = ra[i], sb = rb[i];
      final na = int.tryParse(sa), nb = int.tryParse(sb);
      if (na != null && nb != null) {
        if (na != nb) return na.compareTo(nb);
      } else {
        final c = sa.compareTo(sb);
        if (c != 0) return c;
      }
    }
    return a.toLowerCase().compareTo(b.toLowerCase());
  }

  static List<String> _naturalSegments(String s) =>
      RegExp(r'\d+|\D+').allMatches(s).map((m) => m.group(0)!).toList();

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
            // 检测子目录是否为漫画文件夹
            if (widget.source.isWebDav) {
              pending.add(e.path);
            } else {
              final isComic = await isComicFolder(dirPath: e.path);
              if (isComic) {
                result.add(e.path);
              } else {
                pending.add(e.path);
              }
            }
          } else if ([
            '.cbz', '.zip', '.epub', '.cb7', '.7z', '.cbt', '.tar',
            '.pdf', '.cbr', '.rar', '.mobi', '.azw', '.azw3'
          ].any((ext) => e.name.toLowerCase().endsWith(ext))) {
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
    if (!mounted) return;
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
      builder: (context, _) => Stack(children: [
      Column(
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
                TextButton(onPressed: () => setState(() { for (var e in _filtered) { _selectedPaths.add(e.path); } }), child: const Text('全选')),
                IconButton(icon: const Icon(Icons.label), tooltip: '批量打标签', onPressed: _batchTagFromSelection),
                IconButton(icon: const Icon(Icons.close), tooltip: '退出选择', onPressed: () => setState(() { _selectedPaths.clear(); _selectMode = false; })),
              ] else ...[
                IconButton(
                  icon: const Icon(Icons.checklist),
                  tooltip: '进入选择模式',
                  onPressed: () => setState(() => _selectMode = true),
                ),
              ],
              PopupMenuButton<String>(
                icon: const Icon(Icons.sort),
                tooltip: '排序',
                initialValue: _sort,
                onSelected: (v) => setState(() => _sort = v),
                itemBuilder: (c) => const [
                  PopupMenuItem(value: 'alpha', child: Text('按字母')),
                  PopupMenuItem(value: 'added', child: Text('按加入时间')),
                ],
              ),
              IconButton(icon: Icon(_posterMode ? Icons.view_list : Icons.grid_view), tooltip: _posterMode ? '切换为简略列表' : '切换为海报墙', onPressed: () => setState(() => _posterMode = !_posterMode)),
              IconButton(icon: const Icon(Icons.refresh), tooltip: '刷新', onPressed: _refresh),
            ]),
          ),
        ),
        if (_error != null) Padding(padding: const EdgeInsets.all(8), child: Text(_error!, style: const TextStyle(color: Colors.redAccent))),
        Expanded(child: _loading ? const Center(child: CircularProgressIndicator()) : _posterMode ? _gridView() : _listView()),
      ],
    ),
    if (_showConvertProgress)
      Positioned(left: 0, right: 0, bottom: 0, child: _convertProgressBar()),
    ]),
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
        // 目录：区分漫画文件夹 vs 普通文件夹
        if (e.isDir) {
          if (_isComicDir(e.path)) {
            // 漫画文件夹 → 显示为海报卡片，点击进详情而非下钻
            return _comicFolderCard(e);
          }
          // 普通文件夹 → 现有文件夹卡片
          final sel = _selectMode && _selectedPaths.contains(e.path);
          Widget folderCard = _FolderCard(
            name: e.name,
            onTap: _selectMode ? null : () => _openDir(e.path),
          );
          if (!_selectMode) return folderCard;
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
        // 普通漫画文件
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

  /// 漫画文件夹卡片: 带封面（优先 cover.jpg → 首页缩略图），点击进详情。
  Widget _comicFolderCard(DirEntry e) {
    final sel = _selectMode && _selectedPaths.contains(e.path);
    final card = _ComicFolderCoverCard(
      source: widget.source,
      dirPath: e.path,
      name: e.name,
      onTap: _selectMode
          ? () {}
          : () => Navigator.of(context).push(MaterialPageRoute(
              builder: (_) => BookDetailPage(
                  source: widget.source, path: e.path, title: e.name))),
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

/// 漫画文件夹封面卡片：优先用 cover.jpg，无封面则取首页做缩略图。
class _ComicFolderCoverCard extends StatefulWidget {
  final BookSource source;
  final String dirPath;
  final String name;
  final VoidCallback onTap;

  const _ComicFolderCoverCard({
    required this.source,
    required this.dirPath,
    required this.name,
    required this.onTap,
  });

  @override
  State<_ComicFolderCoverCard> createState() => _ComicFolderCoverCardState();
}

class _ComicFolderCoverCardState extends State<_ComicFolderCoverCard> {
  /// null = 未加载；"" = 无显式封面（用首页）；非空 = 封面路径。
  String? _coverPath;
  bool _loadingCover = false;

  @override
  void initState() {
    super.initState();
    _detectCover();
  }

  Future<void> _detectCover() async {
    if (_loadingCover) return;
    _loadingCover = true;
    try {
      final cp = await folderCoverPath(dirPath: widget.dirPath);
      if (!mounted) return;
      setState(() => _coverPath = cp);
    } catch (_) {
      if (mounted) setState(() => _coverPath = '');
    }
  }

  @override
  Widget build(BuildContext context) {
    return Card(
      clipBehavior: Clip.antiAlias,
      elevation: 3,
      shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(10)),
      child: InkWell(
        onTap: widget.onTap,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Expanded(
              child: _buildCover(),
            ),
            Container(
              color: Colors.black45,
              padding: const EdgeInsets.fromLTRB(6, 5, 6, 6),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Row(
                    children: [
                      const Icon(Icons.folder, size: 14, color: Colors.amber),
                      const SizedBox(width: 4),
                      Expanded(
                        child: Text(
                          widget.name,
                          maxLines: 2,
                          overflow: TextOverflow.ellipsis,
                          style: const TextStyle(fontSize: 12, height: 1.2),
                        ),
                      ),
                    ],
                  ),
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _buildCover() {
    // 有显式封面 → 优先用封面路径解码（第 0 页）
    if (_coverPath != null && _coverPath!.isNotEmpty) {
      return _loadCover(_coverPath!);
    }
    // 无显式封面 → 用漫画目录首页
    if (_coverPath == '') {
      return _loadCover(widget.dirPath);
    }
    // 还在检测中 → 占位
    return Container(
      color: Colors.black26,
      child: const Center(
        child: SizedBox(
          width: 22,
          height: 22,
          child: CircularProgressIndicator(strokeWidth: 2),
        ),
      ),
    );
  }

  Widget _loadCover(String path) {
    return ComicCover(
      source: widget.source,
      path: path,
      force: true,
    );
  }
}
