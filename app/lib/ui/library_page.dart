import 'dart:ui' as ui;

import 'package:app/src/rust/api/book.dart';
import 'package:app/ui/common.dart';
import 'package:app/ui/reader_page.dart';
import 'package:app/ui/webdav_page.dart';
import 'package:flutter/material.dart';

/// 书架:海报墙(网格 + 封面缩略图)。
class LibraryPage extends StatefulWidget {
  const LibraryPage({super.key});

  @override
  State<LibraryPage> createState() => _LibraryPageState();
}

class _LibraryPageState extends State<LibraryPage> {
  final TextEditingController _dirCtrl = TextEditingController(
    text: r'D:\Projects\RCH-source\testdata',
  );
  List<DirEntry> _entries = [];
  String? _error;
  bool _loading = false;
  final Map<String, Future<ui.Image>> _covers = {};

  @override
  void initState() {
    super.initState();
    _refresh();
  }

  Future<void> _refresh() async {
    setState(() {
      _loading = true;
      _error = null;
    });
    try {
      final list = await listLocalDir(path: _dirCtrl.text.trim());
      setState(() {
        _entries = list
            .where((e) =>
                e.isDir ||
                e.name.toLowerCase().endsWith('.cbz') ||
                e.name.toLowerCase().endsWith('.zip'))
            .toList();
      });
    } catch (e) {
      setState(() => _error = '$e');
    } finally {
      setState(() => _loading = false);
    }
  }

  Future<ui.Image> _cover(String path) {
    return _covers.putIfAbsent(
      path,
      () async {
        final p = await bookCover(path: path, page: 0, width: 340, height: 480);
        return rgbaToImage(p.rgba, p.width, p.height);
      },
    );
  }

  void _open(DirEntry e) {
    if (e.isDir) {
      _dirCtrl.text = e.path;
      _refresh();
    } else {
      Navigator.of(context).push(
        MaterialPageRoute(builder: (_) => ReaderPage(path: e.path, title: e.name)),
      );
    }
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(
        title: const Text('RCH 书架'),
        actions: [
          IconButton(
            icon: const Icon(Icons.cloud),
            tooltip: 'WebDAV 书源',
            onPressed: () => Navigator.of(context).push(
              MaterialPageRoute(builder: (_) => const WebDavPage()),
            ),
          ),
        ],
      ),
      body: Column(
        children: [
          Padding(
            padding: const EdgeInsets.fromLTRB(12, 8, 12, 0),
            child: Row(
              children: [
                Expanded(
                  child: TextField(
                    controller: _dirCtrl,
                    decoration: const InputDecoration(
                      labelText: '目录路径',
                      border: OutlineInputBorder(),
                      isDense: true,
                    ),
                    onSubmitted: (_) => _refresh(),
                  ),
                ),
                const SizedBox(width: 8),
                ElevatedButton(onPressed: _refresh, child: const Text('浏览')),
              ],
            ),
          ),
          if (_error != null)
            Padding(
              padding: const EdgeInsets.all(8),
              child: Text('错误: $_error', style: const TextStyle(color: Colors.redAccent)),
            ),
          Expanded(
            child: _loading
                ? const Center(child: CircularProgressIndicator())
                : GridView.builder(
                    padding: const EdgeInsets.all(12),
                    gridDelegate: const SliverGridDelegateWithMaxCrossAxisExtent(
                      maxCrossAxisExtent: 180,
                      childAspectRatio: 0.66,
                      crossAxisSpacing: 12,
                      mainAxisSpacing: 12,
                    ),
                    itemCount: _entries.length,
                    itemBuilder: (context, i) {
                      final e = _entries[i];
                      return _EntryCard(
                        entry: e,
                        coverFuture: e.isDir ? null : _cover(e.path),
                        onTap: () => _open(e),
                      );
                    },
                  ),
          ),
        ],
      ),
    );
  }
}

/// 海报墙卡片:文件夹或漫画封面。
class _EntryCard extends StatelessWidget {
  final DirEntry entry;
  final Future<ui.Image>? coverFuture;
  final VoidCallback onTap;

  const _EntryCard({required this.entry, this.coverFuture, required this.onTap});

  @override
  Widget build(BuildContext context) {
    return Card(
      clipBehavior: Clip.antiAlias,
      elevation: 3,
      shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(10)),
      child: InkWell(
        onTap: onTap,
        child: entry.isDir ? _folder() : _book(context),
      ),
    );
  }

  Widget _folder() {
    return Column(
      mainAxisAlignment: MainAxisAlignment.center,
      children: [
        const Icon(Icons.folder, size: 56, color: Colors.amber),
        const SizedBox(height: 8),
        Padding(
          padding: const EdgeInsets.symmetric(horizontal: 6),
          child: Text(entry.name, maxLines: 2, overflow: TextOverflow.ellipsis, textAlign: TextAlign.center),
        ),
      ],
    );
  }

  Widget _book(BuildContext context) {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Expanded(
          child: FutureBuilder<ui.Image>(
            future: coverFuture,
            builder: (context, snap) {
              if (snap.hasData) {
                return RawImage(
                  image: snap.data,
                  fit: BoxFit.cover,
                  width: double.infinity,
                  height: double.infinity,
                );
              }
              return Container(
                color: Colors.black26,
                child: Center(
                  child: snap.hasError
                      ? const Icon(Icons.broken_image, color: Colors.white38)
                      : const SizedBox(
                          width: 22, height: 22,
                          child: CircularProgressIndicator(strokeWidth: 2),
                        ),
                ),
              );
            },
          ),
        ),
        Container(
          color: Colors.black45,
          padding: const EdgeInsets.fromLTRB(6, 5, 6, 6),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(
                entry.name,
                maxLines: 2,
                overflow: TextOverflow.ellipsis,
                style: const TextStyle(fontSize: 12, height: 1.2),
              ),
              const SizedBox(height: 2),
              Text(fmtSize(entry.size),
                  style: const TextStyle(fontSize: 10, color: Colors.white54)),
            ],
          ),
        ),
      ],
    );
  }
}
