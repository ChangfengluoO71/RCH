import 'package:app/src/rust/frb_generated.dart';
import 'package:app/store/library_store.dart';
import 'package:app/ui/home_page.dart';
import 'package:flutter/material.dart';

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  await RustLib.init();
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
          home: const HomePage(),
        );
      },
    );
  }
}
