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

    test(
      'a damaged manifest cannot report the complete workflow as successful',
      () async {
        final store = EhSubscriptionStore.forTest(
          rules: {
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
          checkedManifestLoader: (_) async => '{"status":"damaged"}',
        );
        final outcome = await store.run(source: EhDailyRunSource.dailyAuto);
        expect(outcome.status, EhDailyRunStatus.partial);
        expect(
          store.dailyAutomation.lastAutoRun?.cloudStage.status,
          EhDailyCloudStageStatus.failed,
        );
      },
    );

    test(
      'a saved authentication wait can resume after a manual result',
      () async {
        final store = EhSubscriptionStore.forTest(
          rules: {
            'out_dir': 'C:/EH output',
            'automation': {
              'daily_enabled': true,
              'last_auto_attempt_date': '2026-10-13',
              'last_run': {
                'date': '2026-10-13',
                'source': 'manual',
                'status': 'completed',
              },
              'last_auto_run': {
                'date': '2026-10-13',
                'source': 'daily_auto',
                'status': 'partial',
                'run_id': 'daily-2026-10-13',
                'cloud_stage': {'status': 'session_unavailable'},
              },
            },
          },
          rulesWriter: (_) async {},
        );
        final lease = (await store.tryAcquireWorkflowLease())!;
        expect(await store.resumeDailyRun('2026-10-13', lease), isTrue);
        expect(store.dailyAutomation.lastAutoRun?.runId, 'daily-2026-10-13');
        expect(store.dailyAutomation.lastAutoRun?.resumeCount, 1);
        await store.releaseWorkflowLease(lease);
      },
    );

    test('a cancelled scan does not start the cloud stage', () async {
      var cloudCalls = 0;
      final store = EhSubscriptionStore.forTest(
        rules: {
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
        collectRunner: (_) async => '{"stage":"cancelled","saved":0}',
        checkedManifestLoader: (_) async => '{"status":"empty"}',
        dailyCloudStageRunner: (_) async {
          cloudCalls++;
          return const EhDailyCloudStageResult(
            status: EhDailyCloudStageStatus.completed,
          );
        },
      );
      final outcome = await store.run(source: EhDailyRunSource.dailyAuto);
      expect(outcome.status, EhDailyRunStatus.cancelled);
      expect(cloudCalls, 0);
    });

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

    testWidgets('Daily workflow setting has no P0 code gate', (tester) async {
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
      expect(find.textContaining('完整自动工作流尚未开放'), findsNothing);
      await store.setDailyEnabled(true);
      expect(store.dailyAutomation.dailyEnabled, isTrue);

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
