import 'package:app/store/library_store.dart';
import 'package:app/store/models.dart';
import 'package:app/ui/home_page.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  testWidgets('Android tablet in a narrow window keeps rotation enabled', (
    tester,
  ) async {
    final original = LibraryStore.instance.settings;
    final calls = <List<dynamic>>[];
    debugDefaultTargetPlatformOverride = TargetPlatform.android;
    LibraryStore.instance.settings = original;
    original.tabletLayout = 'tablet';
    tester.view.physicalSize = const Size(450, 900);
    tester.view.devicePixelRatio = 1;
    tester.view.display.size = const Size(1200, 900);
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(SystemChannels.platform, (call) async {
          if (call.method == 'SystemChrome.setPreferredOrientations') {
            calls.add(call.arguments as List<dynamic>);
          }
          return null;
        });
    addTearDown(() {
      original.tabletLayout = 'auto';
      LibraryStore.instance.settings = original;
      debugDefaultTargetPlatformOverride = null;
      tester.view.reset();
      tester.view.display.reset();
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(SystemChannels.platform, null);
    });
    try {
      await tester.pumpWidget(const MaterialApp(home: HomePage()));
      await tester.pump();
    } finally {
      debugDefaultTargetPlatformOverride = null;
    }
    expect(calls, isNotEmpty);
    expect(
      calls.last,
      unorderedEquals([
        'DeviceOrientation.portraitUp',
        'DeviceOrientation.portraitDown',
        'DeviceOrientation.landscapeLeft',
        'DeviceOrientation.landscapeRight',
      ]),
      reason: 'tablet split screen must not be forced to portrait',
    );
  });

  testWidgets('Android tablet settings expose their own material and preview', (
    tester,
  ) async {
    final original = LibraryStore.instance.settings;
    LibraryStore.instance.settings = AppSettings.fromJson(const {
      'tabletLayout': 'tablet',
      'tabletOverlayMaterial': 'frosted',
    });
    tester.view.physicalSize = const Size(1200, 900);
    tester.view.devicePixelRatio = 1;
    tester.view.display.size = const Size(1200, 900);
    try {
      debugDefaultTargetPlatformOverride = TargetPlatform.android;
      await tester.pumpWidget(const MaterialApp(home: HomePage()));
      await tester.pump();
      await tester.tap(find.text('设置').first);
      await tester.pump(const Duration(milliseconds: 400));
      final settingsList = find
          .ancestor(of: find.text('阅读'), matching: find.byType(ListView))
          .first;
      final settingsScroll = find
          .descendant(of: settingsList, matching: find.byType(Scrollable))
          .first;
      final appearance = find.text('外观与布局');
      await tester.scrollUntilVisible(
        appearance,
        300,
        scrollable: settingsScroll,
      );
      await tester.tap(appearance);
      await tester.pump();
      await tester.pump(const Duration(seconds: 1));
      expect(find.text('平板弹窗材质'), findsOneWidget);
      expect(find.text('平板风格'), findsOneWidget);
      final preview = find.text('预览效果');
      await tester.scrollUntilVisible(preview, 300, scrollable: settingsScroll);
      await tester.ensureVisible(preview);
      await tester.pump();
      await tester.tap(preview);
      await tester.pump(const Duration(milliseconds: 400));
      await tester.pump();
      await tester.pump();
      expect(find.text('弹窗材质预览'), findsOneWidget);
      expect(find.byType(BackdropFilter), findsOneWidget);
    } finally {
      debugDefaultTargetPlatformOverride = null;
      LibraryStore.instance.settings = original;
      tester.view.reset();
      tester.view.display.reset();
    }
  });
}
