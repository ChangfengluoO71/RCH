import 'dart:async';
import 'dart:io';

import 'package:app/src/rust/api/cache.dart';
import 'package:app/store/library_store.dart';
import 'package:app/ui/comic_cover.dart';
import 'package:app/ui/common.dart';
import 'package:flutter/material.dart';

/// 缓存管理面板：展示7类缓存各占空间、独立清理按钮、自定义缓存目录。
class CacheManagerPanel extends StatefulWidget {
  const CacheManagerPanel({super.key});
  @override
  State<CacheManagerPanel> createState() => _CacheManagerPanelState();
}

class _CacheManagerPanelState extends State<CacheManagerPanel> {
  CacheSize? _sizes;
  bool _loading = true;

  @override
  void initState() {
    super.initState();
    _refresh();
  }

  Future<void> _refresh() async {
    setState(() => _loading = true);
    _sizes = await cacheSizes();
    if (mounted) setState(() => _loading = false);
  }

  Future<void> _clear(String label, Future<BigInt> Function() fn, {bool clearCoverMemory = false}) async {
    if (!mounted) return;
    final ok = await showDialog<bool>(
      context: context,
      builder: (c) => AlertDialog(
        title: const Text('确认清理'),
        content: Text('确定要$label吗？'),
        actions: [
          TextButton(onPressed: () => Navigator.of(c).pop(false), child: const Text('取消')),
          FilledButton(onPressed: () => Navigator.of(c).pop(true), child: const Text('确定')),
        ],
      ),
    );
    if (ok != true) return;
    final freed = await fn();
    if (clearCoverMemory) ComicCover.clear();
    if (mounted) _snack('已$label (释放 ${fmtSize(freed)})');
    await _refresh();
  }

