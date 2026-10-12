import 'dart:async';
import 'dart:io';

import 'package:flutter/widgets.dart';

enum EhDailyRunSource { manual, dailyAuto, unknown }

enum EhDailyRunStatus {
  claimed,
  running,
  completed,
  failed,
  cancelled,
  interruptedUnknown,
  unknown,
}

enum EhDailyCoordinatorStatus {
  stopped,
  unsupported,
  disabled,
  waitingForOutputDirectory,
  waitingForCurrentRun,
  waitingForNextDate,
  persistenceFailed,
  claimed,
  running,
  completed,
  failed,
  cancelled,
  interruptedUnknown,
}

class EhDailyRunRecord {
  const EhDailyRunRecord({
    required this.date,
    required this.source,
    required this.status,
    this.saved = 0,
    this.error,
  });

  final String date;
  final EhDailyRunSource source;
  final EhDailyRunStatus status;
  final int saved;
  final String? error;

  EhDailyRunRecord copyWith({
    String? date,
    EhDailyRunSource? source,
    EhDailyRunStatus? status,
    int? saved,
    Object? error = _unchanged,
  }) => EhDailyRunRecord(
    date: date ?? this.date,
    source: source ?? this.source,
    status: status ?? this.status,
    saved: saved ?? this.saved,
    error: identical(error, _unchanged) ? this.error : error as String?,
  );

  Map<String, Object?> toJson() => {
    'date': date,
    'source': _sourceToJson(source),
    'status': _statusToJson(status),
    'saved': saved,
    'error': error,
  };

  factory EhDailyRunRecord.fromJson(Map<String, Object?> json) =>
      EhDailyRunRecord(
        date: json['date'] as String? ?? '',
        source: _sourceFromJson(json['source']),
        status: _statusFromJson(json['status']),
        saved: (json['saved'] as num?)?.toInt() ?? 0,
        error: json['error'] as String?,
      );
}

class EhDailyAutomationState {
  const EhDailyAutomationState({
    this.dailyEnabled = false,
    this.lastAutoAttemptDate,
    this.lastRun,
  });

  final bool dailyEnabled;
  final String? lastAutoAttemptDate;
  final EhDailyRunRecord? lastRun;

  EhDailyAutomationState copyWith({
    bool? dailyEnabled,
    Object? lastAutoAttemptDate = _unchanged,
    Object? lastRun = _unchanged,
  }) => EhDailyAutomationState(
    dailyEnabled: dailyEnabled ?? this.dailyEnabled,
    lastAutoAttemptDate: identical(lastAutoAttemptDate, _unchanged)
        ? this.lastAutoAttemptDate
        : lastAutoAttemptDate as String?,
    lastRun: identical(lastRun, _unchanged)
        ? this.lastRun
        : lastRun as EhDailyRunRecord?,
  );

  Map<String, Object?> toJson() => {
    'daily_enabled': dailyEnabled,
    'last_auto_attempt_date': lastAutoAttemptDate,
    'last_run': lastRun?.toJson(),
  };

  factory EhDailyAutomationState.fromJson(Map<String, Object?> json) {
    final rawRun = json['last_run'];
    return EhDailyAutomationState(
      dailyEnabled: json['daily_enabled'] as bool? ?? false,
      lastAutoAttemptDate: json['last_auto_attempt_date'] as String?,
      lastRun: rawRun is Map
          ? EhDailyRunRecord.fromJson(Map<String, Object?>.from(rawRun))
          : null,
    );
  }
}

class EhRunOutcome {
  const EhRunOutcome({
    required this.started,
    required this.status,
    this.saved = 0,
    this.error,
  });

  static const busy = EhRunOutcome(
    started: false,
    status: EhDailyRunStatus.unknown,
  );

  final bool started;
  final EhDailyRunStatus status;
  final int saved;
  final String? error;
}

