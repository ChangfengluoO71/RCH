import 'package:app/store/eh_daily_coordinator.dart';
import 'package:app/store/cloud115_bt_batch.dart';
import 'package:app/store/eh_subscription_store.dart';
import 'package:app/store/models.dart';
import 'package:app/src/rust/source/cloud115/offline.dart';
import 'package:app/ui/eh_subscription_panel.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  testWidgets('Daily card explains that it waits for the current task', (
    tester,
  ) async {
    final store = EhSubscriptionStore.forTest(
      rules: {
        'out_dir': r'C:\h1-gui-status-test\output',
        'automation': {'daily_enabled': true},
      },
      rulesPath: r'C:\h1-gui-status-test\eh_subscription_rules.json',
    );
    EhDailyCoordinator.initialize(store);
    final coordinator = EhDailyCoordinator.maybeInstance!;
    coordinator.status = EhDailyCoordinatorStatus.waitingForCurrentRun;

    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: SingleChildScrollView(
            child: EhSubscriptionPanel(storeOverride: store),
          ),
        ),
      ),
    );

    expect(find.text('Daily 正在等待当前任务结束。'), findsOneWidget);

    await tester.pumpWidget(const SizedBox.shrink());
    store.dispose();
  });

  testWidgets('Daily rule save failure is visible and keeps the switch off', (
    tester,
  ) async {
    final store = EhSubscriptionStore.forTest(
      rules: {
        'out_dir': r'C:\h1-gui-status-test\output',
        'automation': {'daily_enabled': false},
      },
      rulesWriter: (_) async => throw StateError('controlled save failure'),
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
    await tester.ensureVisible(find.byKey(const ValueKey('eh-daily-toggle')));
    await tester.tap(find.byKey(const ValueKey('eh-daily-toggle')));
    await tester.pumpAndSettle();

    expect(store.dailyAutomation.dailyEnabled, isFalse);
    expect(find.textContaining('保存自动化设置失败'), findsOneWidget);

    await tester.pumpWidget(const SizedBox.shrink());
    store.dispose();
  });

  testWidgets('manual collection failure is shown on the injected store', (
    tester,
  ) async {
    var collectCalls = 0;
    final store = EhSubscriptionStore.forTest(
      rules: {
        'out_dir': r'C:\h1-gui-status-test\output',
        'automation': {'daily_enabled': false},
      },
      checkedManifestLoader: (_) async => '{"status":"empty","entries":[]}',
      torrentDirectoryLoader: (_) async => '{"status":"empty","entries":[]}',
      collectRunner: (_) async {
        collectCalls++;
        throw StateError('controlled collection failure');
      },
    );

    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: SingleChildScrollView(
            child: EhSubscriptionPanel(
              storeOverride: store,
              btSourcesOverride: [
                BookSource(
                  id: 'test-source',
                  type: '115',
                  name: 'Test Cookie source',
                  cookie: 'not-used-in-test',
                ),
              ],
              btBatchPreferencesLoader: () async =>
                  Cloud115BtBatchSelectionPreferences(
                    sourceId: 'test-source',
                    target: const Cloud115BtFolderSelection(
                      id: 'target-folder',
                      parentId: 'parent-folder',
                      name: 'EH订阅',
                      path: '/漫画/EH订阅',
                    ),
                  ),
            ),
          ),
        ),
      ),
    );
    await tester.ensureVisible(
      find.byKey(const ValueKey('eh-full-workflow-run')),
    );
    await tester.tap(find.byKey(const ValueKey('eh-full-workflow-run')));
    await tester.pumpAndSettle();

    expect(collectCalls, 1);
    expect(find.textContaining('controlled collection failure'), findsWidgets);

    await tester.pumpWidget(const SizedBox.shrink());
    store.dispose();
  });

  testWidgets('damaged rules are shown as a load error, not an empty list', (
    tester,
  ) async {
    final store = EhSubscriptionStore.forTest(
      rules: {'out_dir': r'C:\h1-gui-status-test\output'},
    )..loadError = 'controlled malformed rules JSON';

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
      find.textContaining('加载失败：controlled malformed rules JSON'),
      findsOneWidget,
    );

    await tester.pumpWidget(const SizedBox.shrink());
    store.dispose();
  });
}
