import 'package:app/store/eh_daily_coordinator.dart';
import 'package:app/store/eh_subscription_store.dart';
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

    expect(find.text('等待当前收集完成；Daily 不会抢占或中断当前任务。'), findsOneWidget);

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
    expect(find.textContaining('保存 Daily 设置失败'), findsOneWidget);

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
      collectRunner: (_) async {
        collectCalls++;
        throw StateError('controlled collection failure');
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
    await tester.ensureVisible(find.text('开始扫描'));
    await tester.tap(find.text('开始扫描'));
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
