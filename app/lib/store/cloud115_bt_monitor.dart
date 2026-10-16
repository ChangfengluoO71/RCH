import 'dart:async';
import 'dart:math';

import 'package:app/src/rust/api/db.dart' as rust_db;
import 'package:app/src/rust/source/cloud115/offline.dart' as cloud115;
import 'package:app/store/cloud115_bt_batch.dart';
import 'package:app/store/cloud115_session.dart';
import 'package:app/store/library_store.dart';
import 'package:app/store/models.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter_rust_bridge/flutter_rust_bridge_for_generated.dart'
    show PlatformInt64Util;

class Cloud115MonitorLedgerRow {
  const Cloud115MonitorLedgerRow({
    required this.batchId,
    required this.accountId,
    required this.sourceId,
    required this.targetFolderId,
    required this.infoHash,
    required this.relativeFile,
    required this.ledgerState,
    required this.updatedAt,
  });

  final String batchId;
  final String accountId;
  final String sourceId;
  final String targetFolderId;
  final String infoHash;
  final String relativeFile;
  final String ledgerState;
  final int updatedAt;
}

class Cloud115MonitorTask {
  const Cloud115MonitorTask({
    required this.accountId,
    required this.sourceId,
    required this.targetFolderId,
    required this.infoHash,
    required this.relativeFile,
    required this.ledgerState,
    required this.batchId,
    required this.attemptCount,
  });

  final String accountId;
  final String sourceId;
  final String targetFolderId;
  final String infoHash;
  final String relativeFile;
  final String ledgerState;
  final String batchId;
  final int attemptCount;
}

class Cloud115MonitorClaim {
  const Cloud115MonitorClaim({required this.runId, required this.tasks});

  final String runId;
  final List<Cloud115MonitorTask> tasks;
}

class Cloud115MonitorObservation {
  const Cloud115MonitorObservation({
    required this.accountId,
    required this.infoHash,
    required this.targetFolderId,
    this.foundInSnapshot,
    this.statusCode,
    this.percentDone,
    this.errorClass,
  });

  final String accountId;
  final String infoHash;
  final String targetFolderId;
  final bool? foundInSnapshot;
  final int? statusCode;
  final double? percentDone;
  final String? errorClass;
}

class Cloud115MonitorSummary {
  const Cloud115MonitorSummary({
    this.trackedCount = 0,
    this.foundCount = 0,
    this.unknownCount = 0,
    this.dueCount = 0,
    this.lastCheckedAt,
    this.nextCheckAt,
    this.lastErrorClass,
  });

  final int trackedCount;
  final int foundCount;
  final int unknownCount;
  final int dueCount;
  final DateTime? lastCheckedAt;
  final DateTime? nextCheckAt;
  final String? lastErrorClass;

  bool get hasResumableWork => trackedCount > 0;
}

abstract interface class Cloud115BtMonitorBackend {
  Future<void> reconcile(List<Cloud115MonitorLedgerRow> entries);

  Future<Cloud115MonitorClaim?> claimDue({
    required String accountId,
    required String sourceId,
    required String owner,
    required DateTime now,
    required DateTime leaseExpiresAt,
    required int limit,
  });

  Future<void> finishPoll({
    required String runId,
    required String accountId,
    required String owner,
    required List<Cloud115MonitorObservation> observations,
    required String? errorClass,
    required DateTime nextCheckAt,
  });

  Future<Cloud115MonitorSummary> summary({String? sourceId});
}

class _RustCloud115BtMonitorBackend implements Cloud115BtMonitorBackend {
  @override
  Future<void> reconcile(List<Cloud115MonitorLedgerRow> entries) async {
    await rust_db.dbCloud115BtMonitorReconcile(
      entries: entries
          .map(
            (entry) => rust_db.Cloud115BtMonitorLedgerEntryDto(
              batchId: entry.batchId,
              accountId: entry.accountId,
              sourceId: entry.sourceId,
              targetFolderId: entry.targetFolderId,
              infoHash: entry.infoHash,
              relativeFile: entry.relativeFile,
              ledgerState: entry.ledgerState,
              updatedAt: PlatformInt64Util.from(entry.updatedAt),
            ),
          )
          .toList(growable: false),
    );
  }

