// 跨设备资料库搜索（Phase 6.4）。
//
// 结果直接来自 Rust 分页 SQL（dbSearchBooks），展示 设备 / 书源 / path / 标签 / 状态，
// 不整载 library_index；滚动到底加载更多。

import 'package:app/src/rust/api/library.dart' as frb;
import 'package:app/store/library_catalog.dart';
import 'package:app/store/library_store.dart';
import 'package:app/store/models.dart';
import 'package:app/ui/book_detail_page.dart';
import 'package:app/ui/opener.dart';
import 'package:flutter/material.dart';

class GlobalSearchResults extends StatefulWidget {
  final String query;
  final Set<String> tags;
  final bool includeRemote;
  final ValueNotifier<int>? countNotifier;

  const GlobalSearchResults({
    super.key,
    required this.query,
    required this.tags,
    required this.includeRemote,
    this.countNotifier,
  });

  @override
  State<GlobalSearchResults> createState() => _GlobalSearchResultsState();
}

class _GlobalSearchResultsState extends State<GlobalSearchResults> {
  final List<frb.BookSearchDto> _results = [];
  bool _loading = false;
  bool _hasMore = true;
  int _offset = 0;
  static const _page = 100;

  @override
  void initState() {
    super.initState();
    _load(clear: true);
  }

  @override
  void didUpdateWidget(covariant GlobalSearchResults old) {
    super.didUpdateWidget(old);
    if (old.query != widget.query ||
        old.tags.join(',') != widget.tags.join(',') ||
        old.includeRemote != widget.includeRemote) {
      _load(clear: true);
    }
  }

  Future<void> _load({required bool clear}) async {
    if (_loading) return;
    setState(() {
      _loading = true;
      if (clear) {
        _results.clear();
        _offset = 0;
        _hasMore = true;
      }
    });
    try {
      final page = await LibraryCatalogStore.instance.searchBooks(
        query: widget.query,
        tags: widget.tags.toList(),
        includeRemote: widget.includeRemote,
        limit: _page,
        offset: clear ? 0 : _offset,
      );
      if (!mounted) return;
      setState(() {
        _results.addAll(page);
        _offset += page.length;
        _hasMore = page.length == _page;
      });
      widget.countNotifier?.value = _results.length;
    } catch (_) {
      if (mounted) setState(() => _hasMore = false);
    } finally {
      if (mounted) setState(() => _loading = false);
    }
  }

  void _open(BuildContext context, frb.BookSearchDto b) {
    final local = LibraryStore.instance.sourceById(b.sourceId);
    if (b.status == 'index_only' || local == null) {
      Navigator.of(context).push(MaterialPageRoute(
        builder: (_) => BookDetailPage(
          source: BookSource(
            id: b.sourceId,
            type: b.sourceType,
            name: b.sourceName,
            path: b.path,
            remoteOnly: b.isRemote,
            originDeviceId: b.deviceId,
          ),
          path: b.path,
          title: b.title,
        ),
      ));
      return;
    }
    openBook(context, local, b.path, b.title);
  }

  @override
  Widget build(BuildContext context) {
    if (_results.isEmpty) {
      return _loading
          ? const Center(child: CircularProgressIndicator())
          : const Center(
              child: Text('没有匹配的漫画', style: TextStyle(color: Colors.white38)));
    }
    return ListView.builder(
      padding: const EdgeInsets.all(12),
      itemCount: _results.length + (_hasMore ? 1 : 0),
      itemBuilder: (c, i) {
        if (i >= _results.length) {
          return Padding(
            padding: const EdgeInsets.all(8),
            child: _loading
                ? const Center(
                    child: SizedBox(
                      width: 18,
                      height: 18,
                      child: CircularProgressIndicator(strokeWidth: 2),
                    ),
                  )
                : Center(
                    child: TextButton(
                      onPressed: () => _load(clear: false),
                      child: const Text('加载更多', style: TextStyle(fontSize: 12)),
                    ),
                  ),
          );
        }
        final b = _results[i];
        return ListTile(
          dense: true,
          leading: Text(
            LibraryCatalogStore.statusEmoji(b.status),
            style: const TextStyle(fontSize: 16),
          ),
          title: Text(
            b.title,
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            style: const TextStyle(fontSize: 14),
          ),
          subtitle: Text(
            '${b.deviceName} / ${b.sourceName} / ${b.path}'
            '${b.tags.isNotEmpty ? ' · #${b.tags.replaceAll(',', ' #')}' : ''}',
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            style: const TextStyle(fontSize: 11, color: Colors.white54),
          ),
          onTap: () => _open(context, b),
        );
      },
    );
  }
}
