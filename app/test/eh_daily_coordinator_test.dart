import 'package:app/store/eh_daily_coordinator.dart';
import 'package:app/store/eh_daily_background.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  group('EhDailyCoordinator', () {
    test('default disabled produces no automatic collect request', () async {
      final host = _FakeHost();
      final coordinator = _coordinator(host);

      await coordinator.start();
      await coordinator.wake();

      expect(host.runCalls, 0);
      expect(coordinator.status, EhDailyCoordinatorStatus.disabled);
      expect(host.dailyAutomation.dailyEnabled, isFalse);
      coordinator.dispose();
    });

    test('non-Windows platform never starts the Daily collect lane', () async {
      final host = _FakeHost(
        state: const EhDailyAutomationState(dailyEnabled: true),
      );
      final coordinator = EhDailyCoordinator(
        host: host,
        isWindows: false,
        clock: _FakeClock(
          DateTime(2026, 10, 11, 8),
          untilNextMidnight: const Duration(hours: 8),
        ),
        wakeTimer: _FakeWakeTimer(),
      );

      await coordinator.start();

      expect(host.runCalls, 0);
      expect(coordinator.status, EhDailyCoordinatorStatus.unsupported);
      coordinator.dispose();
    });

    test(
      'Android opt-in registers persistent work and disabling cancels it',
      () async {
        final host = _FakeHost();
        final scheduler = _FakeBackgroundScheduler();
        final coordinator = EhDailyCoordinator(
          host: host,
          isWindows: false,
          isAndroid: true,
          backgroundScheduler: scheduler,
          clock: _FakeClock(
            DateTime(2026, 10, 11),
            untilNextMidnight: const Duration(hours: 8),
          ),
        );
        await coordinator.start();
        expect(scheduler.changes, [false]);
        await host.setDailyEnabled(true);
        await coordinator.settingsChanged();
        expect(scheduler.changes, [false, true]);
        expect(host.runCalls, 1);
        await host.setDailyEnabled(false);
        await coordinator.settingsChanged();
        expect(scheduler.changes, [false, true, false]);
        coordinator.dispose();
      },
    );

    test('an explicit enable starts the first eligible daily check', () async {
      final host = _FakeHost();
      final coordinator = _coordinator(host);
      await coordinator.start();

      await host.setDailyEnabled(true);
      await coordinator.settingsChanged();

      expect(host.runCalls, 1);
      expect(host.dailyAutomation.lastAutoAttemptDate, '2026-10-11');
      coordinator.dispose();
    });

    test(
      'explicit enable claims and runs at most once on a local date',
      () async {
        final host = _FakeHost(
          state: const EhDailyAutomationState(dailyEnabled: true),
        );
        final coordinator = _coordinator(host);

        await coordinator.start();
        await coordinator.wake();

        expect(host.runCalls, 1);
        expect(host.dailyAutomation.lastAutoAttemptDate, '2026-10-11');
        expect(
          host.dailyAutomation.lastRun?.status,
          EhDailyRunStatus.completed,
        );
        expect(host.claimWasPersistedBeforeRun, isTrue);
        expect(host.offline115Requests, 0);
        coordinator.dispose();
      },
    );

    test(
      'restart after a completed attempt does not repeat that date',
      () async {
        final host = _FakeHost(
          state: const EhDailyAutomationState(
            dailyEnabled: true,
            lastAutoAttemptDate: '2026-10-11',
            lastRun: EhDailyRunRecord(
              date: '2026-10-11',
              source: EhDailyRunSource.dailyAuto,
              status: EhDailyRunStatus.completed,
            ),
          ),
        );
        final coordinator = _coordinator(host);

        await coordinator.start();

        expect(host.runCalls, 0);
        expect(coordinator.status, EhDailyCoordinatorStatus.waitingForNextDate);
        coordinator.dispose();
      },
    );

    test('date rollback never reclaims an earlier local date', () async {
      final host = _FakeHost(
        state: const EhDailyAutomationState(
          dailyEnabled: true,
          lastAutoAttemptDate: '2026-10-12',
        ),
      );
      final coordinator = _coordinator(host, now: DateTime(2026, 10, 11, 9));

      await coordinator.start();

      expect(host.runCalls, 0);
      expect(coordinator.status, EhDailyCoordinatorStatus.waitingForNextDate);
      coordinator.dispose();
    });

    test(
      'crossing midnight permits one new attempt and timezone rollback does not',
      () async {
        final host = _FakeHost(
          state: const EhDailyAutomationState(dailyEnabled: true),
        );
        final clock = _FakeClock(
          DateTime(2026, 10, 11, 23, 59),
          untilNextMidnight: const Duration(minutes: 1),
        );
        final timer = _FakeWakeTimer();
        final coordinator = EhDailyCoordinator(
          host: host,
          isWindows: true,
          clock: clock,
          wakeTimer: timer,
        );

        await coordinator.start();
        expect(host.runCalls, 1);
        clock.value = DateTime(2026, 10, 12, 0, 1);
        await coordinator.wake();
        expect(host.runCalls, 2);
        clock.value = DateTime(2026, 10, 11, 23, 30);
        await coordinator.wake();
        expect(host.runCalls, 2);
        coordinator.dispose();
      },
    );

    test(
      'manual collection owns the shared lane before Daily claims it',
      () async {
        final host = _FakeHost(
          state: const EhDailyAutomationState(dailyEnabled: true),
          busy: true,
        );
        final coordinator = _coordinator(host);

        await coordinator.start();

        expect(host.runCalls, 0);
        expect(host.dailyAutomation.lastAutoAttemptDate, isNull);
        expect(
          coordinator.status,
          EhDailyCoordinatorStatus.waitingForCurrentRun,
        );

        host.setBusy(false);
        await Future<void>.delayed(Duration.zero);

        expect(host.runCalls, 1);
        coordinator.dispose();
      },
    );

    test('claim persistence failure prevents all network work', () async {
      final host = _FakeHost(
        state: const EhDailyAutomationState(dailyEnabled: true),
        failNextSave: true,
      );
      final coordinator = _coordinator(host);

      await coordinator.start();

      expect(host.runCalls, 0);
      expect(coordinator.status, EhDailyCoordinatorStatus.persistenceFailed);
      coordinator.dispose();
    });

    test(
      'an active workflow lease prevents claiming or starting another run',
      () async {
        final host = _FakeHost(
          state: const EhDailyAutomationState(dailyEnabled: true),
          leaseAvailable: false,
        );
        final coordinator = _coordinator(host);

        await coordinator.start();

        expect(host.dailyAutomation.lastAutoAttemptDate, isNull);
        expect(host.runCalls, 0);
        expect(
          coordinator.status,
          EhDailyCoordinatorStatus.waitingForCurrentRun,
        );
        coordinator.dispose();
      },
    );

    test(
      'a cross-process manual run conflict waits without starting Daily work',
      () async {
        final host = _FakeHost(
          state: const EhDailyAutomationState(dailyEnabled: true),
          claimConflictRun: const EhDailyRunRecord(
            date: '2026-10-11',
            source: EhDailyRunSource.manual,
            status: EhDailyRunStatus.running,
          ),
        );
        final coordinator = _coordinator(host);

        await coordinator.start();

        expect(host.runCalls, 0);
        expect(host.dailyAutomation.lastAutoAttemptDate, isNull);
        expect(host.dailyAutomation.lastRun?.source, EhDailyRunSource.manual);
        expect(
          coordinator.status,
          EhDailyCoordinatorStatus.waitingForCurrentRun,
        );
        coordinator.dispose();
      },
    );

    test(
      'an interrupted automatic run resumes with the same identity after restart',
      () async {
        final host = _FakeHost(
          state: const EhDailyAutomationState(
            dailyEnabled: true,
            lastAutoAttemptDate: '2026-10-11',
            lastRun: EhDailyRunRecord(
              date: '2026-10-11',
              source: EhDailyRunSource.dailyAuto,
              status: EhDailyRunStatus.running,
              runId: 'daily-2026-10-11',
            ),
          ),
        );
        final coordinator = _coordinator(host);

        await coordinator.start();

        expect(host.runCalls, 1);
        expect(host.resumedRunId, 'daily-2026-10-11');
        expect(
          host.dailyAutomation.lastRun?.status,
          EhDailyRunStatus.completed,
        );
        expect(coordinator.status, EhDailyCoordinatorStatus.completed);
        coordinator.dispose();
      },
    );

    test('manual result cannot hide an interrupted automatic run', () async {
      final host = _FakeHost(
        allowResume: false,
        state: EhDailyAutomationState.fromJson({
          'daily_enabled': true,
          'last_auto_attempt_date': '2026-10-11',
          'last_auto_run': {
            'date': '2026-10-11',
            'source': 'daily_auto',
            'status': 'interrupted_unknown',
            'run_id': 'daily-2026-10-11',
          },
          'last_run': {
            'date': '2026-10-11',
            'source': 'manual',
            'status': 'completed',
          },
        }),
      );
      final coordinator = _coordinator(host);

      await coordinator.start();

      expect(host.runCalls, 0);
      expect(coordinator.status, EhDailyCoordinatorStatus.interruptedUnknown);
      coordinator.dispose();
    });

    test(
      'an exhausted interrupted run does not prevent tomorrow from starting',
      () async {
        final host = _FakeHost(
          allowResume: false,
          state: const EhDailyAutomationState(
            dailyEnabled: true,
            lastAutoAttemptDate: '2026-10-10',
            lastAutoRun: EhDailyRunRecord(
              date: '2026-10-10',
              source: EhDailyRunSource.dailyAuto,
              status: EhDailyRunStatus.interruptedUnknown,
              runId: 'daily-2026-10-10',
              resumeCount: 2,
            ),
          ),
        );
        final coordinator = _coordinator(host);

        await coordinator.start();

        expect(host.runCalls, 1);
        expect(host.dailyAutomation.lastAutoAttemptDate, '2026-10-11');
        coordinator.dispose();
      },
    );

    test(
      'DST-sized local day is scheduled using the injected boundary',
      () async {
        final host = _FakeHost(
          state: const EhDailyAutomationState(dailyEnabled: true),
        );
        final wakeTimer = _FakeWakeTimer();
        final coordinator = _coordinator(
          host,
          wakeTimer: wakeTimer,
          untilNextMidnight: const Duration(hours: 23),
        );

        await coordinator.start();

        expect(wakeTimer.delay, const Duration(hours: 23));
        coordinator.dispose();
      },
    );

    test('fall-back DST day uses the 25-hour local boundary', () async {
      final host = _FakeHost(
        state: const EhDailyAutomationState(dailyEnabled: true),
      );
      final wakeTimer = _FakeWakeTimer();
      final coordinator = _coordinator(
        host,
        wakeTimer: wakeTimer,
        untilNextMidnight: const Duration(hours: 25),
      );

      await coordinator.start();

      expect(wakeTimer.delay, const Duration(hours: 25));
      coordinator.dispose();
    });

    test(
      'a later wake can retry a failed EH scan without claiming a new run',
      () async {
        final host = _FakeHost(
          state: const EhDailyAutomationState(dailyEnabled: true),
          nextOutcome: const EhRunOutcome(
            started: true,
            status: EhDailyRunStatus.failed,
            error: 'EH read timeout',
          ),
        );
        final coordinator = _coordinator(host);

        await coordinator.start();
        final runId = host.dailyAutomation.lastRun?.runId;
        host.nextOutcome = const EhRunOutcome(
          started: true,
          status: EhDailyRunStatus.completed,
        );
        await coordinator.wake();

        expect(host.runCalls, 2);
        expect(host.resumedRunId, runId);
        expect(
          host.dailyAutomation.lastRun?.status,
          EhDailyRunStatus.completed,
        );
        coordinator.dispose();
      },
    );

    test('session-unavailable partial resumes once at a later wake', () async {
      final host = _FakeHost(
        state: const EhDailyAutomationState(
          dailyEnabled: true,
          lastAutoAttemptDate: '2026-10-11',
          lastAutoRun: EhDailyRunRecord(
            date: '2026-10-11',
            source: EhDailyRunSource.dailyAuto,
            status: EhDailyRunStatus.partial,
            runId: 'daily-2026-10-11',
            cloudStage: EhDailyCloudStageResult(
              status: EhDailyCloudStageStatus.sessionUnavailable,
            ),
          ),
        ),
      );
      final coordinator = _coordinator(host);

      await coordinator.start();

      expect(host.runCalls, 1);
      expect(host.resumedRunId, 'daily-2026-10-11');
      coordinator.dispose();
    });

    test(
      'cancelled run is recorded and is not automatically repeated',
      () async {
        final host = _FakeHost(
          state: const EhDailyAutomationState(dailyEnabled: true),
          nextOutcome: const EhRunOutcome(
            started: true,
            status: EhDailyRunStatus.cancelled,
            error: 'user cancelled',
          ),
        );
        final coordinator = _coordinator(host);

        await coordinator.start();
        await coordinator.wake();

        expect(host.runCalls, 1);
        expect(
          host.dailyAutomation.lastRun?.status,
          EhDailyRunStatus.cancelled,
        );
        coordinator.dispose();
      },
    );
  });
}

