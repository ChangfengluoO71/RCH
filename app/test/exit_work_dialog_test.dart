import 'dart:async';

import 'package:app/store/cloud115_bt_monitor.dart';
import 'package:app/store/long_task_registry.dart';
import 'package:app/ui/exit_work_dialog.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

LongTaskSnapshot _task({
  String label = 'EH 订阅扫描',
  String phase = '检索 EH 订阅',
  bool resumable = false,
  bool canPrepareForExit = false,
}) => LongTaskSnapshot(
  id: 1,
  kind: 'eh_scan',
  label: label,
  phase: phase,
  startedAt: DateTime(2026, 10, 14),
  resumable: resumable,
  canPrepareForExit: canPrepareForExit,
);

Future<Completer<bool?>> _openDialog(
  WidgetTester tester, {
  List<LongTaskSnapshot> tasks = const [],
  Cloud115MonitorSummary monitorSummary = const Cloud115MonitorSummary(),
  bool monitorStatusUnknown = false,
  required Future<ExitPreparationResult> Function() prepareForExit,
}) async {
  final closed = Completer<bool?>();
  await tester.pumpWidget(
    MaterialApp(
      home: Builder(
        builder: (context) => Scaffold(
          body: Center(
            child: TextButton(
              onPressed: () {
                unawaited(
                  showDialog<bool>(
                    context: context,
                    builder: (_) => ExitWorkDialog(
                      tasks: tasks,
                      monitorSummary: monitorSummary,
                      monitorStatusUnknown: monitorStatusUnknown,
                      prepareForExit: prepareForExit,
                    ),
                  ).then((value) {
                    if (!closed.isCompleted) closed.complete(value);
                  }),
                );
              },
              child: const Text('打开退出确认'),
            ),
          ),
        ),
      ),
    ),
  );
  await tester.tap(find.text('打开退出确认'));
  await tester.pumpAndSettle();
  return closed;
}

void main() {
  testWidgets('continue leaves work running and does not prepare it', (
    tester,
  ) async {
    var preparations = 0;
    final closed = await _openDialog(
      tester,
      tasks: [_task()],
      prepareForExit: () async {
        preparations++;
        return const ExitPreparationResult(
          completed: true,
          failedLabels: [],
          unpreparedLabels: [],
          timedOut: false,
        );
      },
    );

    await tester.tap(find.text('继续使用 RCH'));
    await tester.pumpAndSettle();

    expect(await closed.future, isFalse);
    expect(preparations, 0);
    expect(find.text('EH 订阅扫描'), findsNothing);
  });

  testWidgets('successful preparation exits and explains remote continuation', (
    tester,
  ) async {
    var preparations = 0;
    final closed = await _openDialog(
      tester,
      tasks: [_task(canPrepareForExit: true)],
      monitorSummary: const Cloud115MonitorSummary(trackedCount: 2),
      prepareForExit: () async {
        preparations++;
        return const ExitPreparationResult(
          completed: true,
          failedLabels: [],
          unpreparedLabels: [],
          timedOut: false,
        );
      },
    );

    expect(find.textContaining('115 云端下载继续'), findsOneWidget);
    await tester.tap(find.text('退出并暂停可暂停任务'));
    await tester.pumpAndSettle();

    expect(await closed.future, isTrue);
    expect(preparations, 1);
  });

  testWidgets('failed preparation remains visible and can be canceled', (
    tester,
  ) async {
    final closed = await _openDialog(
      tester,
      tasks: [_task(canPrepareForExit: true)],
      prepareForExit: () async => const ExitPreparationResult(
        completed: false,
        failedLabels: ['EH 订阅扫描'],
        unpreparedLabels: [],
        timedOut: false,
      ),
    );

    await tester.tap(find.text('退出并暂停可暂停任务'));
    await tester.pumpAndSettle();

    expect(find.textContaining('无法暂停：EH 订阅扫描'), findsOneWidget);
    expect(find.text('继续使用 RCH'), findsOneWidget);
    expect(closed.isCompleted, isFalse);
    await tester.tap(find.text('继续使用 RCH'));
    await tester.pumpAndSettle();
    expect(await closed.future, isFalse);
  });

  testWidgets('unknown persisted monitor state is not shown as empty', (
    tester,
  ) async {
    final closed = await _openDialog(
      tester,
      monitorStatusUnknown: true,
      prepareForExit: () async => const ExitPreparationResult(
        completed: true,
        failedLabels: [],
        unpreparedLabels: [],
        timedOut: false,
      ),
    );

    expect(find.textContaining('无法读取本地 115 监控状态'), findsOneWidget);
    await tester.tap(find.text('继续使用 RCH'));
    await tester.pumpAndSettle();
    expect(await closed.future, isFalse);
  });

  testWidgets('resumable checkpoint is distinguished from unsafe work', (
    tester,
  ) async {
    final closed = await _openDialog(
      tester,
      tasks: [
        _task(label: 'AI 超分队列', resumable: true),
        _task(label: '备份导入', canPrepareForExit: false),
      ],
      prepareForExit: () async => const ExitPreparationResult(
        completed: true,
        failedLabels: [],
        unpreparedLabels: [],
        timedOut: false,
      ),
    );

    expect(find.textContaining('AI 超分队列。'), findsOneWidget);
    expect(find.textContaining('备份导入'), findsNWidgets(2));
    expect(find.textContaining('没有安全的暂停检查点'), findsOneWidget);
    await tester.tap(find.text('继续使用 RCH'));
    await tester.pumpAndSettle();
    expect(await closed.future, isFalse);
  });

  testWidgets(
    'narrow layout scrolls long Chinese task details without overflow',
    (tester) async {
      tester.view.physicalSize = const Size(360, 640);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      final closed = await _openDialog(
        tester,
        tasks: [
          _task(
            label: '超长中文任务名称：需要继续执行的远程目录扫描与封面处理工作',
            phase: '当前阶段：扫描带有空格与特殊字符的目录并保存检查点',
          ),
        ],
        monitorSummary: const Cloud115MonitorSummary(trackedCount: 300),
        prepareForExit: () async => const ExitPreparationResult(
          completed: true,
          failedLabels: [],
          unpreparedLabels: [],
          timedOut: false,
        ),
      );

      expect(tester.takeException(), isNull);
      expect(find.text('继续使用 RCH'), findsOneWidget);
      await tester.tap(find.text('继续使用 RCH'));
      await tester.pumpAndSettle();
      expect(await closed.future, isFalse);
    },
  );
}