abstract interface class EhDailyHost implements Listenable {
  EhDailyAutomationState get dailyAutomation;
  bool get dailyReady;
  bool get hasOutDir;
  bool get busy;

  Future<void> setDailyEnabled(bool enabled);
  Future<bool> claimDailyRun(String date);
  Future<void> updateDailyRun(EhDailyRunRecord run);
  Future<void> recoverDailyRun();
  Future<EhRunOutcome> run({required EhDailyRunSource source});
}

abstract interface class EhDailyClock {
  DateTime nowLocal();
  Duration untilNextLocalMidnight();
}

class SystemEhDailyClock implements EhDailyClock {
  const SystemEhDailyClock();

  @override
  DateTime nowLocal() => DateTime.now();

  @override
  Duration untilNextLocalMidnight() {
    final now = DateTime.now();
    final next = DateTime(now.year, now.month, now.day + 1);
    final delay = next.difference(now);
    return delay.isNegative || delay == Duration.zero
        ? const Duration(seconds: 1)
        : delay;
  }
}

abstract interface class EhDailyWakeTimer {
  void schedule(Duration delay, VoidCallback callback);
  void cancel();
}

class _SystemEhDailyWakeTimer implements EhDailyWakeTimer {
  Timer? _timer;

  @override
  void cancel() {
    _timer?.cancel();
    _timer = null;
  }

  @override
  void schedule(Duration delay, VoidCallback callback) {
    cancel();
    _timer = Timer(delay, callback);
  }
}

/// Runs at most one opted-in EH collection attempt for each advancing local
/// calendar date while the Windows RCH process is alive.
class EhDailyCoordinator extends ChangeNotifier with WidgetsBindingObserver {
  EhDailyCoordinator({
    required this.host,
    bool? isWindows,
    this.clock = const SystemEhDailyClock(),
    EhDailyWakeTimer? wakeTimer,
  }) : _isWindows = isWindows ?? Platform.isWindows,
       _wakeTimer = wakeTimer ?? _SystemEhDailyWakeTimer();

  static EhDailyCoordinator? _instance;

  static EhDailyCoordinator get instance =>
      _instance ??
      (throw StateError('EH Daily coordinator has not been initialized'));

  static EhDailyCoordinator? get maybeInstance => _instance;

  static void initialize(EhDailyHost host) {
    _instance ??= EhDailyCoordinator(host: host);
  }

  final EhDailyHost host;
  final bool _isWindows;
  final EhDailyClock clock;
  final EhDailyWakeTimer _wakeTimer;

  EhDailyCoordinatorStatus status = EhDailyCoordinatorStatus.stopped;
  String? error;
  bool _started = false;
  bool _evaluating = false;
  bool _wakeQueued = false;
  bool _wasBusy = false;
  bool _waitingForCurrentRun = false;
  bool _launchingClaim = false;
  String? _pendingClaimDate;

  Future<void> start() async {
    if (!_isWindows) {
      status = EhDailyCoordinatorStatus.unsupported;
      notifyListeners();
      return;
    }
    if (_started) return;
    if (!host.dailyReady) {
      _setStatus(
        EhDailyCoordinatorStatus.persistenceFailed,
        'EH 规则尚未成功加载，Daily 自动收集保持停止。',
      );
      return;
    }

    _started = true;
    _wasBusy = host.busy;
    host.addListener(_onHostChanged);
    WidgetsBinding.instance.addObserver(this);

    _armNextLocalDay();
    await wake();
  }

  /// Re-evaluates on app resume, rule/output changes, or the next local day.
  Future<void> wake() async {
    if (!_started || !_isWindows) return;
    _armNextLocalDay();
    if (_evaluating) {
      _wakeQueued = true;
      return;
    }

    _evaluating = true;
    try {
      try {
        await host.recoverDailyRun();
      } catch (e) {
        _setStatus(EhDailyCoordinatorStatus.persistenceFailed, '$e');
        return;
      }
      await _evaluate();
    } finally {
      _evaluating = false;
      if (_wakeQueued) {
        _wakeQueued = false;
        scheduleMicrotask(() => unawaited(wake()));
      }
    }
  }

