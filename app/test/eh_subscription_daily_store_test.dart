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

        expect(find.text('自动每日检查'), findsOneWidget);
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
  });
}
