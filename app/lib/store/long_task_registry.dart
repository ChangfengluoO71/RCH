import 'dart:async';

import 'package:flutter/foundation.dart';

typedef PrepareLongTaskForExit = Future<void> Function();

/// An unreadable durable monitor summary is not evidence that no work exists.
bool shouldPromptBeforeExit({
  required bool hasActiveTasks,
  required bool hasResumableWork,
  required bool monitorStatusKnown,
}) => hasActiveTasks || hasResumableWork || !monitorStatusKnown;

/// Serializes concurrent platform close requests. A second request is rejected
/// without running another dialog or preparation callback.
class ExitRequestGate {
  bool _inProgress = false;

  bool get inProgress => _inProgress;

  Future<T> run<T>({
    required T duplicateResult,
    required Future<T> Function() action,
  }) async {
    if (_inProgress) return duplicateResult;
    _inProgress = true;
    try {
      return await action();
    } finally {
      _inProgress = false;
    }
  }
}

class LongTaskSnapshot {
  const LongTaskSnapshot({
    required this.id,
    required this.kind,
    required this.label,
    required this.phase,
    required this.startedAt,
    required this.resumable,
    required this.canPrepareForExit,
  });

  final int id;
  final String kind;
  final String label;
  final String phase;
  final DateTime startedAt;
  final bool resumable;
  final bool canPrepareForExit;
}

class ExitPreparationResult {
  const ExitPreparationResult({
    required this.completed,
    required this.failedLabels,
    required this.unpreparedLabels,
    required this.timedOut,
  });

  final bool completed;
  final List<String> failedLabels;
  final List<String> unpreparedLabels;
  final bool timedOut;
}

class LongTaskRegistry extends ChangeNotifier {
  LongTaskRegistry();

  static final LongTaskRegistry instance = LongTaskRegistry();

  final Map<int, _LongTaskEntry> _entries = {};
  int _nextId = 1;

  LongTaskLease register({
    required String kind,
    required String label,
    required String phase,
    bool resumable = false,
    PrepareLongTaskForExit? prepareForExit,
  }) {
    final normalizedKind = kind.trim();
    final normalizedLabel = label.trim();
    if (normalizedKind.isEmpty || normalizedLabel.isEmpty) {
      throw ArgumentError('kind and label must be non-empty');
    }
    final id = _nextId++;
    _entries[id] = _LongTaskEntry(
      id: id,
      kind: normalizedKind,
      label: normalizedLabel,
      phase: phase.trim(),
      startedAt: DateTime.now(),
      resumable: resumable,
      prepareForExit: prepareForExit,
    );
    notifyListeners();
    return LongTaskLease._(this, id);
  }

  void update(LongTaskLease lease, {String? phase, bool? resumable}) {
    final current = _entries[lease.id];
    if (current == null || !identical(lease._registry, this)) return;
    if (phase != null) current.phase = phase.trim();
    if (resumable != null) current.resumable = resumable;
    notifyListeners();
  }

  List<LongTaskSnapshot> snapshot() => _entries.values
      .map(
        (entry) => LongTaskSnapshot(
          id: entry.id,
          kind: entry.kind,
          label: entry.label,
          phase: entry.phase,
          startedAt: entry.startedAt,
          resumable: entry.resumable,
          canPrepareForExit: entry.prepareForExit != null,
        ),
      )
      .toList(growable: false);

  Future<ExitPreparationResult> prepareForExit(
    List<LongTaskSnapshot> tasks, {
    required Duration timeout,
  }) async {
    final selected = <_LongTaskEntry>[];
    final seen = <int>{};
    for (final task in tasks) {
      if (!seen.add(task.id)) continue;
      final entry = _entries[task.id];
      if (entry != null) selected.add(entry);
    }
    final unprepared = selected
        .where((entry) => entry.prepareForExit == null)
        .map((entry) => entry.label)
        .toList(growable: false);
    final preparable = selected
        .where((entry) => entry.prepareForExit != null)
        .toList(growable: false);
    final failures = <String>[];
    try {
      await Future.wait(
        preparable.map((entry) async {
          try {
            await entry.prepareForExit!();
          } catch (_) {
            failures.add(entry.label);
          }
        }),
      ).timeout(timeout);
      return ExitPreparationResult(
        // This means every available safe-preparation callback succeeded.
        // Tasks without one remain explicitly listed as non-checkpointable,
        // but do not trap the user in the exit prompt forever.
        completed: failures.isEmpty,
        failedLabels: List.unmodifiable(failures),
        unpreparedLabels: List.unmodifiable(unprepared),
        timedOut: false,
      );
    } on TimeoutException {
      return ExitPreparationResult(
        completed: false,
        failedLabels: List.unmodifiable(failures),
        unpreparedLabels: List.unmodifiable(unprepared),
        timedOut: true,
      );
    }
  }

  Future<T> run<T>({
    required String kind,
    required String label,
    required String phase,
    bool resumable = false,
    PrepareLongTaskForExit? prepareForExit,
    required Future<T> Function(LongTaskLease lease) action,
  }) async {
    final lease = register(
      kind: kind,
      label: label,
      phase: phase,
      resumable: resumable,
      prepareForExit: prepareForExit,
    );
    try {
      return await action(lease);
    } finally {
      lease.dispose();
    }
  }

  void _remove(int id) {
    if (_entries.remove(id) != null) notifyListeners();
  }
}

class LongTaskLease {
  LongTaskLease._(this._registry, this.id);

  final LongTaskRegistry _registry;
  final int id;
  bool _disposed = false;

  void update({String? phase, bool? resumable}) {
    if (_disposed) return;
    _registry.update(this, phase: phase, resumable: resumable);
  }

  void dispose() {
    if (_disposed) return;
    _disposed = true;
    _registry._remove(id);
  }
}

class _LongTaskEntry {
  _LongTaskEntry({
    required this.id,
    required this.kind,
    required this.label,
    required this.phase,
    required this.startedAt,
    required this.resumable,
    required this.prepareForExit,
  });

  final int id;
  final String kind;
  final String label;
  String phase;
  final DateTime startedAt;
  bool resumable;
  final PrepareLongTaskForExit? prepareForExit;
}
