import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:app/src/rust/api/cache.dart';
import 'package:app/src/rust/api/db.dart';
import 'package:app/src/rust/api/pdf.dart';
import 'package:app/src/rust/frb_generated.dart';
import 'package:app/store/ai_upscale_manager.dart';
import 'package:app/store/automation_coordinator.dart';
import 'package:app/store/folder_snapshot_store.dart';
import 'package:app/store/library_store.dart';
import 'package:app/store/cache_root_marker.dart';
import 'package:app/store/library_catalog.dart';
import 'package:app/store/storage_access.dart';
import 'package:app/store/sync_manager.dart';
import 'package:app/ui/ai_floating_progress.dart';
import 'package:app/ui/home_page.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:path_provider/path_provider.dart';

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  _installErrorLog();
  await RustLib.init();

  // Android：把 jniLibs 的原生库目录传给 Rust，供 pdfium 加载 libpdfium.so。
  if (defaultTargetPlatform == TargetPlatform.android) {
    final dir = await nativeLibraryDir();
    if (dir != null && dir.isNotEmpty) {
      await setNativeLibDir(dir: dir);
    }
  }

  // 启动恢复自定义根：标记文件优先，library.json 兜底（在打开数据库之前）。
  final customRoot = await readCacheRootMarker() ?? await _cacheDirFromLibraryJson();
  if (customRoot != null && customRoot.isNotEmpty) {
    if (_isInvalidRootForPlatform(customRoot)) {
      // 移动端残留 Windows 绝对路径（如 D:\...）：直接忽略并清掉标记，回退默认根，
      // 否则 SQLite 会在不存在的路径上打开失败导致启动崩溃。
      await writeCacheRootMarker('');
      debugPrint('[main] 忽略跨平台无效缓存根: $customRoot');
    } else {
      await setCacheRootPath(path: customRoot);
    }
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

  // 首帧只等待首页真正依赖的数据。启动时不再为了 JSON 备份阻塞 800ms 防抖保存。
  await LibraryStore.instance.load(persist: false);

  // 检测未完成的根目录迁移（源根上的 migration.partial 标记）。
  final root = await cacheRootPath();
  final pending = await pendingMigration(root: root);
  runApp(RchApp(startupPending: pending));

  // 非关键初始化放到首帧之后：保留原有自动同步/刮削与 AI 续跑语义，
  // 但不再让目录快照、资料库树、同步配置和自动流程阻塞用户进入首页。
  WidgetsBinding.instance.addPostFrameCallback((_) {
    unawaited(_initializeAfterFirstFrame());
  });
}

Future<void> _initializeAfterFirstFrame() async {
  // 保留启动时刷新 JSON 备份的语义，但不等待它再启动其他后台服务。
  unawaited(LibraryStore.instance.saveToDisk());

  // 本地只读/轻量状态先恢复，再启动可能涉及同步与刮削的较重流程。
  await FolderSnapshotStore.instance.load();
  await LibraryCatalogStore.instance.loadTree();
  await SyncManager.instance.init();
  await AiUpscaleManager.instance.init();
  await AutomationCoordinator.instance.init();
}

/// 移动端（Android）拒绝 Windows 风格绝对路径（盘符 / UNC）作为缓存根。
bool _isInvalidRootForPlatform(String root) {
  if (Platform.isWindows || Platform.isMacOS || Platform.isLinux) return false;
  return RegExp(r'^[A-Za-z]:[\\/]').hasMatch(root) || root.startsWith(r'\\');
}

/// 全局错误日志：未捕获异常（含完整堆栈）追加写入缓存根目录 errors.log，
/// 用于远程排查（例如进入 115 子目录时的 RangeError）。
void _installErrorLog() {
  final prev = FlutterError.onError;
  FlutterError.onError = (details) {
    prev?.call(details);
    unawaited(_appendErrorLog(
        details.exceptionAsString(), details.stack?.toString() ?? ''));
  };
  PlatformDispatcher.instance.onError = (error, stack) {
    unawaited(_appendErrorLog(error.toString(), stack.toString()));
    return false; // 交给默认错误处理
  };
}

Future<void> _appendErrorLog(String error, String stack) async {
  try {
    final root = await cacheRootPath();
    final f = File('$root${Platform.pathSeparator}errors.log');
    final sink = f.openWrite(mode: FileMode.append);
    sink.writeln('--- ${DateTime.now().toIso8601String()} ---');
    sink.writeln(error);
    sink.writeln(stack);
    sink.writeln();
    await sink.close();
  } catch (_) {
    // 日志写入失败不影响应用
  }
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
          debugShowCheckedModeBanner: false,
          title: 'RCH',
          navigatorKey: AiUpscaleManager.navigatorKey,
          builder: (context, child) => MediaQuery(
            // 安卓触屏：降低手势判定阈值（touchSlop 18→13.5,panSlop 36→27），
            // 双指缩放/滑动更容易触发（指甲长、小幅捏合也能识别）。
            data: MediaQuery.of(context).copyWith(
              gestureSettings: defaultTargetPlatform == TargetPlatform.android
                  ? const DeviceGestureSettings(touchSlop: 13.5)
                  : null,
            ),
            child: Stack(
              children: [
                ?child,
                const Align(alignment: Alignment.topRight, child: AiFloatingProgress()),
              ],
            ),
          ),
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
    await FolderSnapshotStore.instance.flush();
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
      await reopenDataDb();
      final store = LibraryStore.instance;
      store.settings.cacheDir = to;
      store.updateSettings(store.settings);
      try {
        await deleteMigratedItems(root: from);
      } catch (_) {
        // 旧目录清理失败不阻断恢复完成提示
      }
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
