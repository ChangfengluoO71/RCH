import 'dart:async';
import 'dart:io';

import 'package:app/src/rust/api/cache.dart';
import 'package:app/src/rust/api/db.dart';
import 'package:app/store/library_store.dart';
import 'package:app/store/cache_root_marker.dart';
import 'package:app/ui/comic_cover.dart';
import 'package:app/ui/common.dart';
import 'package:file_selector/file_selector.dart';
import 'package:flutter/material.dart';
import 'package:path_provider/path_provider.dart';

/// 缓存管理面板：展示5类缓存各占空间、独立清理按钮、自定义缓存目录。
class CacheManagerPanel extends StatefulWidget {
  const CacheManagerPanel({super.key});
  @override
  State<CacheManagerPanel> createState() => _CacheManagerPanelState();
}

class _CacheManagerPanelState extends State<CacheManagerPanel> {
  CacheSize? _sizes;
  bool _loading = true;
  bool _purging = false;

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

  Future<void> _clear(String label, Future<BigInt> Function() fn,
      {bool clearCoverMemory = false}) async {
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

  /// 「清空全部缓存」：缓存与阅读数据分开确认——最近阅读记录、阅读统计
  /// 各自独立征得同意后才清空对应的内容（数据不同源：清记录会连带清统计，
  /// 清统计仅阅读次数归零、保留列表与进度）。
  Future<void> _clearAllWithChoices() async {
    if (!mounted) return;
    var clearRecent = false;
    var clearStats = false;
    final ok = await showDialog<bool>(
      context: context,
      builder: (c) => StatefulBuilder(builder: (c, setDlgState) => AlertDialog(
        title: const Text('确认清理'),
        content: Column(mainAxisSize: MainAxisSize.min, children: [
          const Padding(padding: EdgeInsets.only(bottom: 6), child: Text('确定要清空全部缓存吗？阅读数据请分别确认：')),
          CheckboxListTile(
            value: clearRecent,
            dense: true,
            onChanged: (v) => setDlgState(() => clearRecent = v ?? false),
            title: const Text('同时清空最近阅读记录'),
            subtitle: const Text('删除最近阅读/最多阅读列表与阅读进度；阅读统计同源于记录，会一并清空'),
          ),
          CheckboxListTile(
            value: clearStats,
            dense: true,
            onChanged: (v) => setDlgState(() => clearStats = v ?? false),
            title: const Text('同时清空阅读统计'),
            subtitle: const Text('仅阅读次数归零；最近阅读列表与进度保留'),
          ),
        ]),
        actions: [
          TextButton(onPressed: () => Navigator.of(c).pop(false), child: const Text('取消')),
          FilledButton(onPressed: () => Navigator.of(c).pop(true), child: const Text('确定')),
        ],
      )),
    );
    if (ok != true) return;
    final msg = StringBuffer('已清空全部缓存');
    if (clearRecent) {
      await LibraryStore.instance.clearReadRecords();
      msg.write('、最近阅读记录');
    }
    if (clearStats) {
      await LibraryStore.instance.resetReadCounts();
      msg.write('、阅读统计');
    }
    final freed = await clearAllCaches();
    ComicCover.clear();
    if (mounted) _snack('$msg (释放 ${fmtSize(freed)})');
    await _refresh();
  }

  Future<void> _changeCacheDir() async {
    final current = await cacheRootPath();
    final supportDir = (await getApplicationSupportDirectory()).path;
    if (!mounted) return;

    final picked = await getDirectoryPath(initialDirectory: current);
    if (picked == null || !mounted) return;
    final newPath = picked.trim();
    if (_samePath(current, newPath)) {
      _snack('当前已是该缓存目录');
      return;
    }
    if (_isSupportDirOrRelated(newPath, supportDir)) {
      _snack('不能选择应用支持目录或其父/子目录');
      return;
    }
    final dir = Directory(newPath);
    if (!await dir.exists()) {
      try {
        await dir.create(recursive: true);
      } catch (e) {
        _snack('无法创建目录: $e');
        return;
      }
    }

    final needed = await _rootSize(current);
    final free = await availableSpace(path: newPath);
    if (free < needed) {
      _snack('目标盘空间不足：需要 ${fmtSize(needed)}，可用 ${fmtSize(free)}');
      return;
    }

    if (!mounted) return;
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (c) => AlertDialog(
        title: const Text('迁移根目录'),
        content: Text('将整个根目录（数据库 + 缓存，约 ${fmtSize(needed)}）从：\n$current\n\n迁移到：\n$newPath\n\n书源、标签、阅读记录随数据库一起迁移。'),
        actions: [
          TextButton(onPressed: () => Navigator.of(c).pop(), child: const Text('取消')),
          FilledButton(onPressed: () => Navigator.of(c).pop(true), child: const Text('确认迁移')),
        ],
      ),
    );
    if (confirmed != true || !mounted) return;

    // 迁移前先把排队中的保存落盘，降低复制热库的不一致风险。
    await LibraryStore.instance.flushPendingSave();
    final ok = await _runMigration(from: current, to: newPath);
    if (!ok || !mounted) return;

    await setCacheRootPath(path: newPath);
    await writeCacheRootMarker(newPath);
    // 重开数据库连接：后续读写指向新根，旧文件不再被占用。
    await reopenDataDb();
    final store = LibraryStore.instance;
    store.settings.cacheDir = newPath;
    store.updateSettings(store.settings);
    try {
      await deleteMigratedItems(root: current);
    } catch (e) {
      if (mounted) _snack('迁移完成，但旧目录清理失败（可稍后手动删除）: $e');
      await _refresh();
      return;
    }
    if (mounted) {
      _snack('根目录已切换并完成迁移');
      await _refresh();
    }
  }