  @override
  Future<Cloud115MonitorClaim?> claimDue({
    required String accountId,
    required String sourceId,
    required String owner,
    required DateTime now,
    required DateTime leaseExpiresAt,
    required int limit,
  }) async {
    final claim = await rust_db.dbCloud115BtMonitorClaimDue(
      accountId: accountId,
      sourceId: sourceId,
      owner: owner,
      leaseExpiresAt: PlatformInt64Util.from(
        leaseExpiresAt.millisecondsSinceEpoch,
      ),
      limit: limit,
    );
    if (claim == null) return null;
    return Cloud115MonitorClaim(
      runId: claim.runId,
      tasks: claim.tasks
          .map(
            (task) => Cloud115MonitorTask(
              accountId: task.accountId,
              sourceId: task.sourceId,
              targetFolderId: task.targetFolderId,
              infoHash: task.infoHash,
              relativeFile: task.relativeFile,
              ledgerState: task.ledgerState,
              batchId: task.batchId,
              attemptCount: int.tryParse(task.attemptCount.toString()) ?? 0,
            ),
          )
          .toList(growable: false),
    );
  }

  @override
  Future<void> finishPoll({
    required String runId,
    required String accountId,
    required String owner,
    required List<Cloud115MonitorObservation> observations,
    required String? errorClass,
    required DateTime nextCheckAt,
  }) => rust_db.dbCloud115BtMonitorFinishPoll(
    runId: runId,
    accountId: accountId,
    owner: owner,
    observations: observations
        .map(
          (observation) => rust_db.Cloud115BtMonitorObservationDto(
            accountId: observation.accountId,
            infoHash: observation.infoHash,
            targetFolderId: observation.targetFolderId,
            foundInSnapshot: observation.foundInSnapshot,
            statusCode: observation.statusCode == null
                ? null
                : PlatformInt64Util.from(observation.statusCode!),
            percentDone: observation.percentDone,
            errorClass: observation.errorClass,
          ),
        )
        .toList(growable: false),
    errorClass: errorClass,
    nextCheckAt: PlatformInt64Util.from(nextCheckAt.millisecondsSinceEpoch),
  );

  @override
  Future<Cloud115MonitorSummary> summary({String? sourceId}) async {
    final value = await rust_db.dbCloud115BtMonitorSummary(sourceId: sourceId);
    return Cloud115MonitorSummary(
      trackedCount: value.trackedCount,
      foundCount: value.foundCount,
      unknownCount: value.unknownCount,
      dueCount: value.dueCount,
      lastCheckedAt: _fromInt64(value.lastCheckedAt),
      nextCheckAt: _fromInt64(value.nextCheckAt),
      lastErrorClass: value.lastErrorClass,
    );
  }
}

DateTime? _fromInt64(Object? value) {
  if (value == null) return null;
  final milliseconds = int.tryParse(value.toString());
  if (milliseconds == null || milliseconds <= 0) return null;
  return DateTime.fromMillisecondsSinceEpoch(milliseconds);
}

typedef Cloud115TaskSnapshotReader =
    Future<cloud115.Cloud115BtTaskSnapshot> Function(BookSource source);
typedef Cloud115SessionReady = bool Function(BookSource source);
typedef Cloud115AccountHint = String? Function(BookSource source);

