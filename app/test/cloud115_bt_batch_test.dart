import 'dart:async';
import 'dart:io';

import 'package:app/src/rust/source/cloud115/offline.dart';
import 'package:app/store/cloud115_bt_batch.dart';
import 'package:app/store/eh_subscription_store.dart';
import 'package:app/store/long_task_registry.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  late Directory tempDir;

  setUp(() async {
    tempDir = await Directory.systemTemp.createTemp('rch-115-batch-test-');
  });

  tearDown(() async {
    if (await tempDir.exists()) await tempDir.delete(recursive: true);
  });

  test(
    'batch registers one ledger-backed lease and releases it on completion',
    () async {
      final registry = LongTaskRegistry();
      final enteredPreflight = Completer<void>();
      final releasePreflight = Completer<void>();
      final processor = Cloud115BtBatchProcessor(
        ledger: Cloud115BtBatchLedger(File('${tempDir.path}/tracked.jsonl')),
        taskRegistry: registry,
      );

      final run = processor.run(
        entries: [_entry()],
        target: _target,
        preflight: (_) async {
          if (!enteredPreflight.isCompleted) enteredPreflight.complete();
          await releasePreflight.future;
          return _preflight(duplicateStatus: 'submit_first');
        },
        upload: (_) async => _upload(),
        recoverUpload: (_, _) async => _upload(),
        submit: (_, _) async => _submitted(),
        queryTask: (_) async => _task('not_found'),
        deleteTorrent: (_, _, _) async {},
      );

      await enteredPreflight.future;
      expect(registry.snapshot(), hasLength(1));
      expect(registry.snapshot().single.kind, 'cloud115_bt_batch');
      expect(registry.snapshot().single.phase, '只读核对账号与远端任务');
      releasePreflight.complete();
      final report = await run;

      expect(report.results.single.state, 'submitted');
      expect(registry.snapshot(), isEmpty);
    },
  );

  test(
    'new torrent submits local preflight file indices without upload',
    () async {
      var uploads = 0;
      var recoveries = 0;
      var submits = 0;
      final report =
          await Cloud115BtBatchProcessor(
            ledger: Cloud115BtBatchLedger(File('${tempDir.path}/direct.jsonl')),
          ).run(
            entries: [_entry()],
            target: _target,
            preflight: (_) async => _preflight(),
            upload: (_) async {
              uploads++;
              return _upload();
            },
            recoverUpload: (_, _) async {
              recoveries++;
              return _upload();
            },
            submit: (_, wanted) async {
              submits++;
              expect(wanted, [0, 1]);
              return _submitted();
            },
            queryTask: (_) async => _task('not_found'),
            deleteTorrent: (_, _, _) async {},
          );
      expect(report.results.single.state, 'submitted');
      expect(uploads, 0);
      expect(recoveries, 0);
      expect(submits, 1);
    },
  );

  test(
    'rejected submission with a task in another folder cannot authorize cleanup',
    () async {
      var deletes = 0;
      final report =
          await Cloud115BtBatchProcessor(
            ledger: Cloud115BtBatchLedger(
              File('${tempDir.path}/wrong-folder.jsonl'),
            ),
          ).run(
            entries: [_entry()],
            target: _target,
            cleanupAfterSubmit: true,
            preflight: (_) async => _preflight(),
            upload: (_) async => _upload(),
            recoverUpload: (_, _) async => _upload(),
            submit: (_, _) async => const Cloud115BtSubmitResult(
              state: 'REJECTED',
              infoHash: _hash,
              message: 'rejected',
              targetFolderId: '30',
            ),
            queryTask: (_) async => const Cloud115BtTaskStatus(
              state: 'found',
              infoHash: _hash,
              statusText: 'found',
              targetFolderId: '99',
            ),
            deleteTorrent: (_, _, _) async {
              deletes++;
            },
          );
      expect(report.results.single.state, 'duplicate_lookup_unknown');
      expect(deletes, 0);
    },
  );

  test(
    'rejection code survives an inconclusive duplicate check without provider secrets',
    () async {
      final report =
          await Cloud115BtBatchProcessor(
            ledger: Cloud115BtBatchLedger(
              File('${tempDir.path}/reject-code.jsonl'),
            ),
          ).run(
            entries: [_entry()],
            target: _target,
            preflight: (_) async => _preflight(),
            upload: (_) async => _upload(),
            recoverUpload: (_, _) async => _upload(),
            submit: (_, _) async => const Cloud115BtSubmitResult(
              state: 'REJECTED',
              infoHash: _hash,
              message: 'error code 1234 Cookie=secret',
              targetFolderId: '30',
            ),
            queryTask: (_) async => const Cloud115BtTaskStatus(
              state: 'unknown',
              infoHash: _hash,
              statusText: '',
              targetFolderId: '',
            ),
            deleteTorrent: (_, _, _) async {},
          );
      expect(report.results.single.state, 'duplicate_lookup_unknown');
      expect(report.results.single.message, contains('1234'));
      expect(report.results.single.message, isNot(contains('secret')));
    },
  );

  test(
    'already-present response without destination proof stays unknown',
    () async {
      var deletes = 0;
      final report =
          await Cloud115BtBatchProcessor(
            ledger: Cloud115BtBatchLedger(
              File('${tempDir.path}/unverified-duplicate.jsonl'),
            ),
          ).run(
            entries: [_entry()],
            target: _target,
            cleanupAfterSubmit: true,
            preflight: (_) async => _preflight(),
            upload: (_) async => _upload(),
            recoverUpload: (_, _) async => _upload(),
            submit: (_, _) async => const Cloud115BtSubmitResult(
              state: 'ALREADY_PRESENT',
              infoHash: _hash,
              message: 'already exists',
              targetFolderId: '30',
            ),
            queryTask: (_) async => const Cloud115BtTaskStatus(
              state: 'unknown',
              infoHash: _hash,
              statusText: 'destination unknown',
              targetFolderId: '',
            ),
            deleteTorrent: (_, _, _) async {
              deletes++;
            },
          );
      expect(report.results.single.state, 'duplicate_lookup_unknown');
      expect(deletes, 0);
    },
  );

  test('failed preflight does not expose provider credentials', () async {
    final report =
        await Cloud115BtBatchProcessor(
          ledger: Cloud115BtBatchLedger(File('${tempDir.path}/private.jsonl')),
        ).run(
          entries: [_entry()],
          target: _target,
          preflight: (_) async =>
              throw StateError('UID=secret; SEID=secret; token=secret'),
          upload: (_) async => _upload(),
          recoverUpload: (_, _) async => _upload(),
          submit: (_, _) async => _submitted(),
          queryTask: (_) async => _task('not_found'),
          deleteTorrent: (_, _, _) async {},
        );
    expect(report.stopped, isTrue);
    expect(report.error, isNot(contains('secret')));
    expect(report.results.single.message, isNot(contains('secret')));
  });

  test('legacy duplicate preflight state does not block 115 submit', () async {
    var uploadCalls = 0;
    var submitCalls = 0;
    var deleteCalls = 0;
    final processor = Cloud115BtBatchProcessor(
      ledger: Cloud115BtBatchLedger(File('${tempDir.path}/batch.jsonl')),
    );

    final report = await processor.run(
      entries: [_entry()],
      target: _target,
      preflight: (_) async => _preflight(duplicateStatus: 'unknown'),
      upload: (_) async {
        uploadCalls += 1;
        return _upload();
      },
      recoverUpload: (_, _) async => _upload(),
      submit: (_, _) async {
        submitCalls += 1;
        return _submitted();
      },
      queryTask: (_) async => _task('found'),
      deleteTorrent: (_, _, _) async {
        deleteCalls += 1;
      },
    );

    expect(report.stopped, isFalse);
    expect(report.results.single.state, 'submitted');
    expect(uploadCalls, 0);
    expect(submitCalls, 1);
    expect(deleteCalls, 0);
  });

  test(
    'submit-first preflight submits directly without task-list query',
    () async {
      var uploadCalls = 0;
      var submitCalls = 0;
      var taskQueries = 0;
      final report =
          await Cloud115BtBatchProcessor(
            ledger: Cloud115BtBatchLedger(
              File('${tempDir.path}/submit-first.jsonl'),
            ),
          ).run(
            entries: [_entry()],
            target: _target,
            preflight: (_) async => _preflight(duplicateStatus: 'submit_first'),
            upload: (_) async {
              uploadCalls += 1;
              return _upload();
            },
            recoverUpload: (_, _) async => _upload(),
            submit: (_, _) async {
              submitCalls += 1;
              return _submitted();
            },
            queryTask: (_) async {
              taskQueries += 1;
              return _task('not_found');
            },
            deleteTorrent: (_, _, _) async {},
          );

      expect(report.stopped, isFalse);
      expect(report.results.single.state, 'submitted');
      expect(uploadCalls, 0);
      expect(submitCalls, 1);
      expect(taskQueries, 0);
    },
  );

  test('accepted submission is persisted and never posted again', () async {
    var uploadCalls = 0;
    var submitCalls = 0;
    final ledgerFile = File('${tempDir.path}/batch.jsonl');
    final processor = Cloud115BtBatchProcessor(
      ledger: Cloud115BtBatchLedger(ledgerFile),
    );
    final callbacks = _Callbacks(
      preflight: (_) async => _preflight(),
      recoverUpload: (_, _) async => _upload(),
      queryTask: (_) async => _task('found'),
      deleteTorrent: (_, _, _) async {},
      upload: (_) async {
        uploadCalls += 1;
        return _upload();
      },
      submit: (_, wanted) async {
        submitCalls += 1;
        expect(wanted, [0, 1]);
        return _submitted();
      },
    );

    final first = await processor.run(
      entries: [_entry()],
      target: _target,
      cleanupAfterSubmit: false,
      preflight: callbacks.preflight,
      upload: callbacks.upload,
      recoverUpload: callbacks.recoverUpload,
      submit: callbacks.submit,
      queryTask: callbacks.queryTask,
      deleteTorrent: callbacks.deleteTorrent,
    );
    expect(first.results.single.state, 'submitted');
    expect(uploadCalls, 0);
    expect(submitCalls, 1);

    final restarted = Cloud115BtBatchProcessor(
      ledger: Cloud115BtBatchLedger(ledgerFile),
    );
    final second = await restarted.run(
      entries: [_entry()],
      target: _target,
      cleanupAfterSubmit: false,
      preflight: callbacks.preflight,
      upload: callbacks.upload,
      recoverUpload: callbacks.recoverUpload,
      submit: callbacks.submit,
      queryTask: callbacks.queryTask,
      deleteTorrent: callbacks.deleteTorrent,
    );
    expect(second.results.single.state, 'submitted');
    expect(uploadCalls, 0);
    expect(submitCalls, 1);
  });

  test(
    'a rejected submit with a read-only same-hash match is treated as duplicate and batch continues',
    () async {
      const secondHash = '1123456789012345678901234567890123456789';
      var submitCalls = 0;
      var taskQueries = 0;
      var deleteCalls = 0;
      var uploadCalls = 0;
      final report =
          await Cloud115BtBatchProcessor(
            ledger: Cloud115BtBatchLedger(
              File('${tempDir.path}/rejected-but-present.jsonl'),
            ),
          ).run(
            entries: [
              _entry(),
              _entry(infoHash: secondHash, file: 'two.torrent'),
            ],
            target: _target,
            cleanupAfterSubmit: true,
            preflight: (entry) async => _preflight(
              infoHash: entry.item.infohash,
              torrentName: entry.item.file,
            ),
            upload: (entry) async {
              uploadCalls++;
              return _upload(
                infoHash: entry.item.infohash,
                torrentName: entry.item.file,
              );
            },
            recoverUpload: (_, _) async => _upload(),
            submit: (entry, _) async {
              submitCalls++;
              if (entry.item.infohash == _hash) {
                return const Cloud115BtSubmitResult(
                  state: 'REJECTED',
                  infoHash: _hash,
                  message: '115 rejected this submission',
                  targetFolderId: '30',
                );
              }
              return _submitted(infoHash: entry.item.infohash);
            },
            queryTask: (hash) async {
              taskQueries++;
              return _task(
                hash == _hash ? 'found' : 'not_found',
                infoHash: hash,
              );
            },
            deleteTorrent: (_, _, _) async => deleteCalls++,
          );

      expect(report.stopped, isFalse);
      expect(report.results.map((result) => result.state), [
        'local_torrent_removed',
        'local_torrent_removed',
      ]);
      expect(taskQueries, 1);
      expect(deleteCalls, 2);
      expect(uploadCalls, 0);
      expect(submitCalls, 2);
    },
  );

  test(
    'cleanup preference off preserves a torrent for an existing task',
    () async {
      var deleteCalls = 0;
      final report =
          await Cloud115BtBatchProcessor(
            ledger: Cloud115BtBatchLedger(
              File('${tempDir.path}/duplicate-cleanup-off.jsonl'),
            ),
          ).run(
            entries: [_entry()],
            target: _target,
            cleanupAfterSubmit: false,
            preflight: (_) async => _preflight(),
            upload: (_) async => _upload(),
            recoverUpload: (_, _) async => _upload(),
            submit: (_, _) async => const Cloud115BtSubmitResult(
              state: 'ALREADY_PRESENT',
              infoHash: _hash,
              message: 'same-hash task already exists',
              targetFolderId: '30',
            ),
            queryTask: (_) async => _task('found'),
            deleteTorrent: (_, _, _) async => deleteCalls++,
          );

      expect(report.stopped, isFalse);
      expect(report.results.single.state, 'duplicate_found');
      expect(deleteCalls, 0);
    },
  );

  test(
    'confirmed duplicate cleanup failure is recoverable after restart',
    () async {
      final ledger = Cloud115BtBatchLedger(
        File('${tempDir.path}/duplicate-cleanup-recovery.jsonl'),
      );
      var deleteCalls = 0;
      final processor = Cloud115BtBatchProcessor(ledger: ledger);

      final first = await processor.run(
        entries: [_entry()],
        target: _target,
        cleanupAfterSubmit: true,
        preflight: (_) async => _preflight(),
        upload: (_) async => _upload(),
        recoverUpload: (_, _) async => _upload(),
        submit: (_, _) async => const Cloud115BtSubmitResult(
          state: 'ALREADY_PRESENT',
          infoHash: _hash,
          message: 'same-hash task already exists',
          targetFolderId: '30',
        ),
        queryTask: (_) async => _task('found'),
        deleteTorrent: (_, _, _) async {
          deleteCalls++;
          throw StateError('temporary local delete failure');
        },
      );

      expect(first.stopped, isFalse);
      expect(first.results.single.state, 'duplicate_found_delete_failed');

      final second =
          await Cloud115BtBatchProcessor(
            ledger: Cloud115BtBatchLedger(ledger.file),
          ).run(
            entries: [_entry()],
            target: _target,
            cleanupAfterSubmit: true,
            preflight: (_) async => _preflight(),
            upload: (_) async => fail('A duplicate recovery must not upload.'),
            recoverUpload: (_, _) async =>
                fail('A duplicate recovery must not parse upload.'),
            submit: (_, _) async =>
                fail('A duplicate recovery must not submit again.'),
            queryTask: (_) async => _task('found'),
            deleteTorrent: (_, _, _) async => deleteCalls++,
          );

      expect(second.stopped, isFalse);
      expect(second.results.single.state, 'local_torrent_removed');
      expect(deleteCalls, 2);
    },
  );

  test(
    'accepted task cleanup is retried after its remote listing becomes visible',
    () async {
      var uploadCalls = 0;
      var submitCalls = 0;
      var deleteCalls = 0;
      final ledgerFile = File('${tempDir.path}/cleanup-retry.jsonl');

      final first =
          await Cloud115BtBatchProcessor(
            ledger: Cloud115BtBatchLedger(ledgerFile),
          ).run(
            entries: [_entry()],
            target: _target,
            cleanupAfterSubmit: true,
            preflight: (_) async => _preflight(),
            upload: (_) async {
              uploadCalls++;
              return _upload();
            },
            recoverUpload: (_, _) async => _upload(),
            submit: (_, _) async {
              submitCalls++;
              return _submitted();
            },
            queryTask: (_) async => _task('not_found'),
            deleteTorrent: (_, _, _) async {
              deleteCalls++;
              throw StateError('remote task not visible yet');
            },
          );
      expect(first.results.single.state, 'submitted_cleanup_pending');
      expect(first.hasPendingRecovery, isTrue);
      expect(deleteCalls, 1);

      final second =
          await Cloud115BtBatchProcessor(
            ledger: Cloud115BtBatchLedger(ledgerFile),
          ).run(
            entries: [_entry()],
            target: _target,
            cleanupAfterSubmit: true,
            preflight: (_) async => _preflight(),
            upload: (_) async {
              uploadCalls++;
              return _upload();
            },
            recoverUpload: (_, _) async => _upload(),
            submit: (_, _) async {
              submitCalls++;
              return _submitted();
            },
            queryTask: (_) async => _task('found'),
            deleteTorrent: (_, _, _) async => deleteCalls++,
          );

      expect(second.results.single.state, 'local_torrent_removed');
      expect(uploadCalls, 0);
      expect(submitCalls, 1);
      expect(deleteCalls, 2);
    },
  );

  test('persisted submission keeps cleanup pending when read-only confirmation still fails', () async {
    final ledger = Cloud115BtBatchLedger(File('${tempDir.path}/submitted-pending.jsonl'));
    await _seedLegacyUpload(ledger, 'submitted');
    var submits = 0;
    final report = await Cloud115BtBatchProcessor(ledger: ledger).run(
      entries: [_entry()], target: _target, cleanupAfterSubmit: true,
      preflight: (_) async => _preflight(), upload: (_) async => _upload(),
      recoverUpload: (_, _) async => _upload(),
      submit: (_, _) async { submits++; return _submitted(); },
      queryTask: (_) async => _task('unknown'),
      deleteTorrent: (_, _, _) async => throw StateError('destination not proven'),
    );
    expect(report.results.single.state, 'submitted_cleanup_pending');
    expect(report.hasPendingRecovery, isTrue);
    expect(submits, 0);
    final snapshot = await ledger.snapshotForMonitoring();
    expect(snapshot.records.single.state, 'submitted');
  });

  test(
    'unknown submit outcome is reconciled read-only and never reposted',
    () async {
      var uploadCalls = 0;
      var submitCalls = 0;
      var taskQueries = 0;
      final ledgerFile = File('${tempDir.path}/batch.jsonl');
      final processor = Cloud115BtBatchProcessor(
        ledger: Cloud115BtBatchLedger(ledgerFile),
      );
      final callbacks = _Callbacks(
        preflight: (_) async => _preflight(),
        recoverUpload: (_, _) async => _upload(),
        queryTask: (_) async {
          taskQueries += 1;
          return _task(
            'unknown',
            statusText: '任务列表超过安全查询上限（最多 300 项），无法证明任务不存在',
          );
        },
        deleteTorrent: (_, _, _) async {},
        upload: (_) async {
          uploadCalls += 1;
          return _upload();
        },
        submit: (_, _) async {
          submitCalls += 1;
          return const Cloud115BtSubmitResult(
            state: 'UNKNOWN_REMOTE_STATE',
            infoHash: _hash,
            message: 'unknown',
            targetFolderId: '30',
          );
        },
      );

      final first = await processor.run(
        entries: [_entry()],
        target: _target,
        preflight: callbacks.preflight,
        upload: callbacks.upload,
        recoverUpload: callbacks.recoverUpload,
        submit: callbacks.submit,
        queryTask: callbacks.queryTask,
        deleteTorrent: callbacks.deleteTorrent,
      );
      expect(first.stopped, isFalse);
      expect(first.results.single.state, 'unknown_remote_state');
      expect(submitCalls, 1);

      final restarted = Cloud115BtBatchProcessor(
        ledger: Cloud115BtBatchLedger(ledgerFile),
      );
      final second = await restarted.run(
        entries: [_entry()],
        target: _target,
        preflight: callbacks.preflight,
        upload: callbacks.upload,
        recoverUpload: callbacks.recoverUpload,
        submit: callbacks.submit,
        queryTask: callbacks.queryTask,
        deleteTorrent: callbacks.deleteTorrent,
      );
      expect(second.stopped, isFalse);
      expect(second.results.single.state, 'unknown_remote_state');
      expect(second.results.single.message, contains('无法证明任务不存在'));
      expect(submitCalls, 1);
      expect(uploadCalls, 0);
      expect(taskQueries, 1);
    },
  );

  test(
    'legacy upload states recover read-only without another upload',
    () async {
      for (final legacyState in [
        'upload_started',
        'upload_outcome_unknown',
        'upload_parse_pending',
        'torrent_parsed',
      ]) {
        final file = File('${tempDir.path}/$legacyState.jsonl');
        final ledger = Cloud115BtBatchLedger(file);
        await _seedLegacyUpload(ledger, legacyState);
        var uploads = 0;
        var recoveries = 0;
        var submits = 0;
        Future<Cloud115BtBatchReport> run() =>
            Cloud115BtBatchProcessor(ledger: ledger).run(
              entries: [_entry()],
              target: _target,
              preflight: (_) async => _preflight(),
              upload: (_) async {
                uploads++;
                return _upload();
              },
              recoverUpload: (_, _) async {
                recoveries++;
                if (recoveries == 1) {
                  throw StateError('read-only recovery failed');
                }
                return _upload();
              },
              submit: (_, wanted) async {
                submits++;
                expect(wanted, [0, 1]);
                return _submitted();
              },
              queryTask: (_) async => _task('not_found'),
              deleteTorrent: (_, _, _) async {},
            );
        final first = await run();
        expect(
          first.results.single.state,
          legacyState == 'upload_parse_pending'
              ? 'upload_parse_pending'
              : 'upload_outcome_unknown',
        );
        expect(submits, 0);
        final recovered = await run();
        expect(recovered.results.single.state, 'submitted');
        expect(uploads, 0);
        expect(recoveries, 2);
        expect(submits, 1);
      }
    },
  );

  test(
    'unknown legacy upload is isolated while the next torrent is submitted directly',
    () async {
      final ledger = Cloud115BtBatchLedger(
        File('${tempDir.path}/legacy-continues.jsonl'),
      );
      await _seedLegacyUpload(ledger, 'upload_outcome_unknown');
      const secondHash = '1123456789012345678901234567890123456789';
      var uploads = 0;
      var recoveries = 0;
      var submits = 0;
      Future<Cloud115BtBatchReport> run() =>
          Cloud115BtBatchProcessor(ledger: ledger).run(
            entries: [
              _entry(),
              _entry(infoHash: secondHash, file: 'two.torrent'),
            ],
            target: _target,
            preflight: (entry) async => _preflight(
              infoHash: entry.item.infohash,
              torrentName: entry.item.file,
            ),
            upload: (_) async {
              uploads++;
              return _upload();
            },
            recoverUpload: (_, _) async {
              recoveries++;
              return _upload(state: 'UNKNOWN_UPLOAD_RESULT');
            },
            submit: (entry, _) async {
              submits++;
              return _submitted(infoHash: entry.item.infohash);
            },
            queryTask: (hash) async => _task('not_found', infoHash: hash),
            deleteTorrent: (_, _, _) async {},
          );
      for (var attempt = 0; attempt < 2; attempt++) {
        final report = await run();
        expect(report.results.map((r) => r.state), [
          'upload_outcome_unknown',
          'submitted',
        ]);
      }
      expect(uploads, 0);
      expect(recoveries, 2);
      expect(submits, 1);
    },
  );

  test(
    'WAF on legacy upload recovery stops before the next torrent write',
    () async {
      final ledger = Cloud115BtBatchLedger(
        File('${tempDir.path}/legacy-waf.jsonl'),
      );
      await _seedLegacyUpload(ledger, 'upload_started');
      const secondHash = '4123456789012345678901234567890123456789';
      var uploads = 0;
      var submits = 0;
      final report = await Cloud115BtBatchProcessor(ledger: ledger).run(
        entries: [
          _entry(),
          _entry(infoHash: secondHash, file: 'two.torrent'),
        ],
        target: _target,
        preflight: (entry) async => _preflight(
          infoHash: entry.item.infohash,
          torrentName: entry.item.file,
        ),
        upload: (_) async {
          uploads++;
          return _upload();
        },
        recoverUpload: (_, _) async =>
            _upload(state: 'WAF_BLOCKED', message: 'HTTP 405'),
        submit: (entry, _) async {
          submits++;
          return _submitted(infoHash: entry.item.infohash);
        },
        queryTask: (hash) async => _task('not_found', infoHash: hash),
        deleteTorrent: (_, _, _) async {},
      );
      expect(report.stopped, isTrue);
      expect(report.results.single.state, 'waf_blocked');
      expect(uploads, 0);
      expect(submits, 0);
    },
  );

  test(
    'submit authentication failure stops before the next Torrent write',
    () async {
      const secondHash = '6123456789012345678901234567890123456789';
      var uploadCalls = 0;
      var submitCalls = 0;
      final report =
          await Cloud115BtBatchProcessor(
            ledger: Cloud115BtBatchLedger(
              File('${tempDir.path}/submit-auth-failure.jsonl'),
            ),
          ).run(
            entries: [
              _entry(),
              _entry(infoHash: secondHash, file: 'two.torrent'),
            ],
            target: _target,
            preflight: (entry) async => _preflight(
              infoHash: entry.item.infohash,
              torrentName: entry.item.file,
            ),
            upload: (entry) async {
              uploadCalls++;
              return _upload(
                infoHash: entry.item.infohash,
                torrentName: entry.item.file,
              );
            },
            recoverUpload: (_, _) async => _upload(),
            submit: (entry, _) async {
              submitCalls++;
              return Cloud115BtSubmitResult(
                state: 'AUTH_EXPIRED',
                infoHash: entry.item.infohash,
                message: 'Cookie session expired',
                targetFolderId: _target.id,
              );
            },
            queryTask: (hash) async => _task('not_found', infoHash: hash),
            deleteTorrent: (_, _, _) async {},
          );

      expect(report.stopped, isTrue);
      expect(report.results.single.state, 'auth_expired');
      expect(uploadCalls, 0);
      expect(submitCalls, 1);
    },
  );

  test('WAF during rejected-submit reconciliation stops the batch', () async {
    const secondHash = '5123456789012345678901234567890123456789';
    var uploadCalls = 0;
    var submitCalls = 0;
    final report =
        await Cloud115BtBatchProcessor(
          ledger: Cloud115BtBatchLedger(
            File('${tempDir.path}/rejected-reconcile-waf.jsonl'),
          ),
        ).run(
          entries: [
            _entry(),
            _entry(infoHash: secondHash, file: 'two.torrent'),
          ],
          target: _target,
          preflight: (entry) async => _preflight(
            infoHash: entry.item.infohash,
            torrentName: entry.item.file,
          ),
          upload: (entry) async {
            uploadCalls++;
            return _upload(
              infoHash: entry.item.infohash,
              torrentName: entry.item.file,
            );
          },
          recoverUpload: (_, _) async => _upload(),
          submit: (entry, _) async {
            submitCalls++;
            return Cloud115BtSubmitResult(
              state: 'REJECTED',
              infoHash: entry.item.infohash,
              message: 'rejected',
              targetFolderId: _target.id,
            );
          },
          queryTask: (hash) async => _task(
            'WAF_BLOCKED',
            infoHash: hash,
            statusCode: 405,
            statusText: 'WAF 405',
          ),
          deleteTorrent: (_, _, _) async {},
        );

    expect(report.stopped, isTrue);
    expect(report.results.single.state, 'waf_blocked');
    expect(uploadCalls, 0);
    expect(submitCalls, 1);
  });

  test('WAF state remains write-blocked after processor restart', () async {
    var uploadCalls = 0;
    var submitCalls = 0;
    final ledgerFile = File('${tempDir.path}/waf.jsonl');
    final first =
        await Cloud115BtBatchProcessor(
          ledger: Cloud115BtBatchLedger(ledgerFile),
        ).run(
          entries: [
            _entry(),
            _entry(
              infoHash: '3123456789012345678901234567890123456789',
              file: 'three.torrent',
            ),
          ],
          target: _target,
          preflight: (entry) async => _preflight(
            infoHash: entry.item.infohash,
            torrentName: entry.item.file,
          ),
          upload: (_) async {
            uploadCalls += 1;
            return _upload();
          },
          recoverUpload: (_, _) async => _upload(),
          submit: (_, _) async {
            submitCalls += 1;
            return const Cloud115BtSubmitResult(
              state: 'WAF_BLOCKED',
              infoHash: _hash,
              message: 'WAF blocked',
              targetFolderId: '30',
            );
          },
          queryTask: (_) async => _task('not_found'),
          deleteTorrent: (_, _, _) async {},
        );
    expect(first.results.single.state, 'waf_blocked');

    final restarted =
        await Cloud115BtBatchProcessor(
          ledger: Cloud115BtBatchLedger(ledgerFile),
        ).run(
          entries: [_entry()],
          target: _target,
          preflight: (_) async => _preflight(),
          upload: (_) async {
            uploadCalls += 1;
            return _upload();
          },
          recoverUpload: (_, _) async => _upload(),
          submit: (_, _) async {
            submitCalls += 1;
            return _submitted();
          },
          queryTask: (_) async => _task('not_found'),
          deleteTorrent: (_, _, _) async {},
        );
    expect(restarted.stopped, isTrue);
    expect(restarted.results.single.state, 'waf_blocked');
    expect(uploadCalls, 0);
    expect(submitCalls, 1);
  });

  test('corrupt ledger fails closed before any remote callback', () async {
    var preflightCalls = 0;
    final file = File('${tempDir.path}/corrupt.jsonl');
    await file.writeAsString('{"type":"bad"}\n');
    final report =
        await Cloud115BtBatchProcessor(ledger: Cloud115BtBatchLedger(file)).run(
          entries: [_entry()],
          target: _target,
          preflight: (_) async {
            preflightCalls += 1;
            return _preflight();
          },
          upload: (_) async => _upload(),
          recoverUpload: (_, _) async => _upload(),
          submit: (_, _) async => _submitted(),
          queryTask: (_) async => _task('found'),
          deleteTorrent: (_, _, _) async {},
        );
    expect(report.stopped, isTrue);
    expect(report.error, contains('账本不可用'));
    expect(preflightCalls, 0);
  });

  test(
    'unknown ledger state fails closed instead of authorizing a new post',
    () async {
      var uploadCalls = 0;
      final file = File('${tempDir.path}/unknown-state.jsonl');
      await file.writeAsString(
        '{"type":"item","accountId":"test-account",'
        '"infoHash":"$_hash","targetFolderId":"30",'
        '"relativeFile":"one.torrent","state":"future_state",'
        '"updatedAtUtc":"2026-10-12T00:00:00Z"}\n',
      );
      final report =
          await Cloud115BtBatchProcessor(
            ledger: Cloud115BtBatchLedger(file),
          ).run(
            entries: [_entry()],
            target: _target,
            preflight: (_) async => _preflight(),
            upload: (_) async {
              uploadCalls += 1;
              return _upload();
            },
            recoverUpload: (_, _) async => _upload(),
            submit: (_, _) async => _submitted(),
            queryTask: (_) async => _task('not_found'),
            deleteTorrent: (_, _, _) async {},
          );
      expect(report.stopped, isTrue);
      expect(uploadCalls, 0);
    },
  );

  test(
    'ledger persists non-secret identity and preference across instances',
    () async {
      final path = File('${tempDir.path}/batch.jsonl');
      final ledger = Cloud115BtBatchLedger(path);
      await ledger.setCleanupAfterSubmit(true);
      await ledger.setSelectionPreferences(
        sourceId: 'cookie-source-2',
        target: _target,
      );
      final processor = Cloud115BtBatchProcessor(ledger: ledger);
      await processor.run(
        entries: [_entry()],
        target: _target,
        preflight: (_) async => _preflight(),
        upload: (_) async => _upload(),
        recoverUpload: (_, _) async => _upload(),
        submit: (_, _) async => _submitted(),
        queryTask: (_) async => _task('found'),
        deleteTorrent: (_, _, _) async {},
      );

      final reopened = Cloud115BtBatchLedger(path);
      expect(await reopened.cleanupAfterSubmit(), isTrue);
      final preferences = await reopened.selectionPreferences();
      expect(preferences.sourceId, 'cookie-source-2');
      expect(preferences.target, _target);
      final content = await path.readAsString();
      expect(content, contains('test-account'));
      expect(content, contains(_hash));
      expect(content, contains('cookie-source-2'));
      expect(content, isNot(contains('fake-cookie')));
      expect(content, isNot(contains('UID=12345_mock')));
      expect(content, isNot(contains('token')));

      await reopened.setSelectionPreferences(
        sourceId: 'cookie-source-1',
        target: null,
      );
      final changed = await Cloud115BtBatchLedger(path).selectionPreferences();
      expect(changed.sourceId, 'cookie-source-1');
      expect(changed.target, isNull);
    },
  );
}