  Future<void> _changeCacheDir() async {
    final current = await cacheRootPath();
    final defaultPath = await defaultCacheRootPath();
    if (!mounted) return;

    final controller = TextEditingController(text: current);
    final result = await showDialog<String>(
      context: context,
      builder: (c) => AlertDialog(
        title: const Text('自定义缓存目录'),
        content: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text('当前: $current',
                style: const TextStyle(fontSize: 12, color: Colors.white54)),
            const SizedBox(height: 8),
            TextField(
              controller: controller,
              decoration: InputDecoration(
                labelText: '新缓存目录路径',
                hintText: '留空恢复默认 ($defaultPath)',
                border: const OutlineInputBorder(),
              ),
            ),
            const SizedBox(height: 8),
            Text(
              '修改后需要将旧目录内容手动迁移到新目录，应用不会自动迁移。',
              style: TextStyle(fontSize: 11, color: Colors.orange.shade300),
            ),
          ],
        ),
        actions: [
          TextButton(onPressed: () => Navigator.of(c).pop(), child: const Text('取消')),
          FilledButton(
            onPressed: () => Navigator.of(c).pop(controller.text.trim()),
            child: const Text('确定'),
          ),
        ],
      ),
    );
    if (result == null || !mounted) return;

    // 检查目录是否存在/可创建
    final newPath = result.isEmpty ? '' : result;
    if (newPath.isNotEmpty) {
      final dir = Directory(newPath);
      if (!await dir.exists()) {
        try {
          await dir.create(recursive: true);
        } catch (e) {
          if (mounted) _snack('无法创建目录: $e');
          return;
        }
      }
    }

    await setCacheRootPath(path: newPath);

    // 持久化设置
    final store = LibraryStore.instance;
    store.settings.cacheDir = newPath.isEmpty ? null : newPath;
    store.updateSettings(store.settings);

    if (mounted) {
      _snack(newPath.isEmpty ? '已恢复默认缓存目录' : '缓存目录已更改为: $newPath');
      await _refresh();
    }
  }

  void _snack(String msg) {
    if (!mounted) return;
    ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text(msg)));
  }

  @override
  Widget build(BuildContext context) {
    return Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
      const Text('缓存管理', style: TextStyle(fontSize: 16, fontWeight: FontWeight.w600)),
      const SizedBox(height: 4),
      Text('各缓存分类独立管理，不影响已阅读漫画的封面缓存',
          style: Theme.of(context).textTheme.bodySmall),
      const SizedBox(height: 12),
      if (_loading)
        const Center(child: Padding(padding: EdgeInsets.all(20), child: CircularProgressIndicator()))
      else ...[
        if (_sizes != null) ...[
          Text('磁盘总占用: ${fmtSize(_sizes!.total)}',
              style: const TextStyle(fontSize: 13, color: Colors.white54)),
          const SizedBox(height: 10),
          _cacheRow(Icons.download, '页面缓存（page/）', _sizes!.page,
              'L2 磁盘缓存，复用后秒开',
              () => _clear('清空页面缓存', clearPageCache)),
          _cacheRow(Icons.cloud_download, '整本下载（raw/）', _sizes!.raw,
              'WebDAV 下载的完整漫画文件',
              () => _clear('清空整本下载缓存', clearRawCache)),
          _cacheRow(Icons.image, '封面缩略图（cover/）', _sizes!.cover,
              '海报墙封面图片缓存',
              () => _clear('清空封面缓存', clearCoverCache, clearCoverMemory: true)),
          _cacheRow(Icons.photo_library, '缩略图（thumb/）', _sizes!.thumb,
              '列表缩略图缓存',
              () => _clear('清空缩略图缓存', clearThumbCache)),
          _cacheRow(Icons.folder_copy, '旧下载目录（download/）', _sizes!.download,
              '旧版下载回退目录',
              () => _clear('清空旧下载缓存', clearDownloadCache)),
          _cacheRow(Icons.auto_awesome, 'AI 超分结果（ai/）', _sizes!.ai,
              'AI 超分输出缓存（暂未启用）',
              () => _clear('清空 AI 缓存', clearAiCache)),
          _cacheRow(Icons.storage, '临时文件（temp/）', _sizes!.temp,
              'CB7/CBR 解压中间产物',
              () => _clear('清空临时文件', clearTempCache)),
        ],
        const SizedBox(height: 12),
        SizedBox(
          width: double.infinity,
          child: OutlinedButton.icon(
            onPressed: () => _clear('清空全部缓存', clearAllCaches, clearCoverMemory: true),
            icon: const Icon(Icons.delete_sweep, size: 18),
            label: const Text('清空全部缓存'),
          ),
        ),
        const SizedBox(height: 8),
        FutureBuilder<String>(
          future: cacheRootPath(),
          builder: (c, sn) => Column(children: [
            Text('目录: ${sn.data ?? ''}',
                style: const TextStyle(fontSize: 11, color: Colors.white38)),
            const SizedBox(height: 8),
            SizedBox(
              width: double.infinity,
              child: OutlinedButton.icon(
                onPressed: _changeCacheDir,
                icon: const Icon(Icons.folder_open, size: 18),
                label: const Text('更改缓存目录'),
              ),
            ),
            const SizedBox(height: 4),
            SizedBox(
              width: double.infinity,
              child: OutlinedButton.icon(
                onPressed: () {
                  final r = LibraryStore.instance.purgeStaleRecords();
                  _snack('已清理 $r 条失效记录');
                },
                icon: const Icon(Icons.cleaning_services, size: 18),
                label: const Text('清理失效漫画记录'),
              ),
            ),
          ]),
        ),
      ],
    ]);
  }

  Widget _cacheRow(IconData icon, String label, BigInt size, String hint, VoidCallback onClear) {
    return Padding(
      padding: const EdgeInsets.only(bottom: 4),
      child: ListTile(
        dense: true,
        contentPadding: EdgeInsets.zero,
        leading: Icon(icon, size: 20, color: size > BigInt.zero ? Colors.lightBlueAccent : Colors.white38),
        title: Text(label, style: const TextStyle(fontSize: 13)),
        subtitle: Text(hint, style: const TextStyle(fontSize: 11, color: Colors.white54)),
        trailing: Row(mainAxisSize: MainAxisSize.min, children: [
          Text(fmtSize(size), style: const TextStyle(fontSize: 12, color: Colors.white54)),
          const SizedBox(width: 8),
          OutlinedButton(onPressed: onClear, child: const Text('清理', style: TextStyle(fontSize: 12))),
        ]),
      ),
    );
  }
}