/// Owns bounded, read-only polling while the RCH process is alive. JSONL stays
/// authoritative for all writes; the Rust database is only a resumable status
/// projection.
class Cloud115BtMonitor extends ChangeNotifier {
  Cloud115BtMonitor({
    Cloud115BtMonitorBackend? backend,
    Future<Cloud115BtBatchLedger> Function()? ledgerFactory,
    Iterable<BookSource> Function()? sources,
    Cloud115SessionReady? sessionReady,
    Cloud115AccountHint? accountHint,
    Cloud115TaskSnapshotReader? readSnapshot,
    DateTime Function()? clock,
    Timer Function(Duration, void Function())? timerFactory,
    String? owner,
  }) : _backend = backend ?? _RustCloud115BtMonitorBackend(),
       _ledgerFactory =
           ledgerFactory ?? Cloud115BtBatchLedger.inApplicationSupportDirectory,
       _sources = sources ?? (() => LibraryStore.instance.sources),
       _sessionReady = sessionReady ?? hasCachedCloud115CookieSession,
       _accountHint = accountHint ?? cloud115CookieAccountIdHint,
       _readSnapshot = readSnapshot ?? cloud115CookieBtTaskSnapshotFor,
       _clock = clock ?? DateTime.now,
       _timerFactory =
           timerFactory ?? ((duration, callback) => Timer(duration, callback)),
       _owner = owner ?? _makeOwner();

  static final Cloud115BtMonitor instance = Cloud115BtMonitor();
  static const _monitorableStates = {
    'submit_started',
    'submitted',
    'local_torrent_removed',
    'duplicate_found',
    'duplicate_found_delete_failed',
    'unknown_remote_state',
    'duplicate_lookup_unknown',
  };
  static const _maxTasksPerCycle = 300;
  static const _leaseDuration = Duration(minutes: 3);
  static const _noSessionWake = Duration(minutes: 1);

  final Cloud115BtMonitorBackend _backend;
  final Future<Cloud115BtBatchLedger> Function() _ledgerFactory;
  final Iterable<BookSource> Function() _sources;
  final Cloud115SessionReady _sessionReady;
  final Cloud115AccountHint _accountHint;
  final Cloud115TaskSnapshotReader _readSnapshot;
  final DateTime Function() _clock;
  final Timer Function(Duration, void Function()) _timerFactory;
  final String _owner;

  Timer? _timer;
  Future<void>? _cycle;
  bool _started = false;
  bool _disposed = false;
  Cloud115MonitorSummary _summary = const Cloud115MonitorSummary();
  String? _lastLocalError;
  DateTime? _noSessionWakeAt;

  Cloud115MonitorSummary get summaryValue => _summary;
  String? get lastLocalError => _lastLocalError;
  bool get isPolling => _cycle != null;
  bool get isStarted => _started;

  Future<void> start() async {
    if (_started || _disposed) return;
    _started = true;
    try {
      await reconcileFromLedger();
      await pollDueOnce();
      await _refreshSummaryAndSchedule();
    } catch (_) {
      _lastLocalError = 'monitor_storage_unavailable';
      _timer?.cancel();
      _timer = null;
      notifyListeners();
    }
  }

  Future<void> reconcileFromLedger() async {
    if (_disposed) return;
    try {
      final snapshot = await (await _ledgerFactory()).snapshotForMonitoring();
      final sources = {for (final source in _sources()) source.id: source};
      final preferences = snapshot.selectionPreferences;
      final rows = <Cloud115MonitorLedgerRow>[];
      for (final record in snapshot.records) {
        if (!_monitorableStates.contains(record.state)) continue;
        final sourceId = record.sourceId ?? preferences.sourceId;
        if (sourceId == null || sourceId.isEmpty) continue;
        final source = sources[sourceId];
        // Legacy ledger rows did not carry a source ID. Use the saved source
        // only when its local UID hint matches the account already in ledger.
        if (record.sourceId == null &&
            _accountHint(source ?? _emptySource) != record.accountId) {
          continue;
        }
        rows.add(
          Cloud115MonitorLedgerRow(
            batchId:
                record.batchId ??
                'legacy:${record.accountId}:${record.targetFolderId}:${record.updatedAtUtc}',
            accountId: record.accountId,
            sourceId: sourceId,
            targetFolderId: record.targetFolderId,
            infoHash: record.infoHash,
            relativeFile: record.relativeFile,
            ledgerState: record.state,
            updatedAt:
                DateTime.tryParse(
                  record.updatedAtUtc,
                )?.millisecondsSinceEpoch ??
                0,
          ),
        );
      }
      await _backend.reconcile(rows);
      _lastLocalError = null;
      await _refreshSummaryAndSchedule();
      notifyListeners();
    } catch (_) {
      // The JSONL ledger is authoritative. If it is unreadable, do not guess
      // monitor entries from the database or continue network polling.
      _lastLocalError = 'ledger_unavailable';
      _timer?.cancel();
      _timer = null;
      notifyListeners();
    }
  }

