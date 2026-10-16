import 'dart:async';

import 'package:app/store/long_task_registry.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test(
    'exit prompt fails closed when durable monitor status is unreadable',
    () {
      expect(
        shouldPromptBeforeExit(
          hasActiveTasks: false,
          hasResumableWork: false,
          monitorStatusKnown: false,
        ),
        isTrue,
      );
      expect(
        shouldPromptBeforeExit(
          hasActiveTasks: false,
          hasResumableWork: false,
          monitorStatusKnown: true,
        ),
        isFalse,
      );
      expect(
        shouldPromptBeforeExit(
          hasActiveTasks: false,
          hasResumableWork: true,
          monitorStatusKnown: true,
        ),
        isTrue,
      );
    },
  );

  test('same-label leases stay independent and snapshots are stable', () async {
    final registry = LongTaskRegistry();
    final first = registry.register(
      kind: 'eh_scan',
      label: 'EH 扫描',
      phase: '搜索',
    );
    final second = registry.register(
      kind: 'remote_scan',
      label: 'EH 扫描',
      phase: '目录读取',
    );

    registry.update(first, phase: '下载 Torrent');
    final snapshot = registry.snapshot();
    expect(snapshot, hasLength(2));
    expect(snapshot.map((task) => task.phase), ['下载 Torrent', '目录读取']);
    first.dispose();
    expect(registry.snapshot().map((task) => task.kind), ['remote_scan']);
    second.dispose();
    expect(registry.snapshot(), isEmpty);
  });

  test('exit preparation runs callbacks and reports failures', () async {
    final registry = LongTaskRegistry();
    var prepared = 0;
    registry.register(
      kind: 'eh_scan',
      label: 'EH 扫描',
      phase: '运行中',
      prepareForExit: () async => prepared++,
    );
    registry.register(
      kind: 'sync',
      label: '同步',
      phase: '上传',
      prepareForExit: () async => throw StateError('flush failed'),
    );

    final result = await registry.prepareForExit(
      registry.snapshot(),
      timeout: const Duration(seconds: 1),
    );
    expect(prepared, 1);
    expect(result.completed, isFalse);
    expect(result.failedLabels, ['同步']);
    expect(result.timedOut, isFalse);
  });

  test(
    'exit preparation reports timeout without claiming completion',
    () async {
      final registry = LongTaskRegistry();
      final release = Completer<void>();
      registry.register(
        kind: 'sync',
        label: '同步',
        phase: '运行中',
        prepareForExit: () => release.future,
      );

      final result = await registry.prepareForExit(
        registry.snapshot(),
        timeout: const Duration(milliseconds: 10),
      );
      expect(result.completed, isFalse);
      expect(result.timedOut, isTrue);
      release.complete();
    },
  );

  test('run releases its lease after success and error', () async {
    final registry = LongTaskRegistry();
    await registry.run<void>(
      kind: 'short',
      label: '短任务',
      phase: '运行中',
      action: (_) async {},
    );
    expect(registry.snapshot(), isEmpty);

    await expectLater(
      registry.run<void>(
        kind: 'short',
        label: '失败任务',
        phase: '运行中',
        action: (_) async => throw StateError('expected'),
      ),
      throwsStateError,
    );
    expect(registry.snapshot(), isEmpty);
  });

  test(
    'exit request gate rejects a duplicate and reopens afterwards',
    () async {
      final gate = ExitRequestGate();
      final release = Completer<int>();
      var actions = 0;
      final first = gate.run<int>(
        duplicateResult: -1,
        action: () {
          actions++;
          return release.future;
        },
      );
      expect(gate.inProgress, isTrue);
      final duplicate = await gate.run<int>(
        duplicateResult: -1,
        action: () async {
          actions++;
          return 2;
        },
      );
      expect(duplicate, -1);
      expect(actions, 1);

      release.complete(1);
      expect(await first, 1);
      expect(gate.inProgress, isFalse);
      expect(
        await gate.run<int>(
          duplicateResult: -1,
          action: () async {
            actions++;
            return 3;
          },
        ),
        3,
      );
      expect(actions, 2);
    },
  );
}