  Future<void> _restoreDefaultCacheDir() async {
    final current = await cacheRootPath();
    final defaultPath = await defaultCacheRootPath();
    final supportDir = (await getApplicationSupportDirectory()).path;
    if (!mounted) return;
    if (_samePath(current, defaultPath)) {
      _snack('当前已是默认缓存目录');
      return;
    }
    if (_isSupportDirOrRelated(defaultPath, supportDir)) {
      _snack('默认目录与应用支持目录冲突，无法恢复');
      return;
    }
    final needed = await _rootSize(current);
    final free = await availableSpace(path: defaultPath);
    if (free < needed) {
      _snack('默认盘空间不足：需要 ${fmtSize(needed)}，可用 ${fmtSize(free)}');
      return;
    }
    if (!mounted) return;
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (c) => AlertDialog(
        title: const Text('恢复默认缓存目录'),
        content: Text('将整个根目录（约 ${fmtSize(needed)}）从：\n$current\n\n迁回默认目录：\n$defaultPath'),
        actions: [
          TextButton(onPressed: () => Navigator.of(c).pop(false), child: const Text('取消')),
          FilledButton(onPressed: () => Navigator.of(c).pop(true), child: const Text('确认恢复')),
        ],
      ),
    );
    if (confirmed != true || !mounted) return;

    await LibraryStore.instance.flushPendingSave();
    final ok = await _runMigration(from: current, to: defaultPath);
    if (!ok || !mounted) return;