EhDailyCoordinator _coordinator(
  _FakeHost host, {
  DateTime? now,
  Duration untilNextMidnight = const Duration(hours: 8),
  _FakeWakeTimer? wakeTimer,
}) => EhDailyCoordinator(
  host: host,
  isWindows: true,
  clock: _FakeClock(
    now ?? DateTime(2026, 10, 11, 8),
    untilNextMidnight: untilNextMidnight,
  ),
  wakeTimer: wakeTimer ?? _FakeWakeTimer(),
);

class _FakeClock implements EhDailyClock {
  _FakeClock(this.value, {required this.untilNextMidnight});

  DateTime value;

  final Duration untilNextMidnight;

  @override
  DateTime nowLocal() => value;

  @override
  Duration untilNextLocalMidnight() => untilNextMidnight;
}

class _FakeBackgroundScheduler implements EhDailyBackgroundScheduler {
  final changes = <bool>[];
  @override
  Future<void> sync(bool enabled) async => changes.add(enabled);
}

class _FakeWakeTimer implements EhDailyWakeTimer {
  Duration? delay;
  void Function()? callback;

  @override
  void cancel() {
    callback = null;
  }

  @override
  void schedule(Duration delay, void Function() callback) {
    this.delay = delay;
    this.callback = callback;
  }
}