  Future<void> settingsChanged() => _started ? wake() : start();

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    if (state == AppLifecycleState.resumed) unawaited(wake());
  }

  Future<void> _evaluate() async {
    final automation = host.dailyAutomation;
    if (!automation.dailyEnabled) {
      _pendingClaimDate = null;
      _waitingForCurrentRun = false;
      _setStatus(EhDailyCoordinatorStatus.disabled);
      return;
    }
    if (!host.hasOutDir) {
      _setStatus(EhDailyCoordinatorStatus.waitingForOutputDirectory);
      return;
    }

    if (_pendingClaimDate != null) {
      await _launchPendingClaim();
      return;
    }

    final today = _dateKey(clock.nowLocal());
    final lastAttempt = automation.lastAutoAttemptDate;
    if (lastAttempt != null && !_isValidDateKey(lastAttempt)) {
      _setStatus(
        EhDailyCoordinatorStatus.persistenceFailed,
        'Daily 配置中的上次尝试日期无效，已停止自动收集。',
      );
      return;
    }
    if (lastAttempt != null && today.compareTo(lastAttempt) <= 0) {
      final currentStatus = automation.lastRun?.status;
      _setStatus(switch (currentStatus) {
        EhDailyRunStatus.interruptedUnknown =>
          EhDailyCoordinatorStatus.interruptedUnknown,
        EhDailyRunStatus.claimed ||
        EhDailyRunStatus.running => EhDailyCoordinatorStatus.running,
        _ => EhDailyCoordinatorStatus.waitingForNextDate,
      });
      return;
    }
    final previousRun = automation.lastRun;
    if ((previousRun?.status == EhDailyRunStatus.claimed ||
            previousRun?.status == EhDailyRunStatus.running) &&
        !_isValidDateKey(previousRun!.date)) {
      _setStatus(
        EhDailyCoordinatorStatus.persistenceFailed,
        'Daily 配置中的运行日期无效，已停止自动收集。',
      );
      return;
    }
    if (previousRun?.status == EhDailyRunStatus.claimed ||
        previousRun?.status == EhDailyRunStatus.running) {
      _setStatus(EhDailyCoordinatorStatus.running);
      return;
    }
    if (previousRun?.status == EhDailyRunStatus.interruptedUnknown &&
        !_isValidDateKey(previousRun!.date)) {
      _setStatus(
        EhDailyCoordinatorStatus.persistenceFailed,
        'Daily 配置中的中断日期无效，已停止自动收集。',
      );
      return;
    }
    if (previousRun?.status == EhDailyRunStatus.interruptedUnknown &&
        today.compareTo(previousRun!.date) <= 0) {
      _setStatus(EhDailyCoordinatorStatus.interruptedUnknown);
      return;
    }

    if (host.busy) {
      _waitingForCurrentRun = true;
      _setStatus(EhDailyCoordinatorStatus.waitingForCurrentRun);
      return;
    }

    try {
      if (!await host.claimDailyRun(today)) {
        final current = host.dailyAutomation;
        if (!current.dailyEnabled) {
          _setStatus(EhDailyCoordinatorStatus.disabled);
        } else if (current.lastAutoAttemptDate != null &&
            today.compareTo(current.lastAutoAttemptDate!) <= 0) {
          _setStatus(
            current.lastRun?.status == EhDailyRunStatus.interruptedUnknown
                ? EhDailyCoordinatorStatus.interruptedUnknown
                : EhDailyCoordinatorStatus.waitingForNextDate,
          );
        } else if (current.lastRun?.status == EhDailyRunStatus.claimed ||
            current.lastRun?.status == EhDailyRunStatus.running) {
          _setStatus(EhDailyCoordinatorStatus.waitingForCurrentRun);
        } else {
          _setStatus(
            EhDailyCoordinatorStatus.persistenceFailed,
            '无法确认本轮 Daily 领取结果，已停止自动收集。',
          );
        }
        return;
      }
    } catch (e) {
      _setStatus(EhDailyCoordinatorStatus.persistenceFailed, '$e');
      return;
    }
    _pendingClaimDate = today;
    _waitingForCurrentRun = false;
    _setStatus(EhDailyCoordinatorStatus.claimed);
    await _launchPendingClaim();
  }

  Future<void> _launchPendingClaim() async {
    final claimedDate = _pendingClaimDate;
    if (claimedDate == null || _launchingClaim) return;
    if (host.busy) {
      _waitingForCurrentRun = true;
      _setStatus(EhDailyCoordinatorStatus.waitingForCurrentRun);
      return;
    }

    if (!host.dailyAutomation.dailyEnabled) {
      _pendingClaimDate = null;
      await _finishClaimAsCancelled(claimedDate, 'Daily 设置已关闭，未启动本轮网络请求。');
      return;
    }

    _launchingClaim = true;
    _setStatus(EhDailyCoordinatorStatus.running);
    try {
      final outcome = await host.run(source: EhDailyRunSource.dailyAuto);
      if (outcome.started) {
        _pendingClaimDate = null;
        _waitingForCurrentRun = false;
        _setStatus(_statusFor(outcome.status), outcome.error);
      } else if (host.busy) {
        _waitingForCurrentRun = true;
        _setStatus(EhDailyCoordinatorStatus.waitingForCurrentRun);
      } else {
        _pendingClaimDate = null;
        await _markClaimFailed(
          claimedDate,
          outcome.error ?? '调度资格已领取，但收集未能启动。',
        );
      }
    } catch (e) {
      _pendingClaimDate = null;
      await _markClaimUnknown(claimedDate, '收集结果未能确认：$e');
    } finally {
      _launchingClaim = false;
      if (_pendingClaimDate != null && !host.busy) {
        scheduleMicrotask(() => unawaited(_launchPendingClaim()));
      }
    }
  }

  Future<void> _finishClaimAsCancelled(String date, String reason) async {
    try {
      await host.updateDailyRun(
        EhDailyRunRecord(
          date: date,
          source: EhDailyRunSource.dailyAuto,
          status: EhDailyRunStatus.cancelled,
          error: reason,
        ),
      );
      _setStatus(EhDailyCoordinatorStatus.cancelled, reason);
    } catch (e) {
      _setStatus(EhDailyCoordinatorStatus.persistenceFailed, '$e');
    }
  }

  Future<void> _markClaimFailed(String date, String reason) async {
    final current = host.dailyAutomation;
    final run = current.lastRun;
    if (run == null ||
        run.date != date ||
        run.status != EhDailyRunStatus.claimed) {
      _setStatus(EhDailyCoordinatorStatus.failed, reason);
      return;
    }
    try {
      await host.updateDailyRun(
        run.copyWith(status: EhDailyRunStatus.failed, error: reason),
      );
      _setStatus(EhDailyCoordinatorStatus.failed, reason);
    } catch (e) {
      _setStatus(EhDailyCoordinatorStatus.persistenceFailed, '$e');
    }
  }

  Future<void> _markClaimUnknown(String date, String reason) async {
    final current = host.dailyAutomation;
    final run = current.lastRun;
    if (run == null || run.date != date) {
      _setStatus(EhDailyCoordinatorStatus.interruptedUnknown, reason);
      return;
    }
    try {
      await host.updateDailyRun(
        run.copyWith(
          status: EhDailyRunStatus.interruptedUnknown,
          error: reason,
        ),
      );
      _setStatus(EhDailyCoordinatorStatus.interruptedUnknown, reason);
    } catch (e) {
      _setStatus(EhDailyCoordinatorStatus.persistenceFailed, '$e');
    }
  }

  void _onHostChanged() {
    final nowBusy = host.busy;
    final finishedRun = _wasBusy && !nowBusy;
    _wasBusy = nowBusy;
    if (finishedRun &&
        (_waitingForCurrentRun || _pendingClaimDate != null) &&
        !_launchingClaim) {
      _waitingForCurrentRun = false;
      unawaited(wake());
    }
  }

  void _armNextLocalDay() {
    if (!_started || !_isWindows || !host.dailyAutomation.dailyEnabled) {
      _wakeTimer.cancel();
      return;
    }
    _wakeTimer.schedule(clock.untilNextLocalMidnight(), () {
      unawaited(wake());
    });
  }

  void _setStatus(EhDailyCoordinatorStatus value, [String? detail]) {
    if (status == value && error == detail) return;
    status = value;
    error = detail;
    notifyListeners();
  }

  @override
  void dispose() {
    _wakeTimer.cancel();
    if (_started) {
      host.removeListener(_onHostChanged);
      WidgetsBinding.instance.removeObserver(this);
    }
    super.dispose();
  }
}

