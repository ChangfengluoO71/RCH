import 'dart:async';
import 'dart:io';

import 'package:app/src/rust/source/cloud115/offline.dart' as cloud115;
import 'package:app/store/cloud115_bt_batch.dart';
import 'package:app/store/cloud115_bt_monitor.dart';
import 'package:app/store/models.dart';
import 'package:flutter_test/flutter_test.dart';

const _hash = '0123456789012345678901234567890123456789';

class _FakeBackend implements Cloud115BtMonitorBackend {
  final List<Cloud115MonitorLedgerRow> rows = [];
  final List<Cloud115MonitorObservation> observations = [];
  DateTime? dueAt;
  int claimCount = 0;
  int attempt = 0;

  @override
  Future<void> reconcile(List<Cloud115MonitorLedgerRow> entries) async {
    for (final entry in entries) {
      if (!rows.any(
        (row) =>
            row.accountId == entry.accountId &&
            row.infoHash == entry.infoHash &&
            row.targetFolderId == entry.targetFolderId,
      )) {
        rows.add(entry);
      }
    }
    dueAt ??= DateTime.utc(2026, 10, 14);
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
    if (dueAt == null || dueAt!.isAfter(now)) return null;
    claimCount++;
    return Cloud115MonitorClaim(
      runId: 'run-$claimCount',
      tasks: rows
          .where((row) => row.accountId == accountId)
          .take(limit)
          .map(
            (row) => Cloud115MonitorTask(
              accountId: row.accountId,
              sourceId: row.sourceId,
              targetFolderId: row.targetFolderId,
              infoHash: row.infoHash,
              relativeFile: row.relativeFile,
              ledgerState: row.ledgerState,
              batchId: row.batchId,
              attemptCount: attempt,
            ),
          )
          .toList(),
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
  }) async {
    this.observations
      ..clear()
      ..addAll(observations);
    dueAt = nextCheckAt;
    attempt++;
  }

  @override
  Future<Cloud115MonitorSummary> summary({String? sourceId}) async {
    final found = observations
        .where((item) => item.foundInSnapshot == true)
        .length;
    return Cloud115MonitorSummary(
      trackedCount: rows.length,
      foundCount: found,
      unknownCount: rows.length - found,
      dueCount: dueAt != null && !dueAt!.isAfter(DateTime.utc(2026, 10, 14))
          ? rows.length
          : 0,
      lastCheckedAt: observations.isEmpty ? null : DateTime.utc(2026, 10, 14),
      nextCheckAt: dueAt,
    );
  }
}

