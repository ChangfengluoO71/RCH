import 'package:app/store/remote_scan_models.dart';
import 'package:app/ui/remote_scan_status.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  testWidgets(
    'status panel shows progress, controls, and redacted degraded error',
    (tester) async {
      final state = ValueNotifier(
        const RemoteScanViewState(
          sourceId: 'source-a',
          status: 'degraded',
          mode: 'incremental',
          processed: 3,
          total: 7,
          errorCode:
              'rangeUnavailable: https://user:secret@example.com/book.cbz?token=abc',
        ),
      );
      final calls = <String>[];

      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: RemoteScanStatusPanel(
              sourceName: 'Cloud',
              stateListenable: state,
              onPause: () async => calls.add('pause'),
              onResume: () async => calls.add('resume'),
              onRetry: () async => calls.add('retry'),
              onRescanIncremental: () async => calls.add('incremental'),
              onRescanFull: () async => calls.add('full'),
            ),
          ),
        ),
      );

      expect(find.textContaining('Cloud'), findsOneWidget);
      expect(find.textContaining('漫画文件扫描：已完成 3 本 / 已发现 7 本'), findsOneWidget);
      expect(find.textContaining('3/7'), findsNothing);
      expect(find.textContaining('Range \u4e0d\u53ef\u7528'), findsOneWidget);
      expect(find.textContaining('token'), findsNothing);
      expect(find.textContaining('secret'), findsNothing);
      expect(find.textContaining('example.com'), findsNothing);

      await tester.tap(find.byTooltip('\u91cd\u8bd5\u8fdc\u7a0b\u626b\u63cf'));
      await tester.tap(find.byTooltip('\u589e\u91cf\u91cd\u65b0\u626b\u63cf'));
      await tester.tap(find.byTooltip('\u5168\u91cf\u91cd\u65b0\u626b\u63cf'));
      await tester.pump();

      expect(calls, ['retry', 'incremental', 'full']);
    },
  );

  testWidgets('paused status offers resume instead of pause', (tester) async {
    final state = ValueNotifier(
      const RemoteScanViewState(
        sourceId: 'source-a',
        status: 'paused',
        mode: 'full',
      ),
    );
    final calls = <String>[];

    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: RemoteScanStatusPanel(
            sourceName: 'Cloud',
            stateListenable: state,
            onPause: () async => calls.add('pause'),
            onResume: () async => calls.add('resume'),
            onRetry: () async => calls.add('retry'),
            onRescanIncremental: () async => calls.add('incremental'),
            onRescanFull: () async => calls.add('full'),
          ),
        ),
      ),
    );

    expect(
      find.byTooltip('\u6682\u505c\u8fdc\u7a0b\u626b\u63cf'),
      findsNothing,
    );
    await tester.tap(find.byTooltip('\u7ee7\u7eed\u8fdc\u7a0b\u626b\u63cf'));
    await tester.pump();

    expect(calls, ['resume']);
  });

  testWidgets('running status offers pause', (tester) async {
    final state = ValueNotifier(
      const RemoteScanViewState(
        sourceId: 'source-a',
        status: 'running',
        mode: 'full',
      ),
    );
    final calls = <String>[];

    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: RemoteScanStatusPanel(
            sourceName: 'Cloud',
            stateListenable: state,
            onPause: () async => calls.add('pause'),
          ),
        ),
      ),
    );

    await tester.tap(find.byTooltip('\u6682\u505c\u8fdc\u7a0b\u626b\u63cf'));
    await tester.pump();
    expect(calls, ['pause']);
  });

  testWidgets('rescan action failure is visible in Chinese', (tester) async {
    final state = ValueNotifier(
      const RemoteScanViewState(
        sourceId: 'source-a',
        status: 'failed',
        mode: 'full',
      ),
    );

    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: RemoteScanStatusPanel(
            sourceName: '云端',
            stateListenable: state,
            onRescanFull: () async => throw StateError('already running'),
          ),
        ),
      ),
    );

    await tester.tap(find.byTooltip('全量重新扫描'));
    await tester.pump();
    expect(find.text('重新扫描失败，请稍后重试'), findsOneWidget);
    expect(find.textContaining('already running'), findsNothing);
  });

  testWidgets('unknown terminal totals are explained instead of showing 0/0', (
    tester,
  ) async {
    final state = ValueNotifier(
      const RemoteScanViewState(
        sourceId: 'source-a',
        status: 'failed',
        mode: 'full',
        processed: 5,
      ),
    );

    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: RemoteScanStatusPanel(sourceName: '云端', stateListenable: state),
        ),
      ),
    );

    expect(find.textContaining('已完成 5 本，全量扫描尚未完成'), findsOneWidget);
    expect(find.textContaining('0/0'), findsNothing);
  });

  testWidgets(
    're-enabling remote covers hides a stale paused-generation hint',
    (tester) async {
      final state = ValueNotifier(
        const RemoteScanViewState(
          sourceId: 'source-a',
          status: 'complete',
          mode: 'full',
          errorCode: 'coverFetchPaused',
          coverFetchPaused: false,
        ),
      );

      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: RemoteScanStatusPanel(
              sourceName: '云端',
              stateListenable: state,
            ),
          ),
        ),
      );

      expect(find.textContaining('远程封面联网获取已暂停'), findsNothing);
    },
  );

  testWidgets('hides controls whose callbacks are unavailable', (tester) async {
    final state = ValueNotifier(
      const RemoteScanViewState(
        sourceId: 'source-a',
        status: 'degraded',
        mode: 'incremental',
      ),
    );

    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: RemoteScanStatusPanel(
            sourceName: 'Cloud',
            stateListenable: state,
          ),
        ),
      ),
    );

    expect(
      find.byTooltip('\u6682\u505c\u8fdc\u7a0b\u626b\u63cf'),
      findsNothing,
    );
    expect(
      find.byTooltip('\u7ee7\u7eed\u8fdc\u7a0b\u626b\u63cf'),
      findsNothing,
    );
    expect(
      find.byTooltip('\u91cd\u8bd5\u8fdc\u7a0b\u626b\u63cf'),
      findsNothing,
    );
    expect(
      find.byTooltip('\u589e\u91cf\u91cd\u65b0\u626b\u63cf'),
      findsNothing,
    );
    expect(
      find.byTooltip('\u5168\u91cf\u91cd\u65b0\u626b\u63cf'),
      findsNothing,
    );
  });

  testWidgets('completed status does not offer pause or resume', (
    tester,
  ) async {
    final state = ValueNotifier(
      const RemoteScanViewState(
        sourceId: 'source-a',
        status: 'complete',
        mode: 'full',
      ),
    );

    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: RemoteScanStatusPanel(
            sourceName: 'Cloud',
            stateListenable: state,
            onPause: () async {},
            onResume: () async {},
          ),
        ),
      ),
    );

    expect(
      find.byTooltip('\u6682\u505c\u8fdc\u7a0b\u626b\u63cf'),
      findsNothing,
    );
    expect(
      find.byTooltip('\u7ee7\u7eed\u8fdc\u7a0b\u626b\u63cf'),
      findsNothing,
    );
  });

  testWidgets('null state shows a Chinese not-started hint', (tester) async {
    final state = ValueNotifier<RemoteScanViewState?>(null);

    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: RemoteScanStatusPanel(
            sourceName: '115',
            stateListenable: state,
          ),
        ),
      ),
    );

    expect(find.text('\u7b49\u5f85\u626b\u63cf'), findsOneWidget);
    expect(find.textContaining('Background'), findsNothing);
    expect(find.textContaining('remote scan'), findsNothing);
  });

  testWidgets('all scan states and modes are presented in Chinese', (
    tester,
  ) async {
    final state = ValueNotifier<RemoteScanViewState?>(
      const RemoteScanViewState(
        sourceId: 'source-a',
        status: 'queued',
        mode: 'full',
      ),
    );

    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: RemoteScanStatusPanel(
            sourceName: '\u4e91\u7aef',
            stateListenable: state,
          ),
        ),
      ),
    );

    const labels = <String, String>{
      'queued': '\u6392\u961f\u4e2d',
      'running': '\u626b\u63cf\u4e2d',
      'complete': '\u5df2\u5b8c\u6210',
      'paused': '\u5df2\u6682\u505c',
      'degraded': '\u90e8\u5206\u5b8c\u6210',
      'rangeUnavailable': 'Range \u4e0d\u53ef\u7528',
      'failed': '\u5931\u8d25',
      'cancelled': '\u5df2\u53d6\u6d88',
    };
    for (final entry in labels.entries) {
      state.value = RemoteScanViewState(
        sourceId: 'source-a',
        status: entry.key,
        mode: entry.key == 'complete' ? 'incremental' : 'full',
      );
      await tester.pump();
      expect(
        find.textContaining(entry.value),
        findsAtLeastNWidgets(1),
        reason: 'missing Chinese label for ${entry.key}',
      );
    }

    state.value = const RemoteScanViewState(
      sourceId: 'source-a',
      status: 'running',
      mode: 'incremental',
    );
    await tester.pump();
    expect(find.textContaining('\u5168\u91cf\u626b\u63cf'), findsNothing);
    state.value = const RemoteScanViewState(
      sourceId: 'source-a',
      status: 'running',
      mode: 'full',
    );
    await tester.pump();
    expect(find.textContaining('\u5168\u91cf\u626b\u63cf'), findsOneWidget);
    state.value = const RemoteScanViewState(
      sourceId: 'source-a',
      status: 'running',
      mode: 'incremental',
    );
    await tester.pump();
    expect(find.textContaining('\u589e\u91cf\u626b\u63cf'), findsOneWidget);
  });
}
