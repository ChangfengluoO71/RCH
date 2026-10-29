import 'dart:io';

import 'package:app/src/rust/source/cloud115/offline.dart' as cloud115;
import 'package:app/store/cloud115_bt_batch.dart';
import 'package:app/store/cloud115_bt_daily_workflow.dart';
import 'package:app/store/eh_daily_coordinator.dart';
import 'package:app/store/eh_subscription_store.dart';
import 'package:app/store/models.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  late Directory tempDir;

  setUp(() async {
    tempDir = await Directory.systemTemp.createTemp('rch-daily-cloud-');
  });

  tearDown(() async {
    if (await tempDir.exists()) await tempDir.delete(recursive: true);
  });

  test(
    'local-only delivery stops after EH saving without reading 115 session',
    () async {
      var batchCalls = 0;
      final ledger = Cloud115BtBatchLedger(
        File('${tempDir.path}/ledger.jsonl'),
      );
      await ledger.setSelectionPreferences(
        sourceId: null,
        target: null,
        mode: Cloud115BtDeliveryMode.localTorrentOnly,
      );
      final workflow = Cloud115BtDailyWorkflow(
        ledger: ledger,
        sources: () => throw StateError('source lookup must not run'),
        cookieSessionIsCached: (_) =>
            throw StateError('session lookup must not run'),
        batchRunner: (_, _, _, _) async {
          batchCalls++;
          return const Cloud115BtBatchReport(results: [], stopped: false);
        },
      );

      final result = await workflow.run(
        entries: [_entry()],
        outputDir: tempDir.path,
      );

      expect(result.status, EhDailyCloudStageStatus.notRun);
      expect(result.total, 1);
      expect(result.message, contains('非 115 云下载'));
      expect(batchCalls, 0);
      expect(
        (await ledger.selectionPreferences()).mode,
        Cloud115BtDeliveryMode.localTorrentOnly,
      );
    },
  );

  test(
    'missing saved source or target is reported before session access',
    () async {
      var sourceReads = 0;
      var sessionReads = 0;
      final workflow = Cloud115BtDailyWorkflow(
        ledger: Cloud115BtBatchLedger(File('${tempDir.path}/ledger.jsonl')),
        sources: () {
          sourceReads++;
          return const [];
        },
        cookieSessionIsCached: (_) {
          sessionReads++;
          return false;
        },
        batchRunner: (_, _, _, _) async =>
            const Cloud115BtBatchReport(results: [], stopped: false),
      );

      final result = await workflow.run(
        entries: [_entry()],
        outputDir: tempDir.path,
      );

      expect(result.status, EhDailyCloudStageStatus.needsConfiguration);
      expect(sourceReads, 0);
      expect(sessionReads, 0);
    },
  );

  test(
    'background restores an existing cookie session before submitting',
    () async {
      final ledger = Cloud115BtBatchLedger(
        File('${tempDir.path}/ledger.jsonl'),
      );
      await ledger.setSelectionPreferences(
        sourceId: 'cookie-115',
        target: const cloud115.Cloud115BtFolderSelection(
          id: '123',
          parentId: '10',
          name: 'target',
          path: '/target',
        ),
      );
      final source = BookSource(
        id: 'cookie-115',
        type: '115',
        name: 'source',
        cookie: 'saved',
      );
      var restored = false;
      var batchCalls = 0;
      final workflow = Cloud115BtDailyWorkflow(
        ledger: ledger,
        sources: () => [source],
        cookieSessionIsCached: (_) => restored,
        restoreCookieSession: (_) async {
          restored = true;
        },
        batchRunner: (_, entries, _, _) async {
          batchCalls++;
          return Cloud115BtBatchReport(
            results: [
              Cloud115BtBatchItemResult(
                relativeFile: entries.single.item.file,
                infoHash: entries.single.item.infohash,
                state: 'submitted',
              ),
            ],
            stopped: false,
          );
        },
      );

      final result = await workflow.run(
        entries: [_entry()],
        outputDir: tempDir.path,
      );

      expect(result.status, EhDailyCloudStageStatus.completed);
      expect(restored, isTrue);
      expect(batchCalls, 1);
    },
  );

  test('a remote-only source cannot be used for a background BT task', () async {
    final ledger = Cloud115BtBatchLedger(File('${tempDir.path}/ledger.jsonl'));
    await ledger.setSelectionPreferences(
      sourceId: 'remote-source',
      target: const cloud115.Cloud115BtFolderSelection(
        id: '123', parentId: '10', name: 'target', path: '/target',
      ),
    );
    var restored = false;
    var submitted = false;
    final workflow = Cloud115BtDailyWorkflow(
      ledger: ledger,
      sources: () => [BookSource(
        id: 'remote-source', type: '115', name: 'remote', cookie: 'saved', remoteOnly: true,
      )],
      cookieSessionIsCached: (_) => false,
      restoreCookieSession: (_) async { restored = true; },
      batchRunner: (_, _, _, _) async {
        submitted = true;
        return const Cloud115BtBatchReport(results: [], stopped: false);
      },
    );
    final result = await workflow.run(entries: [_entry()], outputDir: tempDir.path);
    expect(result.status, EhDailyCloudStageStatus.needsConfiguration);
    expect(restored, isFalse);
    expect(submitted, isFalse);
  });

  test(
    'accepted BT requests are not reported as cloud download completion',
    () async {
      final ledger = Cloud115BtBatchLedger(
        File('${tempDir.path}/ledger.jsonl'),
      );
      await ledger.setSelectionPreferences(
        sourceId: 'cookie-115',
        target: const cloud115.Cloud115BtFolderSelection(
          id: '123',
          parentId: '10',
          name: 'EH订阅',
          path: '/漫画/EH订阅',
        ),
      );
      final workflow = Cloud115BtDailyWorkflow(
        ledger: ledger,
        sources: () => [
          BookSource(
            id: 'cookie-115',
            type: '115',
            name: '测试 Cookie 源',
            cookie: 'mock-cookie',
          ),
        ],
        cookieSessionIsCached: (_) => true,
        batchRunner: (_, entries, _, _) async => Cloud115BtBatchReport(
          results: [
            Cloud115BtBatchItemResult(
              relativeFile: entries.single.item.file,
              infoHash: entries.single.item.infohash,
              state: 'submitted',
            ),
          ],
          stopped: false,
        ),
      );

      final result = await workflow.run(
        entries: [_entry()],
        outputDir: tempDir.path,
      );

      expect(result.status, EhDailyCloudStageStatus.completed);
      expect(result.submitted, 1);
      expect(result.message, contains('云端下载完成与 RCH 书源识别尚未确认'));
    },
  );

  test('rejected and invalid entries do not yield a completed cloud stage', () {
    for (final state in [
      'rejected',
      'local_validation_failed',
      'torrent_invalid',
    ]) {
      final result = Cloud115BtDailyWorkflow.summarizeBatchReport(
        Cloud115BtBatchReport(
          results: [
            Cloud115BtBatchItemResult(
              relativeFile: 'sample.torrent',
              infoHash: '0123456789012345678901234567890123456789',
              state: state,
            ),
          ],
          stopped: false,
        ),
        1,
      );
      expect(result.status, EhDailyCloudStageStatus.partial, reason: state);
      expect(result.skipped, 1);
    }
  });

  test('accepted task awaiting cleanup is partial and is counted as pending', () {
    final result = Cloud115BtDailyWorkflow.summarizeBatchReport(
      const Cloud115BtBatchReport(results: [Cloud115BtBatchItemResult(
        relativeFile: 'sample.torrent', infoHash: '0123456789012345678901234567890123456789',
        state: 'submitted_cleanup_pending',
      )], stopped: false), 1,
    );
    expect(result.status, EhDailyCloudStageStatus.partial);
    expect(result.pending, 1);
  });

  test('confirmed existing destination is included in handled task count', () {
    final result = Cloud115BtDailyWorkflow.summarizeBatchReport(
      const Cloud115BtBatchReport(results: [Cloud115BtBatchItemResult(
        relativeFile: 'sample.torrent', infoHash: '0123456789012345678901234567890123456789',
        state: 'duplicate_found',
      )], stopped: false), 1,
    );
    expect(result.status, EhDailyCloudStageStatus.completed);
    expect(result.submitted, 1);
  });

  test('remote duplicate with failed local cleanup remains pending', () {
    final result = Cloud115BtDailyWorkflow.summarizeBatchReport(
      const Cloud115BtBatchReport(
        results: [
          Cloud115BtBatchItemResult(
            relativeFile: 'sample.torrent',
            infoHash: '0123456789012345678901234567890123456789',
            state: 'duplicate_found_delete_failed',
            message: 'remote task was confirmed; local delete failed',
          ),
        ],
        stopped: false,
      ),
      1,
    );

    expect(result.status, EhDailyCloudStageStatus.partial);
    expect(result.pending, 1);
  });
}

EhCheckedManifestEntry _entry() => EhCheckedManifestEntry(
  item: const EhSavedItem(
    infohash: '0123456789012345678901234567890123456789',
    titleJpn: 'sample',
    rating: 5,
    downloads: 1,
    requiredDl: 0,
    postedUtc: '2026-10-13',
    ageYears: null,
    category: 'Manga',
    filecount: 1,
    filesize: 1,
    tags: [],
    file: 'sample.torrent',
    bytes: 1,
  ),
  fileStatus: EhSavedFileStatus.present,
  validationStatus: EhTorrentValidationStatus.valid,
  fullPath: 'C:/EH output/sample.torrent',
);
