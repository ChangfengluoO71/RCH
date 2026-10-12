import 'dart:async';

/// Gives user initiated work priority at safe boundaries in routine maintenance.
///
/// Foreground leases are short lived and owned by the operation, while
/// maintenance callers wait between source or proposal slices. A one-shot
/// aging timer grants one bounded slice after the maximum wait so background
/// maintenance cannot be starved forever. No UI polling is involved.
class ForegroundWorkCoordinator {
  ForegroundWorkCoordinator._();

  static final instance = ForegroundWorkCoordinator._();

  static const maximumMaintenanceWait = Duration(minutes: 5);

  int _foregroundCount = 0;
  final Set<Completer<void>> _foregroundChangeWaiters = <Completer<void>>{};

  bool get hasForegroundWork => _foregroundCount > 0;

  Future<T> runForeground<T>(Future<T> Function() work) async {
    _foregroundCount++;
    _notifyForegroundChange();
    try {
      return await work();
    } finally {
      _foregroundCount--;
      if (_foregroundCount == 0) _notifyForegroundChange();
    }
  }

  /// Waits until foreground work is idle, or grants one source sized
  /// maintenance slice after [maximumMaintenanceWait]. Call again at the next
  /// safe boundary before starting another slice.
  Future<void> waitForMaintenanceTurn() async {
    final elapsed = Stopwatch()..start();
    while (_foregroundCount > 0) {
      final remaining = maximumMaintenanceWait - elapsed.elapsed;
      if (remaining <= Duration.zero) return;

      final waiter = Completer<void>();
      _foregroundChangeWaiters.add(waiter);
      final timer = Timer(remaining, () {
        if (!waiter.isCompleted) waiter.complete();
      });
      await waiter.future;
      timer.cancel();
      _foregroundChangeWaiters.remove(waiter);
    }
  }

  void _notifyForegroundChange() {
    final waiters = List<Completer<void>>.of(_foregroundChangeWaiters);
    _foregroundChangeWaiters.clear();
    for (final waiter in waiters) {
      if (!waiter.isCompleted) waiter.complete();
    }
  }
}