  Future<void> pollDueOnce() {
    if (_disposed || _cycle != null) return _cycle ?? Future<void>.value();
    final cycle = _guardedPollDueOnce();
    _cycle = cycle;
    return cycle.whenComplete(() {
      if (identical(_cycle, cycle)) _cycle = null;
    });
  }

  Future<void> _guardedPollDueOnce() async {
    try {
      await _pollDueOnce();
    } catch (_) {
      _lastLocalError = 'monitor_storage_unavailable';
      notifyListeners();
    }
  }

  Future<void> _pollDueOnce() async {
    if (!_started || _disposed || _lastLocalError != null) return;
    _noSessionWakeAt = null;
    final ledgerSnapshot = await (await _ledgerFactory())
        .snapshotForMonitoring();
    final sourcesById = {for (final source in _sources()) source.id: source};
    final candidatesByAccount = <String, List<BookSource>>{};
    for (final record in ledgerSnapshot.records) {
      if (!_monitorableStates.contains(record.state)) continue;
      final sourceId =
          record.sourceId ?? ledgerSnapshot.selectionPreferences.sourceId;
      if (sourceId == null) continue;
      final source = sourcesById[sourceId];
      if (source == null || _accountHint(source) != record.accountId) continue;
      final candidates = candidatesByAccount.putIfAbsent(
        record.accountId,
        () => <BookSource>[],
      );
      if (!candidates.any((candidate) => candidate.id == source.id)) {
        candidates.add(source);
      }
    }

    for (final entry in candidatesByAccount.entries) {
      if (_disposed) return;
      BookSource? source;
      for (final candidate in entry.value) {
        if (_sessionReady(candidate)) {
          source = candidate;
          break;
        }
      }
      if (source == null) {
        _noSessionWakeAt = _clock().add(_noSessionWake);
        continue;
      }
      await _pollAccount(entry.key, source);
    }
    await _refreshSummaryAndSchedule();
  }

