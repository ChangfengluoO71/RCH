// 设备 → 书源 → 漫画 三级树（Phase 6.2）。
//
// 懒加载约束：
// - 设备/书源节点展开才构建子节点（ExpansionTile 惰性）；
// - 漫画列表分页拉取（每页 100，滚动到底加载更多），不整载 library_index 进 Dart 内存。
// 语义全部来自 Rust DTO（status/device/归属），Flutter 只做展示映射。

import 'package:app/src/rust/api/library.dart' as frb;
import 'package:app/store/library_catalog.dart';
import 'package:app/store/library_store.dart';
import 'package:app/store/models.dart';
import 'package:app/ui/book_detail_page.dart';
import 'package:app/ui/opener.dart';
import 'package:app/ui/source_browser.dart';
import 'package:flutter/material.dart';

typedef SourceAction = void Function(frb.SourceAvailabilityDto source);

class SourceTreePanel extends StatelessWidget {
  final SourceAction? onEditSource;
  final SourceAction? onDeleteSource;
  final SourceAction? onShowDetail;

  const SourceTreePanel({
    super.key,
    this.onEditSource,
    this.onDeleteSource,
    this.onShowDetail,
  });

  @override
  Widget build(BuildContext context) {
    return ListenableBuilder(
      listenable: LibraryCatalogStore.instance,
      builder: (context, _) {
        final store = LibraryCatalogStore.instance;
        if (!store.loaded) {
          return const Center(child: CircularProgressIndicator());
        }
        if (store.devices.isEmpty) {
          return const Center(
            child: Text('暂无书源', style: TextStyle(color: Colors.white38)),
          );
        }
        return ListView(
          children: [
            for (var i = 0; i < store.devices.length; i++)
              _DeviceTile(
                device: store.devices[i],
                initiallyExpanded: i == 0,
                onEditSource: onEditSource,
                onDeleteSource: onDeleteSource,
                onShowDetail: onShowDetail,
              ),
          ],
        );
      },
    );
  }
}

class _DeviceTile extends StatelessWidget {
  final frb.SourceTreeNodeDto device;
  final bool initiallyExpanded;
  final SourceAction? onEditSource;
  final SourceAction? onDeleteSource;
  final SourceAction? onShowDetail;

  const _DeviceTile({
    required this.device,
    required this.initiallyExpanded,
    this.onEditSource,
    this.onDeleteSource,
    this.onShowDetail,
  });

  @override
  Widget build(BuildContext context) {
    return ExpansionTile(
      key: PageStorageKey('device-${device.deviceId}'),
      initiallyExpanded: initiallyExpanded,
      leading: const Icon(Icons.devices, size: 20),
      title: Text(
        device.deviceName,
        style: const TextStyle(fontWeight: FontWeight.w600, fontSize: 14),
      ),
      subtitle: Text(
        '${device.sources.length} 个书源',
        style: const TextStyle(fontSize: 11),
      ),
      children: [
        for (final src in device.sources)
          _SourceTile(
            source: src,
            onEditSource: onEditSource,
            onDeleteSource: onDeleteSource,
            onShowDetail: onShowDetail,
          ),
      ],
    );
  }
}

class _SourceTile extends StatefulWidget {
  final frb.SourceAvailabilityDto source;
  final SourceAction? onEditSource;
  final SourceAction? onDeleteSource;
  final SourceAction? onShowDetail;

  const _SourceTile({
    required this.source,
    this.onEditSource,
    this.onDeleteSource,
    this.onShowDetail,
  });

  @override
  State<_SourceTile> createState() => _SourceTileState();
}

class _SourceTileState extends State<_SourceTile> {
  bool _expanded = false;

