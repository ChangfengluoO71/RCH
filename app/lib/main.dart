import 'dart:convert';
import 'dart:io';

import 'package:app/src/rust/api/cache.dart';
import 'package:app/src/rust/api/db.dart';
import 'package:app/src/rust/frb_generated.dart';
import 'package:app/store/library_store.dart';
import 'package:app/store/cache_root_marker.dart';
import 'package:app/ui/home_page.dart';
import 'package:flutter/material.dart';
import 'package:path_provider/path_provider.dart';

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  await RustLib.init();

  // 启动恢复自定义根：标记文件优先，library.json 兜底（在打开数据库之前）。
  final customRoot = await readCacheRootMarker() ?? await _cacheDirFromLibraryJson();
  if (customRoot != null && customRoot.isNotEmpty) {
    await setCacheRootPath(path: customRoot);
  }
  // 数据愈合：当前根缺 database.db 时，从旧位置挑最新的一份搬入。
  await _healDatabaseLocation();

  // SQLite 迁移：library.json → database.db（幂等）
  if (!await dataIsMigrated()) {
    final lib = await LibraryStore.instance.filePath();
    try {
      await dataMigrateFromJson(jsonPath: lib);
    } catch (e) {
      // 迁移失败不阻塞启动：回退到纯 JSON 模式
      debugPrint('[main] SQLite 迁移失败，继续使用 JSON: $e');
    }
  }

  // 加载数据（优先 SQLite，fallback JSON）
  await LibraryStore.instance.load();

  // 检测未完成的根目录迁移（源根上的 migration.partial 标记）。
  final root = await cacheRootPath();
  final pending = await pendingMigration(root: root);
  runApp(RchApp(startupPending: pending));
}

/// 从 library.json 读取 settings.cacheDir（标记文件缺失时的兜底）。
Future<String?> _cacheDirFromLibraryJson() async {
  try {
    final dir = await getApplicationSupportDirectory();
    final f = File('${dir.path}${Platform.pathSeparator}library.json');
    if (!await f.exists()) return null;
    final j = jsonDecode(await f.readAsString()) as Map<String, dynamic>;
    return (j['settings'] as Map?)?['cacheDir'] as String?;
  } catch (_) {
    return null;
  }
}

/// 数据愈合：当前根目录缺 database.db 时，从候选位置
/// （支持目录、默认根）挑最新的一份复制过来，校验后删除源。
Future<void> _healDatabaseLocation() async {
  final currentRoot = await cacheRootPath();
  final dbFile = File('$currentRoot${Platform.pathSeparator}database.db');
  if (await dbFile.exists()) return;

  final candidates = <String>{
    (await getApplicationSupportDirectory()).path,
    await defaultCacheRootPath(),
  };
  String? best;
  DateTime? bestTime;
  for (final c in candidates) {
    if (c == currentRoot) continue;
    final f = File('$c${Platform.pathSeparator}database.db');
    if (!await f.exists()) continue;
    final t = await f.lastModified();
    if (best == null || t.isAfter(bestTime!)) {
      best = f.path;
      bestTime = t;
    }
  }
  if (best == null) return;
  try {
    final src = File(best);
    await src.copy(dbFile.path);
    final ok = await dbFile.exists() && await dbFile.length() == await src.length();
    if (ok) {
      await src.delete();
      debugPrint('[main] database.db 已搬入当前根目录: $currentRoot');
    } else {
      await dbFile.delete();
    }
  } catch (e) {
    debugPrint('[main] 搬入 database.db 失败: $e');
  }
}

class RchApp extends StatelessWidget {
  const RchApp({super.key, this.startupPending});
  final (String, String)? startupPending;

  @override
  Widget build(BuildContext context) {
    // 监听设置变化,主题(白天/夜间)即时生效。
    return AnimatedBuilder(
      animation: LibraryStore.instance,
      builder: (context, _) {
        final dark = LibraryStore.instance.settings.themeMode != 'light';
        return MaterialApp(
          title: 'RCH',
          theme: ThemeData.light(useMaterial3: true),
          darkTheme: ThemeData.dark(useMaterial3: true),
          themeMode: dark ? ThemeMode.dark : ThemeMode.light,
          home: _LifecycleFlush(startupPending: startupPending, child: const HomePage()),
        );
      },
    );
  }
}

/// 应用生命周期兜底：退出时强制 flush 待保存的库数据，
/// 配合 LibraryStore 的防抖全量保存，避免正常退出丢标签。
class _LifecycleFlush extends StatefulWidget {
  const _LifecycleFlush({required this.child, this.startupPending});
  final Widget child;
  final (String, String)? startupPending;

  @override
  State<_LifecycleFlush> createState() => _LifecycleFlushState();
}

class _LifecycleFlushState extends State<_LifecycleFlush> {
  late final AppLifecycleListener _listener;

  @override
  void initState() {
    super.initState();
    _listener = AppLifecycleListener(
      onHide: _flush,
      onPause: _flush,
      onDetach: _flush,
    );
    final pending = widget.startupPending;
    if (pending != null) {
      WidgetsBinding.instance
          .addPostFrameCallback((_) => _offerResumeMigration(pending));
    }
  }

  Future<void> _flush() async {
    await LibraryStore.instance.flushPendingSave();
  }

  /// 启动时检测到未完成的根目录迁移 → 提供"继续迁移"。
  Future<void> _offerResumeMigration((String, String) pending) async {
    final (from, to) = pending;
    final messenger = ScaffoldMessenger.of(context);
    final resume = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: const Text('检测到未完成的缓存迁移'),
        content: Text('上次迁移根目录时应用被中断：\n$from\n → \n$to\n\n数据未受影响，是否继续迁移？'),
        actions: [
          TextButton(onPressed: () => Navigator.of(ctx).pop(false), child: const Text('稍后')),
          FilledButton(onPressed: () => Navigator.of(ctx).pop(true), child: const Text('继续迁移')),
        ],
      ),
    );
    if (resume != true) return;
    try {
      final supportDir = (await getApplicationSupportDirectory()).path;
      await migrateCacheRoot(from: from, to: to, supportDir: supportDir);
      await setCacheRootPath(path: to);
      await writeCacheRootMarker(to);
      final store = LibraryStore.instance;
      store.settings.cacheDir = to;
      store.updateSettings(store.settings);
      await deleteMigratedItems(root: from);
      messenger.showSnackBar(const SnackBar(content: Text('缓存迁移已恢复完成')));
    } catch (e) {
      messenger.showSnackBar(SnackBar(content: Text('恢复迁移失败: $e')));
    }
  }

  @override
  void dispose() {
    _listener.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => widget.child;
}