class _FakeHost extends ChangeNotifier implements EhDailyHost {
  _FakeHost({
    EhDailyAutomationState state = const EhDailyAutomationState(),
    this.busy = false,
    this.failNextSave = false,
    this.claimConflictRun,
    this.leaseAvailable = true,
    this.allowResume = true,
    this.nextOutcome = const EhRunOutcome(
      started: true,
      status: EhDailyRunStatus.completed,
      saved: 2,
    ),
  }) : dailyAutomation = state;

  @override
  EhDailyAutomationState dailyAutomation;

  @override
  bool busy;
  bool failNextSave;
  final EhDailyRunRecord? claimConflictRun;
  bool leaseAvailable;
  bool allowResume;
  String? resumedRunId;
  int runCalls = 0;
  int offline115Requests = 0;
  bool claimWasPersistedBeforeRun = false;
  EhRunOutcome nextOutcome;

  @override
  Future<BigInt?> tryAcquireWorkflowLease() async =>
      leaseAvailable ? BigInt.one : null;

  @override
  Future<void> releaseWorkflowLease(BigInt lease) async {}

  @override
  Future<bool> resumeDailyRun(String date, BigInt lease) async {
    final run = dailyAutomation.effectiveLastAutoRun;
    if (!allowResume ||
        run?.date != date ||
        run == null ||
        run.resumeCount >= 2 ||
        !(run.status == EhDailyRunStatus.interruptedUnknown ||
            (run.status == EhDailyRunStatus.failed &&
                run.cloudStage.status == EhDailyCloudStageStatus.notRun) ||
            (run.status == EhDailyRunStatus.partial &&
                (run.cloudStage.status ==
                        EhDailyCloudStageStatus.needsConfiguration ||
                    run.cloudStage.status ==
                        EhDailyCloudStageStatus.sessionUnavailable)))) {
      return false;
    }
    resumedRunId = run.runId;
    final resumed = run.copyWith(
      status: EhDailyRunStatus.claimed,
      resumeCount: run.resumeCount + 1,
    );
    dailyAutomation = dailyAutomation.copyWith(
      lastRun: resumed,
      lastAutoRun: resumed,
    );
    return true;
  }