String ehLocalDateKey(DateTime value) => _dateKey(value);

String _dateKey(DateTime value) =>
    '${value.year.toString().padLeft(4, '0')}-'
    '${value.month.toString().padLeft(2, '0')}-'
    '${value.day.toString().padLeft(2, '0')}';

bool _isValidDateKey(String value) {
  if (!RegExp(r'^\d{4}-\d{2}-\d{2}$').hasMatch(value)) return false;
  final parsed = DateTime.tryParse(value);
  return parsed != null && _dateKey(parsed) == value;
}

String _sourceToJson(EhDailyRunSource source) => switch (source) {
  EhDailyRunSource.manual => 'manual',
  EhDailyRunSource.dailyAuto => 'daily_auto',
  EhDailyRunSource.unknown => 'unknown',
};

EhDailyRunSource _sourceFromJson(Object? value) => switch (value) {
  'manual' => EhDailyRunSource.manual,
  'daily_auto' => EhDailyRunSource.dailyAuto,
  _ => EhDailyRunSource.unknown,
};

String _statusToJson(EhDailyRunStatus status) => switch (status) {
  EhDailyRunStatus.claimed => 'claimed',
  EhDailyRunStatus.running => 'running',
  EhDailyRunStatus.completed => 'completed',
  EhDailyRunStatus.failed => 'failed',
  EhDailyRunStatus.cancelled => 'cancelled',
  EhDailyRunStatus.interruptedUnknown => 'interrupted_unknown',
  EhDailyRunStatus.unknown => 'unknown',
};