class _Callbacks {
  const _Callbacks({
    required this.preflight,
    required this.upload,
    required this.recoverUpload,
    required this.submit,
    required this.queryTask,
    required this.deleteTorrent,
  });

  final Future<Cloud115BtPreflight> Function(EhCheckedManifestEntry) preflight;
  final Future<Cloud115BtUploadParseResult> Function(EhCheckedManifestEntry)
  upload;
  final Future<Cloud115BtUploadParseResult> Function(
    EhCheckedManifestEntry,
    String?,
  )
  recoverUpload;
  final Future<Cloud115BtSubmitResult> Function(
    EhCheckedManifestEntry,
    List<int>,
  )
  submit;
  final Future<Cloud115BtTaskStatus> Function(String) queryTask;
  final Future<void> Function(
    EhCheckedManifestEntry,
    String,
    Cloud115BtFolderSelection,
  )
  deleteTorrent;
}

const _hash = '0123456789012345678901234567890123456789';
const _target = Cloud115BtFolderSelection(
  id: '30',
  parentId: '10',
  name: 'EH订阅',
  path: '/漫画/EH订阅',
);

EhCheckedManifestEntry _entry({
  String infoHash = _hash,
  String file = 'one.torrent',
}) => EhCheckedManifestEntry(
  item: EhSavedItem(
    infohash: infoHash,
    titleJpn: 'title',
    rating: 5,
    downloads: 1,
    requiredDl: 0,
    postedUtc: '2026-10-12',
    ageYears: null,
    category: 'Manga',
    filecount: 1,
    filesize: 10,
    tags: [],
    file: file,
    bytes: 10,
    torrentName: file,
    savedAtUtc: '2026-10-12T00:00:00Z',
  ),
  fileStatus: EhSavedFileStatus.present,
  validationStatus: EhTorrentValidationStatus.valid,
  fullPath: '/isolated/$file',
);

