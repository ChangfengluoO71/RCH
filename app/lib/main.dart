import 'package:app/src/rust/api/db.dart';
import 'package:app/src/rust/frb_generated.dart';
import 'package:app/store/library_store.dart';
import 'package:app/ui/home_page.dart';
import 'package:flutter/material.dart';

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  await RustLib.init();

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
  runApp(const RchApp());
}

class RchApp extends StatelessWidget {
  const RchApp({super.key});

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
          home: const _LifecycleFlush(child: HomePage()),
        );
      },
    );
  }
}

/// 应用生命周期兜底：退出时强制 flush 待保存的库数据，
/// 配合 LibraryStore 的防抖全量保存，避免正常退出丢标签。
class _LifecycleFlush extends StatefulWidget {
  const _LifecycleFlush({required this.child});
  final Widget child;

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
  }

  Future<void> _flush() async {
    await LibraryStore.instance.flushPendingSave();
  }

  @override
  void dispose() {
    _listener.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => widget.child;
}
