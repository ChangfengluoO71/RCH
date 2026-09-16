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
      expect(find.textContaining('3/7'), findsOneWidget);
      expect(find.textContaining('Range unavailable'), findsOneWidget);
      expect(find.textContaining('token'), findsNothing);
      expect(find.textContaining('secret'), findsNothing);
      expect(find.textContaining('example.com'), findsNothing);

      await tester.tap(find.byTooltip('Pause remote scan'));
      await tester.tap(find.byTooltip('Retry remote scan'));
      await tester.tap(find.byTooltip('Incremental rescan'));
      await tester.tap(find.byTooltip('Full rescan'));
      await tester.pump();

      expect(calls, ['pause', 'retry', 'incremental', 'full']);
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

    expect(find.byTooltip('Pause remote scan'), findsNothing);
    await tester.tap(find.byTooltip('Resume remote scan'));
    await tester.pump();

    expect(calls, ['resume']);
  });

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

    expect(find.byTooltip('Pause remote scan'), findsNothing);
    expect(find.byTooltip('Resume remote scan'), findsNothing);
    expect(find.byTooltip('Retry remote scan'), findsNothing);
    expect(find.byTooltip('Incremental rescan'), findsNothing);
    expect(find.byTooltip('Full rescan'), findsNothing);
  });
}
