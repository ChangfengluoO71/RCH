import 'package:app/src/rust/source/cloud115/offline.dart';
import 'package:app/store/cloud115_session.dart';
import 'package:app/store/eh_subscription_store.dart';
import 'package:app/store/models.dart';
import 'package:app/ui/eh_subscription_panel.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  testWidgets('upload and BT submission each wait for their own confirmation', (
    tester,
  ) async {
    var preflightCalls = 0;
    var uploadCalls = 0;
    var submitCalls = 0;
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
              btPreflightRunner:
                  (source, outputDir, relativeFile, infoHash) async {
                    preflightCalls++;
                    expect(source.id, 'cookie-source');
                    expect(outputDir, r'C:\isolated\eh');
                    expect(relativeFile, 'one.torrent');
                    return _preflight();
                  },
              btUploadRunner:
                  (source, outputDir, relativeFile, infoHash, folderId) async {
                    uploadCalls++;
                    expect(source.id, 'cookie-source');
                    expect(folderId, '12345');
                    return _parsedUpload();
                  },
              btSubmitRunner:
                  (
                    source,
                    outputDir,
                    relativeFile,
                    infoHash,
                    folderId,
                    wanted,
                  ) async {
                    submitCalls++;
                    expect(folderId, '12345');
                    expect(wanted, [0]);
                    return const Cloud115BtSubmitResult(
                      state: 'ACCEPTED',
                      infoHash: '0123456789012345678901234567890123456789',
                      message: '远端任务列表已确认该任务',
                      targetFolderId: '12345',
                    );
                  },
              btTaskStatusRunner: (source, infoHash, folderId) async {
                statusCalls++;
                expect(infoHash, '0123456789012345678901234567890123456789');
                expect(folderId, '12345');
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

    expect(preflightCalls, 0);
    expect(uploadCalls, 0);
    expect(submitCalls, 0);
    expect(statusCalls, 0);
    expect(find.text('115 Cookie BT 实验入口'), findsOneWidget);

    await _tapVisible(tester, '检查账号、目录与重复任务');
    expect(preflightCalls, 1);
    expect(uploadCalls, 0);
    await tester.pumpAndSettle();
    expect(find.textContaining('/漫画/EH订阅（ID 12345）'), findsOneWidget);

    await _tapVisible(tester, '上传 Torrent 并解析');
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
    expect(uploadCalls, 0, reason: 'Dialog must gate the upload request.');
    await tester.tap(find.text('取消').last);
    await tester.pumpAndSettle();
    expect(uploadCalls, 0);

    await _tapVisible(tester, '上传 Torrent 并解析');
    await tester.pumpAndSettle();
    await tester.tap(find.text('确认上传').last);
    await tester.pumpAndSettle();
    expect(uploadCalls, 1);
    expect(find.text('远端 Torrent 已解析'), findsOneWidget);
    expect(find.text('提交 BT 任务（一次）'), findsOneWidget);
    expect(submitCalls, 0);

    await _tapVisible(tester, '提交 BT 任务（一次）');
    await tester.pumpAndSettle();
    expect(find.byType(AlertDialog), findsOneWidget);
    expect(
      tester
          .widgetList<CheckboxListTile>(find.byType(CheckboxListTile))
          .every((field) => field.onChanged == null),
      isTrue,
      reason: 'wanted indices stay fixed while confirming submission.',
    );
    expect(submitCalls, 0, reason: 'BT submission has a separate dialog.');
    await tester.tap(find.text('取消').last);
    await tester.pumpAndSettle();
    expect(submitCalls, 0);

    await _tapVisible(tester, '提交 BT 任务（一次）');
    await tester.pumpAndSettle();
    await tester.tap(find.text('确认提交一次').last);
    await tester.pumpAndSettle();
    expect(submitCalls, 1);
    expect(find.textContaining('ACCEPTED'), findsOneWidget);

    await _tapVisible(tester, '查询 BT 任务');
    await tester.pumpAndSettle();
    expect(statusCalls, 1);
    expect(find.textContaining('downloading'), findsOneWidget);
    expect(find.textContaining('25%'), findsOneWidget);
    expect(
      submitCalls,
      1,
      reason: 'Read-only status checks must not resubmit.',
    );
  });

  testWidgets('unknown upload outcome exposes only read-only recovery', (
    tester,
  ) async {
    var uploadCalls = 0;
    var recoveryCalls = 0;
    final source = _cookieSource();
    final sources = [source];
    final store = _store();

    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: SingleChildScrollView(
            child: EhSubscriptionPanel(
              storeOverride: store,
              btSourcesOverride: sources,
              btPreflightRunner:
                  (source, outputDir, relativeFile, infoHash) async =>
                      _preflight(),
              btUploadRunner:
                  (source, outputDir, relativeFile, infoHash, folderId) async {
                    uploadCalls++;
                    return _unknownUpload();
                  },
              btUploadRecoveryRunner:
                  (
                    source,
                    outputDir,
                    relativeFile,
                    infoHash,
                    folderId,
                    pickCode,
                  ) async {
                    recoveryCalls++;
                    expect(pickCode, isNull);
                    return _unknownUpload();
                  },
            ),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();

    await _tapVisible(tester, '检查账号、目录与重复任务');
    await tester.pumpAndSettle();
    await _tapVisible(tester, '上传 Torrent 并解析');
    await tester.pumpAndSettle();
    await tester.tap(find.text('确认上传').last);
    await tester.pumpAndSettle();

    expect(uploadCalls, 1);
    expect(find.text('核对 Torrent 上传结果'), findsOneWidget);
    expect(find.text('上传 Torrent 并解析'), findsNothing);
    expect(find.text('提交 BT 任务（一次）'), findsNothing);

    await _tapVisible(tester, '核对 Torrent 上传结果');
    await tester.pumpAndSettle();
    expect(recoveryCalls, 1);
    expect(uploadCalls, 1, reason: 'Recovery must never repeat upload.');
    expect(find.textContaining('UNKNOWN_UPLOAD_RESULT'), findsOneWidget);

    sources.clear();
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: SingleChildScrollView(
            child: EhSubscriptionPanel(
              storeOverride: store,
              btSourcesOverride: sources,
            ),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
    expect(find.text('核对 Torrent 上传结果'), findsNothing);
    final queryButton = tester.widget<OutlinedButton>(
      find.widgetWithText(OutlinedButton, '查询 BT 任务'),
    );
    expect(queryButton.onPressed, isNull);
  });

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
            ),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();

    await _tapVisible(tester, '检查账号、目录与重复任务');
    await tester.pumpAndSettle();

    expect(find.textContaining('没有可复用的已连接 115 Cookie 会话'), findsOneWidget);
    expect(find.textContaining('不会自动刷新或替换会话'), findsNothing);
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
          expectedTargetFolderId: '12345',
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

BookSource _cookieSource({
  String cookie = 'fake-cookie-not-used-by-this-test',
}) => BookSource(
  id: 'cookie-source',
  type: '115',
  name: '测试 Cookie 源',
  cookie: cookie,
);

EhSubscriptionStore _store() => EhSubscriptionStore.forTest(
  rules: const {'out_dir': r'C:\isolated\eh'},
  manifestReport: EhManifestReport(
    status: EhManifestReadStatus.ready,
    entries: [_checkedEntry()],
  ),
);

Cloud115BtPreflight _preflight() => Cloud115BtPreflight(
  accountId: 'test-account',
  targetFolderPath: '/漫画/EH订阅',
  targetFolderId: '12345',
  torrentName: 'one.torrent',
  torrentSize: BigInt.from(10),
  torrentSha1: 'whole-file-sha1',
  infoHash: '0123456789012345678901234567890123456789',
  files: [
    Cloud115BtFile(index: 0, path: '漫画/第 1 话.cbz', size: BigInt.from(10)),
  ],
  duplicateStatus: 'not_found',
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

EhCheckedManifestEntry _checkedEntry() => EhCheckedManifestEntry(
  item: const EhSavedItem(
    infohash: '0123456789012345678901234567890123456789',
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
    file: 'one.torrent',
    bytes: 10,
    torrentName: 'one.torrent',
    savedAtUtc: '2026-01-01T00:00:00Z',
  ),
  fileStatus: EhSavedFileStatus.present,
  validationStatus: EhTorrentValidationStatus.valid,
  fullPath: r'C:\isolated\eh\one.torrent',
);