  @override
  bool get hasOutDir => true;

  @override
  bool get dailyReady => true;

  Future<void> saveDailyAutomation(EhDailyAutomationState state) async {
    if (failNextSave) {
      failNextSave = false;
      throw StateError('injected persistence failure');
    }
    dailyAutomation = state;
    notifyListeners();
  }

  @override
  Future<void> setDailyEnabled(bool enabled) async {
    await saveDailyAutomation(dailyAutomation.copyWith(dailyEnabled: enabled));
  }

  @override
  Future<bool> claimDailyRun(String date) async {
    if (failNextSave) {
      failNextSave = false;
      throw StateError('injected persistence failure');
    }
    if (!dailyAutomation.dailyEnabled ||
        (dailyAutomation.lastAutoAttemptDate != null &&
            date.compareTo(dailyAutomation.lastAutoAttemptDate!) <= 0)) {
      return false;
    }
    if (claimConflictRun != null) {
      dailyAutomation = dailyAutomation.copyWith(lastRun: claimConflictRun);
      notifyListeners();
      return false;
    }
    dailyAutomation = dailyAutomation.copyWith(
      lastAutoAttemptDate: date,
      lastRun: EhDailyRunRecord(
        date: date,
        source: EhDailyRunSource.dailyAuto,
        status: EhDailyRunStatus.claimed,
        runId: 'daily-$date',
      ),
    );
    notifyListeners();
    return true;
  }