  @override
  Widget build(BuildContext context) {
    final source = widget.source;
    final hasMenu = widget.onEditSource != null ||
        widget.onDeleteSource != null ||
        widget.onShowDetail != null;
    return Column(children: [
      ListTile(
        dense: true,
        key: PageStorageKey('source-${source.sourceId}'),
        leading: const Icon(Icons.folder_outlined, size: 18),
        title: Text(
          '${LibraryCatalogStore.statusEmoji(source.status)} ${source.name}',
          style: const TextStyle(fontSize: 13),
        ),
        subtitle: Text(
          '${LibraryCatalogStore.statusLabel(source.status)} · ${source.offlineIndexCount} 本'
          '${source.isRemote ? ' · 远端' : ''}',
          style: const TextStyle(fontSize: 11, color: Colors.white54),
        ),
        // 点开书源 = 打开浏览器（在线浏览或离线索引），恢复"点开书源看漫画"。
        onTap: () => _openBrowser(context),
        trailing: Row(mainAxisSize: MainAxisSize.min, children: [
          IconButton(
            icon: Icon(_expanded ? Icons.expand_less : Icons.expand_more),
            tooltip: '离线书目',
            visualDensity: VisualDensity.compact,
            onPressed: () => setState(() => _expanded = !_expanded),
          ),
          if (hasMenu)
            PopupMenuButton<String>(
              itemBuilder: (c) => [
                if (widget.onShowDetail != null)
                  const PopupMenuItem(value: 'detail', child: Text('书源详情')),
                if (widget.onEditSource != null)
                  const PopupMenuItem(value: 'edit', child: Text('编辑书源')),
                if (widget.onDeleteSource != null)
                  const PopupMenuItem(value: 'delete', child: Text('删除书源')),
              ],
              onSelected: (act) {
                if (act == 'edit') {
                  widget.onEditSource?.call(source);
                } else if (act == 'detail') {
                  widget.onShowDetail?.call(source);
                } else if (act == 'delete') {
                  widget.onDeleteSource?.call(source);
                }
              },
            ),
        ]),
      ),
      if (_expanded)
        Padding(
          padding: const EdgeInsets.only(left: 16),
          child: _SourceBooksList(source: source),
        ),
    ]);
  }

  void _openBrowser(BuildContext context) {
    final source = widget.source;
    final src = LibraryStore.instance.sourceById(source.sourceId) ??
        BookSource(
          id: source.sourceId,
          type: source.type,
          name: source.name,
          path: source.path,
          remoteOnly: source.isRemote,
          originDeviceId: source.deviceId,
        );
    Navigator.of(context).push(MaterialPageRoute(
      builder: (_) => SourceBrowser(source: src, showBack: true),
    ));
  }
}

/// 书源下漫画：分页懒加载。
class _SourceBooksList extends StatefulWidget {
  final frb.SourceAvailabilityDto source;

  const _SourceBooksList({required this.source});

  @override
  State<_SourceBooksList> createState() => _SourceBooksListState();
}

class _SourceBooksListState extends State<_SourceBooksList> {
  final List<frb.BookSearchDto> _books = [];
  bool _loading = false;
  bool _hasMore = true;
  int _offset = 0;
  static const _page = 100;

  @override
  void initState() {
    super.initState();
    _loadMore();
  }

  Future<void> _loadMore() async {
    if (_loading || !_hasMore) return;
    setState(() => _loading = true);
    try {
      final page = await LibraryCatalogStore.instance.sourceBooks(
        sourceId: widget.source.sourceId,
        limit: _page,
        offset: _offset,
      );
      if (!mounted) return;
      setState(() {
        _books.addAll(page);
        _offset += page.length;
        _hasMore = page.length == _page;
      });
    } catch (_) {
      if (mounted) setState(() => _hasMore = false);
    } finally {
      if (mounted) setState(() => _loading = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    if (_books.isEmpty) {
      return Padding(
        padding: const EdgeInsets.all(12),
        child: Text(
          _loading ? '加载中…' : '暂无漫画索引',
          style: const TextStyle(fontSize: 12, color: Colors.white38),
        ),
      );
    }
    return ListView.builder(
      shrinkWrap: true,
      physics: const NeverScrollableScrollPhysics(),
      itemCount: _books.length + (_hasMore ? 1 : 0),
      itemBuilder: (c, i) {
        if (i >= _books.length) {
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
                      onPressed: _loadMore,
                      child: const Text('加载更多', style: TextStyle(fontSize: 12)),
                    ),
                  ),
          );
        }
        final b = _books[i];
        return ListTile(
          dense: true,
          contentPadding: const EdgeInsets.symmetric(horizontal: 12),
          title: Text(
            '${LibraryCatalogStore.statusEmoji(b.status)} ${b.title}',
            style: const TextStyle(fontSize: 13),
          ),
          subtitle: Text(
            b.path,
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            style: const TextStyle(fontSize: 10, color: Colors.white38),
          ),
          onTap: () => _openBook(context, b),
        );
      },
    );
  }

  void _openBook(BuildContext context, frb.BookSearchDto b) {
    final local = LibraryStore.instance.sourceById(b.sourceId);
    if (b.status == 'index_only' || local == null) {
      // ⚪ 仅索引：详情页可编辑元数据，不尝试读取（远端源置 remoteOnly 显示只读横幅）
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
}