    await setCacheRootPath(path: '');
    await writeCacheRootMarker('');
    await reopenDataDb();
    final store = LibraryStore.instance;
    store.settings.cacheDir = null;
    store.updateSettings(store.settings);
    try {
      await deleteMigratedItems(root: current);
    } catch (e) {
      if (mounted) _snack('已恢复默认目录，但旧目录清理失败（可稍后手动删除）: $e');
      await _refresh();
      return;
    }
    if (mounted) {
      _snack('已恢复默认缓存目录');
      await _refresh();
    }
  }

  /// 根目录待迁移总量：缓存大小 + database.db（若存在）。
  Future<BigInt> _rootSize(String root) async {
    final sizes = await cacheSizes();
    var total = sizes.page +
        sizes.raw +
        sizes.cover +
        sizes.ai +
        sizes.temp;
    try {
      final db = File('$root${Platform.pathSeparator}database.db');
      if (await db.exists()) {
        total += BigInt.from(await db.length());
      }
    } catch (_) {}
    return total;
  }

  Future<bool> _runMigration({required String from, required String to}) async {
    final supportDir = (await getApplicationSupportDirectory()).path;
    if (!mounted) return false;
    final notifier = ValueNotifier<double>(0);
    unawaited(showDialog<void>(
      context: context,
      barrierDismissible: false,
      builder: (_) => AlertDialog(
        title: const Text('迁移根目录'),
        content: ValueListenableBuilder<double>(
          valueListenable: notifier,
          builder: (_, v, _) => Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              LinearProgressIndicator(value: v > 0 ? v : null),
              const SizedBox(height: 8),
              Text('迁移中 ${(v * 100).toStringAsFixed(0)}% ...'),
            ],
          ),
        ),
      ),
    ));
    final timer = Timer.periodic(const Duration(milliseconds: 300), (_) async {
      final p = await migrationProgress();
      final total = p.$2;
      notifier.value = total == BigInt.zero ? 0 : p.$1 / total;
    });
    try {
      await migrateCacheRoot(from: from, to: to, supportDir: supportDir);
      notifier.value = 1;
      return true;
    } catch (e) {
      _snack('迁移失败: $e');
      return false;
    } finally {
      timer.cancel();
      if (mounted) Navigator.of(context, rootNavigator: true).pop();
    }
  }

  bool _samePath(String a, String b) {
    String n(String x) {
      final t = x.trim().replaceAll('\\', '/');
      return t.endsWith('/') ? t.substring(0, t.length - 1) : t;
    }
    return n(a).toLowerCase() == n(b).toLowerCase();
  }

  bool _isSupportDirOrRelated(String path, String supportDir) {
    final p = path.replaceAll('\\', '/').toLowerCase();
    final s = supportDir.replaceAll('\\', '/').toLowerCase();
    // 只拒绝"支持目录本身或其内部"；默认根是支持目录的父目录（%APPDATA%\RCH），必须允许。
    return p == s || p.startsWith('$s/');
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
      Text('各缓存分类独立清理，清理后可自动重新生成',
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
              '远程书源整本下载的原始文件',
              () => _clear('清空整本下载缓存', clearRawCache)),
          _cacheRow(Icons.image, '封面缩略图（cover/）', _sizes!.cover,
              '海报墙封面图片缓存',
              () => _clear('清空封面缓存', clearCoverCache, clearCoverMemory: true)),
          _cacheRow(Icons.auto_awesome, 'AI 超分结果（ai/）', _sizes!.ai,
              'AI 超分输出缓存',
              () => _clear('清空 AI 缓存', clearAiCache)),
          _cacheRow(Icons.storage, '临时文件（temp/）', _sizes!.temp,
              'AI 超分输入/输出中间文件',
              () => _clear('清空临时文件', clearTempCache)),
        ],
        const SizedBox(height: 12),
        SizedBox(
          width: double.infinity,
          child: OutlinedButton.icon(
            onPressed: () => _clearAllWithChoices(),
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
                onPressed: _restoreDefaultCacheDir,
                icon: const Icon(Icons.settings_backup_restore, size: 18),
                label: const Text('恢复默认缓存目录'),
              ),
            ),
            const SizedBox(height: 4),
            SizedBox(
              width: double.infinity,
              child: OutlinedButton.icon(
                onPressed: _purging
                    ? null
                    : () async {
                        setState(() => _purging = true);
                        try {
                          final (r, m, freed, failed) =
                              await LibraryStore.instance.purgeStaleData();
                          final tip = failed > 0
                              ? '（$failed 个远程书源在线核对失败，已保留其数据，请检查网络/登录后重试）'
                              : '';
                          _snack('已清理 $r 条失效记录、$m 条失效元数据，释放 '
                              '${fmtSize(BigInt.from(freed))} 缓存$tip');
                        } finally {
                          if (mounted) setState(() => _purging = false);
                        }
                      },
                icon: const Icon(Icons.cleaning_services, size: 18),
                label: Text(_purging ? '正在核对远程书源并清理…' : '清理失效漫画数据'),
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
