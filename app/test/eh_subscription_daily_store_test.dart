import 'dart:convert';
import 'dart:io';

import 'package:app/store/eh_daily_coordinator.dart';
import 'package:app/store/eh_subscription_store.dart';
import 'package:app/ui/eh_subscription_panel.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  group('EH Daily state in the existing rules configuration', () {
    test('filter rule edits preserve the typed Daily settings', () async {
      String? savedRules;
      final store = EhSubscriptionStore.forTest(
        rules: {
          'search': 'old query',
          'out_dir': 'C:/EH output',
          'automation': {
            'daily_enabled': true,
            'last_auto_attempt_date': '2026-10-11',
            'last_run': {
              'date': '2026-10-11',
              'source': 'daily_auto',
              'status': 'partial',
              'saved': 3,
              'cloud_stage': {
                'status': 'partial',
                'total': 3,
                'submitted': 1,
                'skipped': 0,
                'pending': 2,
                'removed': 0,
                'failed': 0,
                'message': 'unknown items retained',
              },
            },
          },
        },
        rulesWriter: (rulesJson) async {
          savedRules = rulesJson;
        },
      );

      store.patch('search', 'new query');
      await store.saveRules();

      final saved = jsonDecode(savedRules!) as Map<String, dynamic>;
      expect(saved['search'], 'new query');
      expect(saved['automation']['daily_enabled'], isTrue);
      expect(saved['automation']['last_auto_attempt_date'], '2026-10-11');
      final decodedState = EhDailyAutomationState.fromJson(
        Map<String, Object?>.from(saved['automation'] as Map),
      );
      expect(decodedState.lastRun?.status, EhDailyRunStatus.partial);
      expect(decodedState.lastRun?.cloudStage.pending, 2);
      expect(
        decodedState.lastRun?.cloudStage.message,
        'unknown items retained',
      );
    });

    test('default and legacy configurations keep Daily disabled', () {
      final store = EhSubscriptionStore.forTest(
        rules: {'search': 'legacy query', 'out_dir': 'C:/EH output'},
      );

      expect(store.dailyAutomation.dailyEnabled, isFalse);
      expect(store.dailyAutomation.lastAutoAttemptDate, isNull);
    });

    test(
      'rules persistence failure releases busy and never starts collection',
      () async {
        var collectCalls = 0;
        final store = EhSubscriptionStore.forTest(
          rules: {'search': 'query', 'out_dir': 'C:/EH output'},
          rulesWriter: (_) async => throw StateError('disk unavailable'),
          collectRunner: (_) async {
            collectCalls++;
            return '{"stage":"done","saved":0}';
          },
          checkedManifestLoader: (_) async => '{"status":"empty"}',
        );

        final result = await store.run();

        expect(result.started, isFalse);
        expect(result.status, EhDailyRunStatus.failed);
        expect(store.busy, isFalse);
        expect(collectCalls, 0);
      },
    );

    test(
      'Daily runs the cloud stage from the refreshed checked manifest',
      () async {
        var cloudCalls = 0;
        List<EhCheckedManifestEntry>? receivedEntries;
        final store = EhSubscriptionStore.forTest(
          rules: {
            'search': 'query',
            'out_dir': 'C:/EH output',
            'automation': {
              'daily_enabled': true,
              'last_auto_attempt_date': '2026-10-13',
              'last_run': {
                'date': '2026-10-13',
                'source': 'daily_auto',
                'status': 'claimed',
              },
            },
          },
          rulesWriter: (_) async {},
          collectRunner: (_) async => '{"stage":"done","saved":1}',
          checkedManifestLoader: (_) async => jsonEncode({
            'status': 'ready',
            'entries': [
              {
                'item': {
                  'infohash': '0123456789012345678901234567890123456789',
                  'title_jpn': 'sample',
                  'rating': 5,
                  'downloads': 1,
                  'required_dl': 0,
                  'posted_utc': '2026-10-13',
                  'category': 'Manga',
                  'filecount': 1,
                  'filesize': 1,
                  'tags': [],
                  'file': 'sample.torrent',
                  'bytes': 1,
                },
                'file_status': 'present',
                'validation_status': 'valid',
                'full_path': 'C:/EH output/sample.torrent',
              },
            ],
          }),
          dailyCloudStageRunner: (entries) async {
            cloudCalls++;
            receivedEntries = entries;
            return const EhDailyCloudStageResult(
              status: EhDailyCloudStageStatus.partial,
              total: 1,
              pending: 1,
              message: 'unknown item retained; no retry',
            );
          },
        );

        final outcome = await store.run(source: EhDailyRunSource.dailyAuto);

        expect(cloudCalls, 1);
        expect(receivedEntries, hasLength(1));
        expect(
          receivedEntries!.single.validationStatus,
          EhTorrentValidationStatus.valid,
        );
        expect(outcome.status, EhDailyRunStatus.partial);
        expect(store.dailyAutomation.lastRun?.cloudStage.pending, 1);
        expect(
          store.dailyAutomation.lastRun?.cloudStage.status,
          EhDailyCloudStageStatus.partial,
        );
        expect(
          store.dailyAutomation.lastRun?.cloudStage.message,
          contains('unknown item'),
        );
      },
    );

    testWidgets(
      'Daily UI is off by default and never starts collection itself',
      (tester) async {
        var collectCalls = 0;
        final store = EhSubscriptionStore.forTest(
          rules: {'search': 'query', 'out_dir': 'C:/EH output'},
          rulesWriter: (_) async {},
          collectRunner: (_) async {
            collectCalls++;
            return '{"stage":"done","saved":0}';
          },
        );
        await tester.pumpWidget(
          MaterialApp(
            home: Scaffold(
              body: SingleChildScrollView(
                child: EhSubscriptionPanel(storeOverride: store),
              ),
            ),
          ),
        );

        expect(find.text('自动每日完整工作流'), findsOneWidget);
        final toggle = tester.widget<Switch>(
          find.byKey(const ValueKey('eh-daily-toggle')),
        );
        expect(toggle.value, isFalse);
        if (Platform.isWindows) {
          await tester.ensureVisible(
            find.byKey(const ValueKey('eh-daily-toggle')),
          );
          await tester.pumpAndSettle();
          await tester.tap(find.byKey(const ValueKey('eh-daily-toggle')));
          await tester.pumpAndSettle();
          expect(store.dailyAutomation.dailyEnabled, isTrue);
        } else {
          expect(toggle.onChanged, isNull);
        }
        expect(collectCalls, 0);
      },
    );

    testWidgets('P0 gate prevents enabling the complete Daily workflow', (
      tester,
    ) async {
      final store = EhSubscriptionStore.forTest(
        rules: {'search': 'query', 'out_dir': 'C:/EH output'},
        dailyWorkflowReady: false,
        dailyWorkflowGateMessage: '115 Daily 等待 P0_REAL_PASS',
      );
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: SingleChildScrollView(
              child: EhSubscriptionPanel(storeOverride: store),
            ),
          ),
        ),
      );
      final toggle = tester.widget<Switch>(
        find.byKey(const ValueKey('eh-daily-toggle')),
      );
      expect(toggle.value, isFalse);
      expect(toggle.onChanged, isNull);
      expect(find.textContaining('完整自动工作流尚未开放'), findsOneWidget);
      await expectLater(store.setDailyEnabled(true), throwsStateError);
      expect(store.dailyAutomation.dailyEnabled, isFalse);
      expect(find.textContaining('等待 P0_REAL_PASS'), findsWidgets);

      await tester.pumpWidget(const SizedBox.shrink());
      store.dispose();
    });

    testWidgets('Daily full-workflow status appears before seed handoff', (
      tester,
    ) async {
      final store = EhSubscriptionStore.forTest(
        rules: {'search': 'query', 'out_dir': 'C:/EH output'},
      );
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: SingleChildScrollView(
              child: EhSubscriptionPanel(storeOverride: store),
            ),
          ),
        ),
      );

      expect(
        tester
            .getTopLeft(find.byKey(const ValueKey('eh-daily-workflow-card')))
            .dy,
        lessThan(
          tester.getTopLeft(find.byKey(const ValueKey('eh-seed-handoff'))).dy,
        ),
      );
      expect(
        tester.getTopLeft(find.byKey(const ValueKey('eh-seed-handoff'))).dy,
        lessThan(
          tester
              .getTopLeft(find.byKey(const ValueKey('eh-advanced-options')))
              .dy,
        ),
      );

      await tester.pumpWidget(const SizedBox.shrink());
      store.dispose();
    });
  });
}
