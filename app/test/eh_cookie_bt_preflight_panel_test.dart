import 'dart:convert';

import 'package:app/src/rust/source/cloud115/offline.dart';
import 'package:app/store/cloud115_bt_batch.dart';
import 'package:app/store/cloud115_session.dart';
import 'package:app/store/eh_subscription_store.dart';
import 'package:app/store/models.dart';
import 'package:app/ui/eh_subscription_panel.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  testWidgets(
    'Daily run-now performs a manual EH scan then requires one BT batch confirmation',
    (tester) async {
      var collectCalls = 0;
      var batchCalls = 0;
      final store = EhSubscriptionStore.forTest(
        rules: const {'out_dir': r'C:\isolated\eh'},
        collectRunner: (_) async {
          collectCalls++;
          return '{"stage":"done","saved":1}';
        },
        checkedManifestLoader: (_) async => jsonEncode({
          'status': 'ready',
          'entries': [_checkedEntryJson()],
        }),
        torrentDirectoryLoader: (_) async => jsonEncode({
          'status': 'ready',
          'entries': [_checkedEntryJson()],
        }),
      );

      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: SingleChildScrollView(
              child: EhSubscriptionPanel(
                storeOverride: store,
                btSourcesOverride: [_cookieSource()],
                btBatchPreferencesLoader: () async =>
                    Cloud115BtBatchSelectionPreferences(
                      sourceId: 'cookie-source',
                      target: _targetSelection,
                    ),
                btBatchRunner:
                    (source, entries, target, cleanupAfterSubmit) async {
                      batchCalls++;
                      expect(source.id, 'cookie-source');
                      expect(entries.single.item.file, 'one.torrent');
                      expect(target.id, _targetSelection.id);
                      return Cloud115BtBatchReport(
                        results: [
                          Cloud115BtBatchItemResult(
                            relativeFile: 'one.torrent',
                            infoHash: entries.single.item.infohash,
                            state: 'submitted',
                          ),
                        ],
                        stopped: false,
                      );
                    },
              ),
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();

      expect(collectCalls, 0);
      expect(batchCalls, 0);
      await tester.tap(find.byKey(const ValueKey('eh-full-workflow-run')));
      await tester.pump();
      await tester.pump(const Duration(seconds: 1));

      expect(collectCalls, 1, reason: 'The button must start the EH scan.');
      expect(batchCalls, 0, reason: '115 waits for explicit confirmation.');
      expect(find.textContaining('确认批量处理'), findsOneWidget);
      await tester.tap(find.text('确认开始批次').last);
      await tester.pumpAndSettle();

      expect(batchCalls, 1);
      expect(store.dailyAutomation.dailyEnabled, isFalse);
      expect(
        store.dailyAutomation.lastRun?.cloudStage.submitted,
        1,
        reason: 'Manual results should be reflected in the recent run card.',
      );

      await tester.pumpWidget(const SizedBox.shrink());
      store.dispose();
    },
  );

  testWidgets(
    'incomplete EH scan still offers already saved checked Torrents to the batch',
    (tester) async {
      var batchCalls = 0;
      final store = EhSubscriptionStore.forTest(
        rules: const {'out_dir': r'C:\isolated\eh'},
        collectRunner: (_) async =>
            '{"stage":"failed","saved":1,"error":"EH network timeout"}',
        checkedManifestLoader: (_) async => jsonEncode({
          'status': 'ready',
          'entries': [_checkedEntryJson()],
        }),
        torrentDirectoryLoader: (_) async => jsonEncode({
          'status': 'ready',
          'entries': [_checkedEntryJson()],
        }),
      );

      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: SingleChildScrollView(
              child: EhSubscriptionPanel(
                storeOverride: store,
                btSourcesOverride: [_cookieSource()],
                btBatchPreferencesLoader: () async =>
                    Cloud115BtBatchSelectionPreferences(
                      sourceId: 'cookie-source',
                      target: _targetSelection,
                    ),
                btBatchRunner:
                    (source, entries, target, cleanupAfterSubmit) async {
                      batchCalls++;
                      expect(entries.single.item.file, 'one.torrent');
                      return const Cloud115BtBatchReport(
                        results: [],
                        stopped: false,
                      );
                    },
              ),
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();

      await tester.tap(find.byKey(const ValueKey('eh-full-workflow-run')));
      await tester.pump();
      await tester.pump(const Duration(seconds: 1));

      final confirmation = tester.widget<AlertDialog>(find.byType(AlertDialog));
      final confirmationText =
          ((confirmation.content! as SingleChildScrollView).child! as Text)
              .data!;
      expect(
        confirmationText,
        contains('EH network timeout'),
        reason: 'The incomplete scan must remain visible before confirmation.',
      );
      expect(
        find.text('确认批量处理 1 个 Torrent'),
        findsOneWidget,
        reason: 'A prior valid disk Torrent should still be offered.',
      );
      expect(batchCalls, 0, reason: '115 remains behind confirmation.');

      await tester.tap(find.text('确认开始批次').last);
      await tester.pumpAndSettle();

      expect(batchCalls, 1);
      expect(
        store.dailyAutomation.lastRun?.status.name,
        'partial',
        reason: 'A successful cloud stage cannot hide the incomplete EH scan.',
      );
      await tester.pumpWidget(const SizedBox.shrink());
      store.dispose();
    },
  );

  testWidgets('batch BT processing is inert until one explicit confirmation', (
    tester,
  ) async {
    var batchCalls = 0;
    List<EhCheckedManifestEntry>? processedEntries;
    Cloud115BtFolderSelection? processedTarget;
    final source = _cookieSource();

    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: SingleChildScrollView(
            child: EhSubscriptionPanel(
              storeOverride: _store(),
              btSourcesOverride: [source],
              btTargetSelectionOverride: _targetSelection,
              btBatchRunner:
                  (source, entries, target, cleanupAfterSubmit) async {
                    batchCalls += 1;
                    expect(source.id, 'cookie-source');
                    expect(cleanupAfterSubmit, isFalse);
                    processedEntries = entries;
                    processedTarget = target;
                    return Cloud115BtBatchReport(
                      results: [
                        Cloud115BtBatchItemResult(
                          relativeFile: entries.single.item.file,
                          infoHash: entries.single.item.infohash,
                          state: 'submitted',
                          message: '115 accepted BT submission request',
                        ),
                      ],
                      stopped: false,
                    );
                  },
            ),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
    await _openAdvanced(tester);

    expect(batchCalls, 0, reason: 'Opening the panel must not contact 115.');
    expect(find.text('已校验 Torrent（默认第一项）'), findsNothing);
    await _tapVisible(tester, '批量上传并提交 BT 任务（1）');
    await tester.pumpAndSettle();
    expect(batchCalls, 0, reason: 'The batch dialog gates all work.');
    expect(find.text('确认批量处理 1 个 Torrent'), findsOneWidget);

    await tester.tap(find.text('确认开始批次').last);
    await tester.pumpAndSettle();
    expect(batchCalls, 1);
    expect(processedEntries?.single.item.file, 'one.torrent');
    expect(processedTarget, _targetSelection);
    expect(find.text('批次结果：1 项'), findsOneWidget);
    await _tapVisible(tester, '批次结果：1 项');
    await tester.pumpAndSettle();
    expect(find.textContaining('115 已接受提交请求'), findsOneWidget);
  });

  testWidgets('batch candidates come from disk inventory, not the manifest', (
    tester,
  ) async {
    final store = EhSubscriptionStore.forTest(
      rules: const {'out_dir': r'C:\isolated\eh'},
      manifestReport: const EhManifestReport(
        status: EhManifestReadStatus.damaged,
      ),
      torrentDirectoryReport: EhManifestReport(
        status: EhManifestReadStatus.ready,
        entries: [
          _checkedEntry(),
          _checkedEntry(
            file: 'orphan.torrent',
            infoHash: 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
          ),
        ],
      ),
    );

    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: SingleChildScrollView(
            child: EhSubscriptionPanel(
              storeOverride: store,
              btSourcesOverride: [_cookieSource()],
              btTargetSelectionOverride: _targetSelection,
              btBatchRunner: (_, _, _, _) async =>
                  const Cloud115BtBatchReport(results: [], stopped: false),
            ),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
    await _openAdvanced(tester);

    expect(find.textContaining('保存目录实存 Torrent：2 个有效'), findsOneWidget);
    await _tapVisible(tester, '批量上传并提交 BT 任务（2）');
    await tester.pumpAndSettle();
    expect(find.text('确认批量处理 2 个 Torrent'), findsOneWidget);
    await tester.tap(find.text('取消').last);
    await tester.pumpAndSettle();
  });

  testWidgets('batch confirmation explains submit-first duplicate handling', (
    tester,
  ) async {
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: SingleChildScrollView(
            child: EhSubscriptionPanel(
              storeOverride: _store(),
              btSourcesOverride: [_cookieSource()],
              btTargetSelectionOverride: _targetSelection,
              btBatchRunner:
                  (source, entries, target, cleanupAfterSubmit) async =>
                      const Cloud115BtBatchReport(results: [], stopped: false),
            ),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
    await _openAdvanced(tester);

    await _tapVisible(tester, '批量上传并提交 BT 任务（1）');
    await tester.pumpAndSettle();
    final dialog = tester.widget<AlertDialog>(find.byType(AlertDialog));
    final content = dialog.content! as SingleChildScrollView;
    final message = (content.child! as Text).data!;
    expect(message, contains('不在提交前扫描完整离线任务列表'));
    expect(message, contains('已有记录交由 115 在提交时处理'));
    expect(message, contains('继续下一项'));
    expect(message, contains('WAF 阻断会停止整批'));
    await tester.tap(find.text('取消').last);
    await tester.pumpAndSettle();
  });

  testWidgets('uploaded parse pending result explains read-only recovery', (
    tester,
  ) async {
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: SingleChildScrollView(
            child: EhSubscriptionPanel(
              storeOverride: _store(),
              btSourcesOverride: [_cookieSource()],
              btTargetSelectionOverride: _targetSelection,
              btBatchRunner:
                  (source, entries, target, cleanupAfterSubmit) async =>
                      Cloud115BtBatchReport(
                        stopped: true,
                        results: [
                          Cloud115BtBatchItemResult(
                            relativeFile: entries.single.item.file,
                            infoHash: entries.single.item.infohash,
                            state: 'upload_parse_pending',
                            message: 'Torrent 已上传；115 解析结果暂不可用，不会重传。',
                          ),
                        ],
                      ),
            ),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
    await _openAdvanced(tester);

    await _tapVisible(tester, '批量上传并提交 BT 任务（1）');
    await tester.pumpAndSettle();
    await tester.tap(find.text('确认开始批次').last);
    await tester.pumpAndSettle();

    expect(find.textContaining('本项已上传、解析待恢复'), findsOneWidget);
    expect(find.textContaining('不会重传'), findsOneWidget);
  });

  testWidgets('Cookie source and target are saved and restored locally', (
    tester,
  ) async {
    final first = _cookieSource(id: 'cookie-source-1', name: '第一 Cookie 源');
    final second = _cookieSource(id: 'cookie-source-2', name: '第二 Cookie 源');
    var savedPreferences = const Cloud115BtBatchSelectionPreferences();
    var folderReads = 0;

    Widget buildPanel() => MaterialApp(
      home: Scaffold(
        body: SingleChildScrollView(
          child: EhSubscriptionPanel(
            storeOverride: _store(),
            btSourcesOverride: [first, second],
            btBatchPreferencesLoader: () async => savedPreferences,
            btBatchPreferencesSaver: (sourceId, target, mode) async {
              savedPreferences = Cloud115BtBatchSelectionPreferences(
                sourceId: sourceId,
                target: target,
                mode: mode,
              );
            },
            btBatchRunner:
                (source, entries, target, cleanupAfterSubmit) async =>
                    const Cloud115BtBatchReport(results: [], stopped: false),
            btFolderListRunner: (source, parentId) async {
              folderReads += 1;
              if (parentId == '0') {
                return const [Cloud115BtFolder(id: '30', name: '自定义目标')];
              }
              return const [];
            },
          ),
        ),
      ),
    );

    await tester.pumpWidget(buildPanel());
    await tester.pumpAndSettle();
    await _openAdvanced(tester);
    expect(folderReads, 0, reason: 'Restoring settings must not query 115.');

    final firstSourceDropdown = find.byKey(
      const ValueKey('eh115-bt-source-cloud115-cookie-source-1'),
    );
    await tester.ensureVisible(firstSourceDropdown);
    expect(
      tester
          .widget<DropdownButtonFormField<String>>(firstSourceDropdown)
          .onChanged,
      isNotNull,
    );
    await tester.tap(firstSourceDropdown);
    await tester.pumpAndSettle();
    expect(find.text('第二 Cookie 源'), findsWidgets);
    await tester.tap(find.text('第二 Cookie 源').last);
    await tester.pumpAndSettle();
    expect(savedPreferences.sourceId, 'cookie-source-2');
    expect(savedPreferences.target, isNull);

    await _tapVisible(tester, '选择目录');
    await tester.pumpAndSettle();
    expect(folderReads, 1);
    await tester.tap(find.text('自定义目标'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('使用此目录'));
    await tester.pumpAndSettle();

    expect(savedPreferences.sourceId, 'cookie-source-2');
    expect(savedPreferences.target?.id, '30');
    expect(savedPreferences.target?.path, '/自定义目标');
    expect(folderReads, 2);

    await tester.pumpWidget(buildPanel());
    await tester.pumpAndSettle();
    await _openAdvanced(tester);
    expect(find.text('第二 Cookie 源'), findsOneWidget);
    expect(find.textContaining('/自定义目标（ID 30）'), findsOneWidget);
    expect(folderReads, 2, reason: 'Restoring settings must remain offline.');

    final secondSourceDropdown = find.byKey(
      const ValueKey('eh115-bt-source-cloud115-cookie-source-2'),
    );
    await tester.ensureVisible(secondSourceDropdown);
    await tester.tap(secondSourceDropdown);
    await tester.pumpAndSettle();
    await tester.tap(find.text('第一 Cookie 源').last);
    await tester.pumpAndSettle();
    expect(savedPreferences.sourceId, 'cookie-source-1');
    expect(savedPreferences.target, isNull);
  });

  testWidgets('single Torrent writes use the idempotent batch confirmation', (
    tester,
  ) async {
    var preflightCalls = 0;
    var uploadCalls = 0;
    var submitCalls = 0;
    var batchCalls = 0;
    var statusCalls = 0;
    final source = _cookieSource();
    final store = _store();

    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: SingleChildScrollView(
            child: EhSubscriptionPanel(
              storeOverride: store,
              btSourcesOverride: [source],
              btTargetSelectionOverride: _targetSelection,
              btPreflightRunner:
                  (source, outputDir, relativeFile, infoHash, target) async {
                    preflightCalls++;
                    expect(source.id, 'cookie-source');
                    expect(outputDir, r'C:\isolated\eh');
                    expect(relativeFile, 'one.torrent');
                    expect(target, _targetSelection);
                    return _preflight();
                  },
              btUploadRunner:
                  (source, outputDir, relativeFile, infoHash, target) async {
                    uploadCalls++;
                    expect(source.id, 'cookie-source');
                    expect(target, _targetSelection);
                    return _parsedUpload();
                  },
              btSubmitRunner:
                  (
                    source,
                    outputDir,
                    relativeFile,
                    infoHash,
                    target,
                    wanted,
                  ) async {
                    submitCalls++;
                    expect(target, _targetSelection);
                    expect(wanted, [0]);
                    return const Cloud115BtSubmitResult(
                      state: 'ACCEPTED',
                      infoHash: '0123456789012345678901234567890123456789',
                      message: '远端任务列表已确认该任务',
                      targetFolderId: '12345',
                    );
                  },
              btBatchRunner:
                  (source, entries, target, cleanupAfterSubmit) async {
                    batchCalls++;
                    expect(source.id, 'cookie-source');
                    expect(entries.single.item.file, 'one.torrent');
                    expect(target, _targetSelection);
                    expect(cleanupAfterSubmit, isFalse);
                    return Cloud115BtBatchReport(
                      results: [
                        Cloud115BtBatchItemResult(
                          relativeFile: 'one.torrent',
                          infoHash: '0123456789012345678901234567890123456789',
                          state: 'submitted',
                          message: '115 已接受 BT 提交请求',
                        ),
                      ],
                      stopped: false,
                    );
                  },
              btTaskStatusRunner: (source, infoHash, target) async {
                statusCalls++;
                expect(infoHash, '0123456789012345678901234567890123456789');
                expect(target, _targetSelection);
                return const Cloud115BtTaskStatus(
                  state: 'found',
                  infoHash: '0123456789012345678901234567890123456789',
                  statusText: 'downloading',
                  percentDone: 0.25,
                  targetFolderId: '12345',
                );
              },
            ),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
    await _openAdvanced(tester);

    expect(preflightCalls, 0);
    expect(uploadCalls, 0);
    expect(submitCalls, 0);
    expect(batchCalls, 0);
    expect(statusCalls, 0);
    expect(find.text('115 云端下载'), findsOneWidget);

    await _tapVisible(tester, '检查最新有效种子');
    expect(preflightCalls, 1);
    expect(uploadCalls, 0);
    await tester.pumpAndSettle();
    expect(find.textContaining('/漫画/EH订阅（ID 12345）'), findsNWidgets(2));
    expect(find.textContaining('按 115 BT 提交结果处理重复项'), findsOneWidget);

    await _tapVisible(tester, '幂等处理此 Torrent');
    await tester.pumpAndSettle();
    expect(find.byType(AlertDialog), findsOneWidget);
    expect(
      tester
          .widgetList<DropdownButtonFormField<String>>(
            find.byType(DropdownButtonFormField<String>),
          )
          .every((field) => field.onChanged == null),
      isTrue,
      reason:
          'The selected Torrent and Cookie source stay fixed while confirming.',
    );
    expect(batchCalls, 0, reason: 'The confirmation must gate the batch.');
    await tester.tap(find.text('取消').last);
    await tester.pumpAndSettle();
    expect(batchCalls, 0);

    await _tapVisible(tester, '幂等处理此 Torrent');
    await tester.pumpAndSettle();
    await tester.tap(find.text('确认开始批次').last);
    await tester.pumpAndSettle();
    expect(batchCalls, 1);
    expect(
      uploadCalls,
      0,
      reason: 'UI writes must go through the batch runner.',
    );
    expect(
      submitCalls,
      0,
      reason: 'UI writes must go through the batch runner.',
    );
    expect(find.text('批次结果：1 项'), findsOneWidget);

    await _tapVisible(tester, '查询 BT 任务');
    await tester.pumpAndSettle();
    expect(statusCalls, 1);
    expect(find.textContaining('downloading'), findsOneWidget);
    expect(find.textContaining('25%'), findsOneWidget);
    expect(batchCalls, 1, reason: 'Read-only status checks must not resubmit.');
  });

  testWidgets(
    'unknown upload status remains in the shared idempotent batch flow',
    (tester) async {
      var batchCalls = 0;
      var directUploadCalls = 0;
      final store = _store();

      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: SingleChildScrollView(
              child: EhSubscriptionPanel(
                storeOverride: store,
                btSourcesOverride: [_cookieSource()],
                btTargetSelectionOverride: _targetSelection,
                btPreflightRunner:
                    (source, outputDir, relativeFile, infoHash, target) async =>
                        _preflight(),
                btUploadRunner:
                    (source, outputDir, relativeFile, infoHash, target) async {
                      directUploadCalls++;
                      return _unknownUpload();
                    },
                btBatchRunner:
                    (source, entries, target, cleanupAfterSubmit) async {
                      batchCalls++;
                      return Cloud115BtBatchReport(
                        results: [
                          Cloud115BtBatchItemResult(
                            relativeFile: 'one.torrent',
                            infoHash:
                                '0123456789012345678901234567890123456789',
                            state: 'upload_outcome_unknown',
                            message: '本项上传结果未知；不会重传。',
                          ),
                        ],
                        stopped: false,
                      );
                    },
              ),
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      await _openAdvanced(tester);
      await _tapVisible(tester, '检查最新有效种子');
      await tester.pumpAndSettle();

      expect(find.text('上传 Torrent 并解析'), findsNothing);
      expect(find.text('提交 BT 任务（一次）'), findsNothing);
      await _tapVisible(tester, '幂等处理此 Torrent');
      await tester.pumpAndSettle();
      await tester.tap(find.text('确认开始批次').last);
      await tester.pumpAndSettle();

      expect(batchCalls, 1);
      expect(directUploadCalls, 0);
      await tester.tap(find.text('批次结果：1 项'));
      await tester.pumpAndSettle();
      expect(find.textContaining('本项上传结果未知'), findsOneWidget);
    },
  );

  testWidgets('missing cached Cookie session is not reported as expired', (
    tester,
  ) async {
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: SingleChildScrollView(
            child: EhSubscriptionPanel(
              storeOverride: _store(),
              btSourcesOverride: [_cookieSource()],
              btTargetSelectionOverride: _targetSelection,
            ),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
    await _openAdvanced(tester);

    await _tapVisible(tester, '检查最新有效种子');
    await tester.pumpAndSettle();

    expect(find.textContaining('没有可复用的已连接 115 Cookie 会话'), findsOneWidget);
    expect(find.textContaining('不会自动刷新或替换会话'), findsNothing);
  });

  testWidgets('target folder browsing is explicit and reaches preflight', (
    tester,
  ) async {
    final source = _cookieSource();
    final parentsRead = <String>[];
    Cloud115BtFolderSelection? preflightTarget;

    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: SingleChildScrollView(
            child: EhSubscriptionPanel(
              storeOverride: _store(),
              btSourcesOverride: [source],
              btBatchPreferencesLoader: () async =>
                  const Cloud115BtBatchSelectionPreferences(),
              btBatchPreferencesSaver: (sourceId, target, mode) async {},
              btFolderListRunner: (source, parentId) async {
                parentsRead.add(parentId);
                if (parentId == '0') {
                  return const [Cloud115BtFolder(id: '30', name: '自定义目标')];
                }
                return const [];
              },
              btPreflightRunner:
                  (source, outputDir, relativeFile, infoHash, target) async {
                    preflightTarget = target;
                    return _preflight(
                      targetFolderId: target.id,
                      targetFolderPath: target.path,
                    );
                  },
            ),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
    await _openAdvanced(tester);

    expect(
      parentsRead,
      isEmpty,
      reason: 'Opening the EH panel must not call 115.',
    );
    await _tapVisible(tester, '选择目录');
    await tester.pumpAndSettle();
    expect(parentsRead, ['0']);
    expect(find.text('选择 115 目标目录'), findsOneWidget);

    await tester.tap(find.text('自定义目标'));
    await tester.pumpAndSettle();
    expect(parentsRead, ['0', '30']);
    await tester.tap(find.text('使用此目录'));
    await tester.pumpAndSettle();
    expect(find.textContaining('/自定义目标（ID 30）'), findsOneWidget);

    await _tapVisible(tester, '检查最新有效种子');
    await tester.pumpAndSettle();
    expect(preflightTarget?.id, '30');
    expect(preflightTarget?.path, '/自定义目标');
  });

  test(
    'BT entry does not reconnect, refresh, or mutate a Cookie source',
    () async {
      const cookie = 'UID=12345_mock; SEID=unchanged';
      final source = _cookieSource(cookie: cookie);

      expect(
        () => cloud115CookieBtUploadAndParseFor(
          source,
          outputDir: r'C:\isolated\eh',
          relativeFile: 'one.torrent',
          expectedInfoHash: '0123456789012345678901234567890123456789',
          target: _targetSelection,
        ),
        throwsA(isA<StateError>()),
      );

      expect(source.cookie, cookie);
    },
  );
}

Future<void> _tapVisible(WidgetTester tester, String label) async {
  final finder = find.text(label);
  expect(finder, findsOneWidget);
  await tester.ensureVisible(finder);
  await tester.tap(finder);
}

Future<void> _openAdvanced(WidgetTester tester) async {
  final tile = find.byKey(const ValueKey('eh-advanced-options'));
  expect(tile, findsOneWidget);
  if (find.byKey(const ValueKey('eh115-bt-batch-run')).evaluate().isNotEmpty) {
    return;
  }
  await tester.ensureVisible(tile);
  await tester.tap(find.text('高级选项'));
  await tester.pumpAndSettle();
}

BookSource _cookieSource({
  String id = 'cookie-source',
  String name = '测试 Cookie 源',
  String cookie = 'fake-cookie-not-used-by-this-test',
}) => BookSource(id: id, type: '115', name: name, cookie: cookie);

const _targetSelection = Cloud115BtFolderSelection(
  id: '12345',
  parentId: '10',
  name: 'EH订阅',
  path: '/漫画/EH订阅',
);

EhSubscriptionStore _store() => EhSubscriptionStore.forTest(
  rules: const {'out_dir': r'C:\isolated\eh'},
  manifestReport: EhManifestReport(
    status: EhManifestReadStatus.ready,
    entries: [_checkedEntry()],
  ),
  torrentDirectoryReport: EhManifestReport(
    status: EhManifestReadStatus.ready,
    entries: [_checkedEntry()],
  ),
);

Cloud115BtPreflight _preflight({
  String targetFolderId = '12345',
  String targetFolderPath = '/漫画/EH订阅',
  String duplicateDetail = '',
}) => Cloud115BtPreflight(
  accountId: 'test-account',
  targetFolderPath: targetFolderPath,
  targetFolderId: targetFolderId,
  torrentName: 'one.torrent',
  torrentSize: BigInt.from(10),
  torrentSha1: 'whole-file-sha1',
  infoHash: '0123456789012345678901234567890123456789',
  files: [
    Cloud115BtFile(index: 0, path: '漫画/第 1 话.cbz', size: BigInt.from(10)),
  ],
  duplicateStatus: 'submit_first',
  duplicateDetail: duplicateDetail,
  writeGate: 'REQUIRES_EXPLICIT_CONFIRMATION',
);

Cloud115BtUploadParseResult _parsedUpload() => Cloud115BtUploadParseResult(
  state: 'PARSED',
  message: '远端文件清单与本地 infohash 匹配',
  accountId: 'test-account',
  targetFolderPath: '/漫画/EH订阅',
  targetFolderId: '12345',
  torrentName: 'one.torrent',
  torrentSize: BigInt.from(10),
  torrentSha1: 'whole-file-sha1',
  infoHash: '0123456789012345678901234567890123456789',
  pickCode: 'pick-code-test',
  uploadReused: false,
  files: [
    Cloud115BtFile(index: 0, path: '漫画/第 1 话.cbz', size: BigInt.from(10)),
  ],
);

Cloud115BtUploadParseResult _unknownUpload() => Cloud115BtUploadParseResult(
  state: 'UNKNOWN_UPLOAD_RESULT',
  message: '上传结果待只读核对',
  accountId: 'test-account',
  targetFolderPath: '/漫画/EH订阅',
  targetFolderId: '12345',
  torrentName: 'one.torrent',
  torrentSize: BigInt.from(10),
  torrentSha1: 'whole-file-sha1',
  infoHash: '0123456789012345678901234567890123456789',
  pickCode: null,
  uploadReused: false,
  files: [],
);

EhCheckedManifestEntry _checkedEntry({
  String file = 'one.torrent',
  String infoHash = '0123456789012345678901234567890123456789',
}) => EhCheckedManifestEntry(
  item: EhSavedItem(
    infohash: infoHash,
    titleJpn: '测试漫画',
    rating: 4.5,
    downloads: 100,
    requiredDl: 50,
    postedUtc: '2026-01-01',
    ageYears: 0,
    category: 'Manga',
    filecount: 1,
    filesize: 10,
    tags: [],
    file: file,
    bytes: 10,
    torrentName: file,
    savedAtUtc: '2026-01-01T00:00:00Z',
  ),
  fileStatus: EhSavedFileStatus.present,
  validationStatus: EhTorrentValidationStatus.valid,
  fullPath: 'C:\\isolated\\eh\\$file',
);

Map<String, Object?> _checkedEntryJson() => {
  'item': {
    'infohash': '0123456789012345678901234567890123456789',
    'title_jpn': '测试漫画',
    'rating': 4.5,
    'downloads': 100,
    'required_dl': 50,
    'posted_utc': '2026-01-01',
    'age_years': 0,
    'category': 'Manga',
    'filecount': 1,
    'filesize': 10,
    'tags': <String>[],
    'file': 'one.torrent',
    'bytes': 10,
    'torrent_name': 'one.torrent',
    'saved_at_utc': '2026-01-01T00:00:00Z',
  },
  'file_status': 'present',
  'validation_status': 'valid',
  'full_path': r'C:\isolated\eh\one.torrent',
};
