import 'package:app/store/library_store.dart';
import 'package:app/store/models.dart';
import 'package:app/theme/overlay_material_scope.dart';
import 'package:app/theme/window_material_scope.dart';
import 'package:app/ui/common.dart';
import 'package:app/ui/rch_overlay.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  late AppSettings original;
  setUp(() => original = LibraryStore.instance.settings);
  tearDown(() {
    LibraryStore.instance.settings = original;
    debugDefaultTargetPlatformOverride = null;
  });

  Future<void> showDialogWithSettings(
    WidgetTester tester, {
    required TargetPlatform platform,
    required Map<String, dynamic> settings,
    bool highContrast = false,
    bool reduceMotion = false,
    bool windowsAcrylic = false,
    bool micaSupported = false,
  }) async {
    debugDefaultTargetPlatformOverride = platform;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    LibraryStore.instance.settings = AppSettings.fromJson(settings);
    try {
      await tester.pumpWidget(
        WindowMaterialScope(
          micaSupported: micaSupported,
          child: OverlayMaterialScope(
            acrylicSelected: windowsAcrylic,
            child: MaterialApp(
              builder: (context, child) => MediaQuery(
                data: MediaQuery.of(context).copyWith(
                  highContrast: highContrast,
                  disableAnimations: reduceMotion,
                ),
                child: child!,
              ),
              home: Builder(
                builder: (context) => TextButton(
                  onPressed: () => showRchDialog<void>(
                    context: context,
                    builder: (_) => const AlertDialog(content: Text('内容')),
                  ),
                  child: const Text('打开'),
                ),
              ),
            ),
          ),
        ),
      );
      await tester.tap(find.text('打开'));
      await tester.pumpAndSettle();
    } finally {
      debugDefaultTargetPlatformOverride = null;
    }
  }

  testWidgets(
    'Android tablet frosted dialog is clipped and independent of Mica',
    (tester) async {
      await showDialogWithSettings(
        tester,
        platform: TargetPlatform.android,
        settings: {
          'tabletLayout': 'tablet',
          'tabletOverlayMaterial': 'frosted',
        },
      );
      final filter = find.byType(BackdropFilter);
      expect(filter, findsOneWidget);
      expect(
        find.ancestor(of: filter, matching: find.byType(ClipRRect)),
        findsWidgets,
      );
      final surface = find
          .descendant(
            of: find.byType(AlertDialog),
            matching: find.byType(Material),
          )
          .first;
      expect(
        tester.getRect(filter),
        tester.getRect(surface),
        reason: 'blur must stay within the dialog rather than cover the page',
      );
      expect(find.text('内容'), findsOneWidget);
    },
  );

  testWidgets(
    'high contrast and reduced motion fall back to standard surfaces',
    (tester) async {
      for (final flags in [(true, false), (false, true)]) {
        await showDialogWithSettings(
          tester,
          platform: TargetPlatform.android,
          settings: {
            'tabletLayout': 'tablet',
            'tabletOverlayMaterial': 'frosted',
          },
          highContrast: flags.$1,
          reduceMotion: flags.$2,
        );
        expect(find.byType(BackdropFilter), findsNothing);
        await tester.pumpWidget(const SizedBox());
      }
    },
  );

  testWidgets('Windows acrylic retains its native capability gate', (
    tester,
  ) async {
    await showDialogWithSettings(
      tester,
      platform: TargetPlatform.windows,
      settings: const {},
      windowsAcrylic: true,
      micaSupported: true,
    );
    expect(find.byType(BackdropFilter), findsOneWidget);
    await tester.pumpWidget(const SizedBox());
    await showDialogWithSettings(
      tester,
      platform: TargetPlatform.windows,
      settings: const {},
      windowsAcrylic: true,
      micaSupported: false,
    );
    expect(find.byType(BackdropFilter), findsNothing);
  });

  testWidgets(
    'auto Android tablet can use frosted; explicit phone mode stays standard',
    (tester) async {
      tester.view.display.size = const Size(1200, 900);
      tester.view.devicePixelRatio = 1;
      try {
        await showDialogWithSettings(
          tester,
          platform: TargetPlatform.android,
          settings: const {
            'tabletLayout': 'auto',
            'tabletOverlayMaterial': 'frosted',
          },
        );
        expect(find.byType(BackdropFilter), findsOneWidget);
        await tester.pumpWidget(const SizedBox());
        await showDialogWithSettings(
          tester,
          platform: TargetPlatform.android,
          settings: const {
            'tabletLayout': 'mobile',
            'tabletOverlayMaterial': 'frosted',
          },
        );
        expect(find.byType(BackdropFilter), findsNothing);
      } finally {
        tester.view.reset();
        tester.view.display.reset();
      }
    },
  );

  testWidgets('frosted bottom sheet clips blur to the sheet', (tester) async {
    LibraryStore.instance.settings = AppSettings.fromJson(const {
      'tabletLayout': 'tablet',
      'tabletOverlayMaterial': 'frosted',
    });
    try {
      debugDefaultTargetPlatformOverride = TargetPlatform.android;
      await tester.pumpWidget(
        MaterialApp(
          home: Builder(
            builder: (context) => TextButton(
              onPressed: () => showRchModalBottomSheet<void>(
                context: context,
                builder: (_) => const SizedBox(height: 180, child: Text('面板')),
              ),
              child: const Text('打开'),
            ),
          ),
        ),
      );
      await tester.tap(find.text('打开'));
      await tester.pumpAndSettle();
      final filter = find.byType(BackdropFilter);
      expect(filter, findsOneWidget);
      expect(tester.getSize(filter).height, 180);
      expect(
        find.ancestor(of: filter, matching: find.byType(ClipRRect)),
        findsWidgets,
      );
    } finally {
      debugDefaultTargetPlatformOverride = null;
    }
  });

  testWidgets('reader dialogs do not blur the comic canvas', (tester) async {
    final owner = Object();
    try {
      debugDefaultTargetPlatformOverride = TargetPlatform.android;
      appOrientationCoordinator.enterReader(owner);
      await showDialogWithSettings(
        tester,
        platform: TargetPlatform.android,
        settings: const {
          'tabletLayout': 'tablet',
          'tabletOverlayMaterial': 'frosted',
        },
      );
      expect(find.byType(BackdropFilter), findsNothing);
    } finally {
      debugDefaultTargetPlatformOverride = TargetPlatform.android;
      appOrientationCoordinator.leaveReader(owner);
      debugDefaultTargetPlatformOverride = null;
    }
  });

  testWidgets('frosted bounds follow a dialog whose content changes size', (
    tester,
  ) async {
    LibraryStore.instance.settings = AppSettings.fromJson(const {
      'tabletLayout': 'tablet',
      'tabletOverlayMaterial': 'frosted',
    });
    var height = 100.0;
    try {
      debugDefaultTargetPlatformOverride = TargetPlatform.android;
      await tester.pumpWidget(
        MaterialApp(
          home: Builder(
            builder: (context) => TextButton(
              onPressed: () => showRchDialog<void>(
                context: context,
                builder: (_) => StatefulBuilder(
                  builder: (context, setState) {
                    return AlertDialog(
                      content: SizedBox(
                        height: height,
                        width: 300,
                        child: TextButton(
                          onPressed: () => setState(() => height = 250),
                          child: const Text('调整大小'),
                        ),
                      ),
                    );
                  },
                ),
              ),
              child: const Text('打开'),
            ),
          ),
        ),
      );
      await tester.tap(find.text('打开'));
      await tester.pumpAndSettle();
      final before = tester.getRect(find.byType(BackdropFilter));
      await tester.tap(find.text('调整大小'));
      await tester.pumpAndSettle();
      final surface = find
          .descendant(
            of: find.byType(AlertDialog),
            matching: find.byType(Material),
          )
          .first;
      expect(
        tester.getRect(find.byType(BackdropFilter)),
        tester.getRect(surface),
      );
      expect(
        tester.getRect(find.byType(BackdropFilter)).height,
        greaterThan(before.height),
      );
    } finally {
      debugDefaultTargetPlatformOverride = null;
    }
  });
}