  @override
  Future<void> updateDailyRun(EhDailyRunRecord run) async {
    await saveDailyAutomation(dailyAutomation.copyWith(lastRun: run));
  }

  @override
  Future<void> recoverDailyRun() async {
    final current = dailyAutomation.lastRun;
    if (current == null ||
        (current.status != EhDailyRunStatus.claimed &&
            current.status != EhDailyRunStatus.running)) {
      return;
    }
    await saveDailyAutomation(
      dailyAutomation.copyWith(
        lastRun: current.copyWith(
          status: EhDailyRunStatus.interruptedUnknown,
          error: '上次进程退出时任务尚未结束，远端结果未知；本轮不会自动重试。',
        ),
      ),
    );
  }

  @override
  Future<EhRunOutcome> run({
    required EhDailyRunSource source,
    BigInt? workflowLease,
  }) async {
    if (busy) return EhRunOutcome.busy;
    runCalls++;
    claimWasPersistedBeforeRun =
        source != EhDailyRunSource.dailyAuto ||
        (dailyAutomation.lastAutoAttemptDate != null &&
            dailyAutomation.lastRun?.status == EhDailyRunStatus.claimed);
    busy = true;
    notifyListeners();
    final outcome = nextOutcome;
    dailyAutomation = dailyAutomation.copyWith(
      lastRun: EhDailyRunRecord(
        date: dailyAutomation.lastRun?.date ?? '2026-10-11',
        source: source,
        status: outcome.status,
        runId: dailyAutomation.lastRun?.runId,
        saved: outcome.saved,
        error: outcome.error,
      ),
    );
    busy = false;
    notifyListeners();
    return outcome;
  }

  void setBusy(bool value) {
    busy = value;
    notifyListeners();
  }
}
