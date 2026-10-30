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
import 'package:app/ui/source_browser.dart';
import 'package:flutter/material.dart';

typedef SourceAction = void Function(frb.SourceAvailabilityDto source);

bool needsRemoteScanStatusFor(frb.SourceAvailabilityDto source) =>
    source.isRemote ||
    const {'webdav', 'smb', 'sftp', 'ftp', 's3', 'baidu', '115', 'quark'}.contains(source.type);

String remoteScanIdleMessageFor(
  frb.SourceAvailabilityDto source, {
  required bool backgroundScanEnabled,
}) {
  if (source.isRemote || source.status == 'index_only') {
    return '仅离线索引，不执行在线扫描';
  }
  if (!backgroundScanEnabled) {
    return '后台扫描已关闭';
  }
  if (source.status == 'needs_network' || !source.hasCredentials) {
    return '等待授权或打开根目录后开始扫描';
  }
  return '等待扫描';
}

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
      listenable: Listenable.merge([
        LibraryCatalogStore.instance,
        LibraryStore.instance,
      ]),
      builder: (context, _) {
        final store = LibraryCatalogStore.instance;
        if (!store.loaded) {
          return const Center(child: CircularProgressIndicator());
        }
        if (store.devices.isEmpty) {
          return Center(
            child: Text(
              '暂无书源',
              style: TextStyle(
                color: Theme.of(context).colorScheme.onSurfaceVariant,
              ),
            ),
          );
        }
        // 同名源消歧：树里只用 `name` 做标题时，直连 115 与它的同步镜像
        // （都叫「115 网盘」）长得一模一样——删掉其中一个后看起来"没反应"。
        // 这里统计重名，重名的书源在标题后补一段短 id。
        final nameCounts = <String, int>{};
        for (final device in store.devices) {
          for (final src in device.sources) {
            nameCounts[src.name] = (nameCounts[src.name] ?? 0) + 1;
          }
        }
        final duplicatedNames = nameCounts.entries
            .where((entry) => entry.value > 1)
            .map((entry) => entry.key)
            .toSet();
        return ListView(
          children: [
            for (var i = 0; i < store.devices.length; i++)
              _DeviceTile(
                device: store.devices[i],
                initiallyExpanded: i == 0,
                duplicatedNames: duplicatedNames,
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

  /// 显示名重复的书源集合（树里标题只显示名字，重名会让"删了一个"看起来没反应）。
  final Set<String> duplicatedNames;
  final SourceAction? onEditSource;
  final SourceAction? onDeleteSource;
  final SourceAction? onShowDetail;

  const _DeviceTile({
    required this.device,
    required this.initiallyExpanded,
    this.duplicatedNames = const <String>{},
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
            duplicatedName: duplicatedNames.contains(src.name),
            onEditSource: onEditSource,
            onDeleteSource: onDeleteSource,
            onShowDetail: onShowDetail,
          ),
      ],
    );
  }
}

class _SourceTile extends StatelessWidget {
  final frb.SourceAvailabilityDto source;

  /// 是否与其它书源同名（同名时标题补短 id 以区分）。
  final bool duplicatedName;
  final SourceAction? onEditSource;
  final SourceAction? onDeleteSource;
  final SourceAction? onShowDetail;

  const _SourceTile({
    required this.source,
    this.duplicatedName = false,
    this.onEditSource,
    this.onDeleteSource,
    this.onShowDetail,
  });

  @override
  Widget build(BuildContext context) {
    final hasMenu =
        onEditSource != null || onDeleteSource != null || onShowDetail != null;
    return ListTile(
      dense: true,
      key: PageStorageKey('source-${source.sourceId}'),
      leading: Icon(
        _isLocalFileSource(source.type)
            ? Icons.folder_outlined
            : Icons.cloud_outlined,
        size: 19,
        color: _isLocalFileSource(source.type)
            ? Theme.of(context).colorScheme.primary
            : Theme.of(context).colorScheme.tertiary,
      ),
      title: Text(
        '${source.name}${duplicatedName ? ' ·#${_shortSourceId(source.sourceId)}' : ''}',
        style: const TextStyle(fontSize: 13),
      ),
      subtitle: Text(
        '${source.comicCount} 本漫画',
        style: TextStyle(
          fontSize: 11,
          color: Theme.of(context).colorScheme.onSurfaceVariant,
        ),
      ),
      // 点开书源 = 打开浏览器（在线浏览或离线索引）。
      onTap: () => _openBrowser(context),
      trailing: hasMenu
          ? PopupMenuButton<String>(
              itemBuilder: (c) => [
                if (onShowDetail != null)
                  const PopupMenuItem(value: 'detail', child: Text('书源详情')),
                if (onEditSource != null)
                  const PopupMenuItem(value: 'edit', child: Text('编辑书源')),
                if (onDeleteSource != null)
                  const PopupMenuItem(value: 'delete', child: Text('删除书源')),
              ],
              onSelected: (act) {
                if (act == 'edit') {
                  onEditSource?.call(source);
                } else if (act == 'detail') {
                  onShowDetail?.call(source);
                } else if (act == 'delete') {
                  onDeleteSource?.call(source);
                }
              },
            )
          : null,
    );
  }

  void _openBrowser(BuildContext context) {
    final src =
        LibraryStore.instance.sourceById(source.sourceId) ??
        BookSource(
          id: source.sourceId,
          type: source.type,
          name: source.name,
          path: source.path,
          remoteOnly: source.isRemote,
          originDeviceId: source.deviceId,
        );
    Navigator.of(context).push(
      MaterialPageRoute(
        builder: (_) => SourceBrowser(source: src, showBack: true),
      ),
    );
  }
}

bool _isLocalFileSource(String type) => type == 'local';

/// 书源 id 的短标识（用于同名书源的标题消歧）。
/// 形如 `sync_62556ad8_1786286465729` → `17862864`；`115_1789360028897` → `17893600`。
String _shortSourceId(String id) {
  final parts = id.split('_');
  final tail = parts.isNotEmpty ? parts.last : id;
  return tail.length <= 8 ? tail : tail.substring(0, 8);
}