Future<void> _seedLegacyUpload(Cloud115BtBatchLedger ledger, String state) =>
    ledger.withExclusiveLock(
      (transaction) => transaction.recordItem(
        accountId: 'test-account',
        infoHash: _hash,
        targetFolderId: _target.id,
        relativeFile: 'one.torrent',
        state: state,
      ),
    );

Cloud115BtPreflight _preflight({
  String duplicateStatus = 'submit_first',
  String duplicateDetail = '',
  String infoHash = _hash,
  String? torrentName,
}) => Cloud115BtPreflight(
  accountId: 'test-account',
  targetFolderPath: _target.path,
  targetFolderId: _target.id,
  torrentName:
      torrentName ?? (infoHash == _hash ? 'one.torrent' : 'two.torrent'),
  torrentSize: BigInt.from(10),
  torrentSha1: 'torrent-sha1',
  infoHash: infoHash,
  files: [
    Cloud115BtFile(index: 0, path: 'a.cbz', size: BigInt.one),
    Cloud115BtFile(index: 1, path: 'b.cbz', size: BigInt.one),
  ],
  duplicateStatus: duplicateStatus,
  duplicateDetail: duplicateDetail,
  writeGate: 'REQUIRES_EXPLICIT_CONFIRMATION',
);

Cloud115BtUploadParseResult _upload({
  String state = 'PARSED',
  String message = 'parsed',
  String infoHash = _hash,
  String? torrentName,
}) => Cloud115BtUploadParseResult(
  state: state,
  message: message,
  accountId: 'test-account',
  targetFolderPath: _target.path,
  targetFolderId: _target.id,
  torrentName:
      torrentName ?? (infoHash == _hash ? 'one.torrent' : 'two.torrent'),
  torrentSize: BigInt.from(10),
  torrentSha1: 'torrent-sha1',
  infoHash: infoHash,
  uploadReused: false,
  files: [
    Cloud115BtFile(index: 0, path: 'a.cbz', size: BigInt.one),
    Cloud115BtFile(index: 1, path: 'b.cbz', size: BigInt.one),
  ],
);

Cloud115BtSubmitResult _submitted({String infoHash = _hash}) =>
    Cloud115BtSubmitResult(
      state: 'ACCEPTED',
      infoHash: infoHash,
      message: 'accepted',
      targetFolderId: '30',
    );

Cloud115BtTaskStatus _task(
  String state, {
  String? statusText,
  int? statusCode,
  String infoHash = _hash,
}) => Cloud115BtTaskStatus(
  state: state,
  infoHash: infoHash,
  statusCode: statusCode,
  statusText: statusText ?? state,
  targetFolderId: '30',
);