EhDailyRunStatus _statusFromJson(Object? value) => switch (value) {
  'claimed' => EhDailyRunStatus.claimed,
  'running' => EhDailyRunStatus.running,
  'completed' => EhDailyRunStatus.completed,
  'failed' => EhDailyRunStatus.failed,
  'cancelled' => EhDailyRunStatus.cancelled,
  'interrupted_unknown' => EhDailyRunStatus.interruptedUnknown,
  _ => EhDailyRunStatus.unknown,
};

EhDailyCoordinatorStatus _statusFor(EhDailyRunStatus status) =>
    switch (status) {
      EhDailyRunStatus.claimed => EhDailyCoordinatorStatus.claimed,
      EhDailyRunStatus.running => EhDailyCoordinatorStatus.running,
      EhDailyRunStatus.completed => EhDailyCoordinatorStatus.completed,
      EhDailyRunStatus.failed => EhDailyCoordinatorStatus.failed,
      EhDailyRunStatus.cancelled => EhDailyCoordinatorStatus.cancelled,
      EhDailyRunStatus.interruptedUnknown =>
        EhDailyCoordinatorStatus.interruptedUnknown,
      EhDailyRunStatus.unknown => EhDailyCoordinatorStatus.failed,
    };

const Object _unchanged = Object();