  Future<void> _pollAccount(String accountId, BookSource source) async {
    final now = _clock();
    final claim = await _backend.claimDue(
      accountId: accountId,
      sourceId: '*',
      owner: _owner,
      now: now,
      leaseExpiresAt: now.add(_leaseDuration),
      limit: _maxTasksPerCycle,
    );
    if (claim == null || claim.tasks.isEmpty) return;

    String? errorClass;
    final observations = <Cloud115MonitorObservation>[];
    var attempt = 0;
    try {
      final snapshot = await _readSnapshot(source);
      if (snapshot.accountId != accountId) {
        errorClass = 'account_mismatch';
      } else if (snapshot.errorClass != null) {
        errorClass = snapshot.errorClass;
      } else if (snapshot.state != 'ok' || !snapshot.complete) {
        errorClass = 'partial_snapshot';
      }
      final accountMatches = snapshot.accountId == accountId;
      final byHash = accountMatches
          ? <String, cloud115.Cloud115BtTaskSnapshotItem>{
              for (final task in snapshot.tasks)
                task.infoHash.toLowerCase(): task,
            }
          : <String, cloud115.Cloud115BtTaskSnapshotItem>{};
      attempt = claim.tasks.fold<int>(
        0,
        (maximum, task) => max(maximum, task.attemptCount),
      );
      for (final task in claim.tasks) {
        final remote = byHash[task.infoHash.toLowerCase()];
        observations.add(
          Cloud115MonitorObservation(
            accountId: accountId,
            infoHash: task.infoHash,
            targetFolderId: task.targetFolderId,
            foundInSnapshot: !accountMatches
                ? null
                : remote != null
                ? true
                : snapshot.complete && snapshot.state == 'ok'
                ? false
                : null,
            statusCode: !accountMatches || remote?.statusCode == null
                ? null
                : int.tryParse(remote!.statusCode.toString()),
            percentDone: accountMatches ? remote?.percentDone : null,
            errorClass: errorClass,
          ),
        );
      }
    } catch (_) {
      errorClass = 'network_or_session_error';
      attempt = claim.tasks.fold<int>(
        0,
        (maximum, task) => max(maximum, task.attemptCount),
      );
      for (final task in claim.tasks) {
        observations.add(
          Cloud115MonitorObservation(
            accountId: accountId,
            infoHash: task.infoHash,
            targetFolderId: task.targetFolderId,
            errorClass: errorClass,
          ),
        );
      }
    }

    final nextCheck = _clock().add(retryDelay(attempt));
    await _backend.finishPoll(
      runId: claim.runId,
      accountId: accountId,
      owner: _owner,
      observations: observations,
      errorClass: errorClass,
      nextCheckAt: nextCheck,
    );
    notifyListeners();
  }

  static Duration retryDelay(int previousAttemptCount) {
    const minutes = [1, 5, 15, 30];
    final index = previousAttemptCount.clamp(0, minutes.length - 1);
    return Duration(minutes: minutes[index]);
  }

  Future<void> refreshSummary() async {
    if (_disposed) return;
    _summary = await _backend.summary();
    notifyListeners();
  }

  Future<void> _refreshSummaryAndSchedule() async {
    if (_disposed) return;
    _summary = await _backend.summary();
    _timer?.cancel();
    _timer = null;
    if (!_started) return;
    if (_summary.trackedCount == 0) return;
    final now = _clock();
    final due = _summary.nextCheckAt;
    final Duration wait;
    if (due == null) {
      wait = _noSessionWake;
    } else if (due.isAfter(now)) {
      wait = due.difference(now);
    } else {
      final deferredWake = _noSessionWakeAt;
      wait = deferredWake == null || !deferredWake.isAfter(now)
          ? Duration.zero
          : deferredWake.difference(now);
    }
    _timer = _timerFactory(wait, () async {
      await pollDueOnce();
      await _refreshSummaryAndSchedule();
    });
  }

  /// Stops local polling before exit. Durable monitor and batch records remain
  /// untouched; remote 115 downloads continue independently.
  Future<void> stopForExit() async {
    _started = false;
    _timer?.cancel();
    _timer = null;
    final cycle = _cycle;
    if (cycle != null) {
      try {
        await cycle.timeout(const Duration(seconds: 15));
      } on TimeoutException {
        // A bounded read may outlive shutdown; its SQLite lease expires and
        // will recover on the next application launch.
      } catch (_) {}
    }
  }

  Future<void> wakeAfterResume() async {
    if (!_started || _disposed) return;
    await reconcileFromLedger();
    await pollDueOnce();
  }

  static String _makeOwner() {
    final suffix = Random.secure().nextInt(1 << 32).toRadixString(16);
    return 'rch-${DateTime.now().microsecondsSinceEpoch}-$suffix';
  }

  static final BookSource _emptySource = BookSource(
    id: '',
    type: 'local',
    name: '',
    path: '',
  );

  @override
  void dispose() {
    _disposed = true;
    _timer?.cancel();
    super.dispose();
  }
}