void main() {
  late Directory temp;
  late Cloud115BtBatchLedger ledger;
  late BookSource source;
  late _FakeBackend backend;

  setUp(() async {
    temp = await Directory.systemTemp.createTemp('rch-cloud115-monitor-');
    ledger = Cloud115BtBatchLedger(File('${temp.path}/batch.jsonl'));
    source = BookSource(
      id: 'source-1',
      type: '115',
      name: 'test source',
      cookie: 'UID=12345_token; other=value',
    );
    backend = _FakeBackend();
    await ledger.withExclusiveLock((transaction) async {
      await transaction.setSelectionPreferences(
        sourceId: source.id,
        target: const cloud115.Cloud115BtFolderSelection(
          id: '42',
          parentId: '1',
          name: 'EH订阅',
          path: '/漫画/EH订阅',
        ),
      );
      await transaction.beginBatchContext(
        sourceId: source.id,
        batchId: 'batch-1',
      );
      await transaction.recordItem(
        accountId: '12345',
        infoHash: _hash,
        targetFolderId: '42',
        relativeFile: 'one.torrent',
        state: 'submitted',
      );
    });
  });

  tearDown(() async {
    await temp.delete(recursive: true);
  });

  test(
    'reconciles a ledger-only record after restart and polls one snapshot',
    () async {
      var requests = 0;
      final monitor = Cloud115BtMonitor(
        backend: backend,
        ledgerFactory: () async => ledger,
        sources: () => [source],
        sessionReady: (_) => true,
        readSnapshot: (_) async {
          requests++;
          return const cloud115.Cloud115BtTaskSnapshot(
            accountId: '12345',
            state: 'ok',
            complete: true,
            pagesRead: 1,
            message: '',
            tasks: [cloud115.Cloud115BtTaskSnapshotItem(infoHash: _hash)],
          );
        },
        clock: () => DateTime.utc(2026, 10, 14),
        owner: 'test-owner',
      );

      await monitor.start();

      expect(backend.rows, hasLength(1));
      expect(backend.rows.single.batchId, 'batch-1');
      expect(requests, 1);
      expect(monitor.summaryValue.foundCount, 1);
      expect(backend.observations.single.foundInSnapshot, isTrue);
      await monitor.stopForExit();
      monitor.dispose();
    },
  );

  test(
    'does not query under a different account or without cached session',
    () async {
      var requests = 0;
      final monitor = Cloud115BtMonitor(
        backend: backend,
        ledgerFactory: () async => ledger,
        sources: () => [source],
        sessionReady: (_) => false,
        readSnapshot: (_) async {
          requests++;
          throw StateError('should not run');
        },
        clock: () => DateTime.utc(2026, 10, 14),
        owner: 'test-owner',
      );
      await monitor.start();
      expect(requests, 0);
      expect(backend.rows, hasLength(1));
      await monitor.stopForExit();
      monitor.dispose();

      source.cookie = 'UID=99999_other';
      var mismatchRequests = 0;
      final mismatched = Cloud115BtMonitor(
        backend: _FakeBackend(),
        ledgerFactory: () async => ledger,
        sources: () => [source],
        sessionReady: (_) => true,
        readSnapshot: (_) async {
          mismatchRequests++;
          throw StateError('must not query');
        },
        clock: () => DateTime.utc(2026, 10, 14),
        owner: 'test-owner-2',
      );
      await mismatched.start();
      // Keep the historical projection, but never query under the now
      // mismatched account identity.
      expect(mismatched.summaryValue.trackedCount, 1);
      expect(mismatchRequests, 0);
      await mismatched.stopForExit();
      mismatched.dispose();
    },
  );

  test(
    'a missing cached session does not schedule a zero-delay poll loop',
    () async {
      final delays = <Duration>[];
      final monitor = Cloud115BtMonitor(
        backend: backend,
        ledgerFactory: () async => ledger,
        sources: () => [source],
        sessionReady: (_) => false,
        readSnapshot: (_) async => throw StateError('must not be called'),
        clock: () => DateTime.utc(2026, 10, 14),
        timerFactory: (duration, callback) {
          delays.add(duration);
          return Timer(const Duration(days: 1), callback);
        },
        owner: 'no-session-test',
      );

      await monitor.start();

      expect(delays.last, const Duration(minutes: 1));
      await monitor.stopForExit();
      monitor.dispose();
    },
  );

  test(
    'an unexpected snapshot account contributes no remote task fields',
    () async {
      final monitor = Cloud115BtMonitor(
        backend: backend,
        ledgerFactory: () async => ledger,
        sources: () => [source],
        sessionReady: (_) => true,
        readSnapshot: (_) async => const cloud115.Cloud115BtTaskSnapshot(
          accountId: 'unexpected-account',
          state: 'ok',
          complete: true,
          pagesRead: 1,
          message: '',
          tasks: [
            cloud115.Cloud115BtTaskSnapshotItem(
              infoHash: _hash,
              statusCode: 99,
              percentDone: 1,
            ),
          ],
        ),
        clock: () => DateTime.utc(2026, 10, 14),
        owner: 'account-mismatch-test',
      );

      await monitor.start();

      expect(backend.observations.single.foundInSnapshot, isNull);
      expect(backend.observations.single.statusCode, isNull);
      expect(backend.observations.single.percentDone, isNull);
      expect(backend.observations.single.errorClass, 'account_mismatch');
      await monitor.stopForExit();
      monitor.dispose();
    },
  );

  test(
    'partial snapshot cannot mark absent tasks as missing or completed',
    () async {
      final monitor = Cloud115BtMonitor(
        backend: backend,
        ledgerFactory: () async => ledger,
        sources: () => [source],
        sessionReady: (_) => true,
        readSnapshot: (_) async => const cloud115.Cloud115BtTaskSnapshot(
          accountId: '12345',
          state: 'partial',
          complete: false,
          pagesRead: 10,
          errorClass: 'query_budget_exhausted',
          message: 'bounded partial list',
          tasks: [],
        ),
        clock: () => DateTime.utc(2026, 10, 14),
        owner: 'test-owner',
      );
      await monitor.start();
      expect(backend.observations.single.foundInSnapshot, isNull);
      expect(backend.observations.single.errorClass, 'query_budget_exhausted');
      expect(monitor.summaryValue.foundCount, 0);
      await monitor.stopForExit();
      monitor.dispose();
    },
  );

  test('retry schedule is bounded at thirty minutes', () {
    expect(Cloud115BtMonitor.retryDelay(0), const Duration(minutes: 1));
    expect(Cloud115BtMonitor.retryDelay(1), const Duration(minutes: 5));
    expect(Cloud115BtMonitor.retryDelay(2), const Duration(minutes: 15));
    expect(Cloud115BtMonitor.retryDelay(3), const Duration(minutes: 30));
    expect(Cloud115BtMonitor.retryDelay(99), const Duration(minutes: 30));
  });
}
