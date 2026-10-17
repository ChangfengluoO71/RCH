import 'dart:convert';
import 'dart:io';

import 'package:app/store/eh_subscription_store.dart';
import 'package:app/ui/eh_subscription_panel.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

Map<String, dynamic> _checkedManifest({
  String status = 'ready',
  String fileStatus = 'present',
  String validationStatus = 'valid',
}) => {
  'status': status,
  'invalid_entries': 0,
  'error_code': null,
  'entries': [
    {
      'item': {
        'infohash': '54ada1b6bfdf4e7792774b033f0026c616dcd0cb',
        'gid': '1',
        'title': 'title',
        'title_jpn': '中文标题 & 特殊字符',
        'rating': 4.5,
        'downloads': 10,
        'required_dl': 0,
        'posted': null,
        'posted_utc': '2026-10-11',
        'age_years': null,
        'tags': <String>[],
        'category': 'Doujinshi',
        'filecount': 1,
        'filesize': 123,
        'uploader': 'uploader',
        'torrent_name': 'torrent',
        'file': '中文 #1/漫画 & [测试].torrent',
        'bytes': 122,
        'source_url': 'https://example.invalid/',
        'saved_at_utc': '2026-10-11T00:00:00Z',
        'semantic': <String, dynamic>{},
      },
      'file_status': fileStatus,
      'validation_status': validationStatus,
      'full_path': r'D:\EhTorrents\中文 #1\漫画 & [测试].torrent',
    },
  ],
};

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  test('checked manifest decoder keeps local evidence states typed', () {
    final report = EhManifestReport.fromJson(_checkedManifest());
    expect(report.status, EhManifestReadStatus.ready);
    expect(report.entries.single.item.savedAtUtc, '2026-10-11T00:00:00Z');
    expect(report.entries.single.fileStatus, EhSavedFileStatus.present);
    expect(
      report.entries.single.validationStatus,
      EhTorrentValidationStatus.valid,
    );
    expect(report.entries.single.fullPath, contains('漫画 & [测试].torrent'));
  });

  test('folder inventory is loaded independently from the manifest', () async {
    var manifestReads = 0;
    var directoryReads = 0;
    final directory = _checkedManifest();
    final entries = directory['entries'] as List;
    final entry = entries.single as Map<String, dynamic>;
    final item = entry['item'] as Map<String, dynamic>;
    item['file'] = 'orphan.torrent';
    item['torrent_name'] = 'orphan.torrent';
    entry['full_path'] = r'D:\EhTorrents\orphan.torrent';

    final store = EhSubscriptionStore.forTest(
      rules: {'out_dir': r'D:\EhTorrents'},
      checkedManifestLoader: (_) async {
        manifestReads++;
        throw StateError('manifest must not be read for disk inventory');
      },
      torrentDirectoryLoader: (outDir) async {
        directoryReads++;
        expect(outDir, r'D:\EhTorrents');
        return jsonEncode(directory);
      },
    );

    await store.refreshTorrentDirectory();

    expect(directoryReads, 1);
    expect(manifestReads, 0);
    expect(
      store.torrentDirectoryReport.entries.single.item.file,
      'orphan.torrent',
    );
    expect(store.manifestReport.status, EhManifestReadStatus.empty);
    expect(store.manifestReport.entries, isEmpty);
  });

  test(
    'non-Windows folder open safely declines without launching a process',
    () async {
      var processCalls = 0;
      final opened = await openEhOutputDirectory(
        r'D:\EhTorrents',
        isWindows: false,
        runProcess: (executable, arguments) async {
          processCalls += 1;
          throw StateError('must not run off Windows');
        },
      );
      expect(opened, isFalse);
      expect(processCalls, 0);
    },
  );

  test(
    'Windows folder open launches Explorer only for the configured folder',
    () async {
      String? executable;
      List<String>? arguments;
      final opened = await openEhOutputDirectory(
        r'D:\EH output',
        isWindows: true,
        runProcess: (value, args) async {
          executable = value;
          arguments = args;
          return ProcessResult(0, 0, '', '');
        },
      );
      expect(opened, isTrue);
      expect(executable, 'explorer');
      expect(arguments, [r'D:\EH output']);
    },
  );

  testWidgets('refresh reads the local manifest without starting an EH scan', (
    tester,
  ) async {
    var manifestReads = 0;
    var scanRequests = 0;
    final store = EhSubscriptionStore.forTest(
      rules: {'out_dir': r'D:\EhTorrents'},
      checkedManifestLoader: (outDir) async {
        manifestReads += 1;
        expect(outDir, r'D:\EhTorrents');
        return jsonEncode(_checkedManifest());
      },
      onScanRequested: () => scanRequests += 1,
    );

    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: SingleChildScrollView(
            child: EhSubscriptionPanel(storeOverride: store),
          ),
        ),
      ),
    );
    expect(
      manifestReads,
      0,
      reason: 'building the panel must not read or scan',
    );
    expect(scanRequests, 0);

    await tester.ensureVisible(
      find.byKey(const ValueKey('eh-manifest-refresh')),
    );
    await tester.tap(find.byKey(const ValueKey('eh-manifest-refresh')));
    await tester.pumpAndSettle();

    expect(manifestReads, 1);
    expect(scanRequests, 0);
    expect(store.progress.stage, 'idle');
    expect(find.text('中文标题 & 特殊字符'), findsOneWidget);
    expect(
      find.textContaining('54ada1b6bfdf4e7792774b033f0026c616dcd0cb'),
      findsOneWidget,
    );
    expect(find.textContaining('校验通过'), findsOneWidget);
  });

  testWidgets('handoff removes path-copy and manual-submit controls', (
    tester,
  ) async {
    final store = EhSubscriptionStore.forTest(
      rules: {'out_dir': r'D:\EhTorrents'},
      manifestReport: EhManifestReport.fromJson(_checkedManifest()),
    );

    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: SingleChildScrollView(
            child: EhSubscriptionPanel(storeOverride: store),
          ),
        ),
      ),
    );
    expect(find.byKey(const ValueKey('eh-handoff-copy-path')), findsNothing);
    expect(find.byKey(const ValueKey('eh-handoff-instructions')), findsNothing);
    expect(find.textContaining('非 115 云下载'), findsOneWidget);
    expect(find.textContaining('本地清单状态不代表已上传'), findsOneWidget);
  });

  testWidgets('saved Torrent list defaults to the newest entry only', (
    tester,
  ) async {
    final manifest = _checkedManifest();
    final entries = manifest['entries'] as List;
    final older =
        jsonDecode(jsonEncode(entries.single)) as Map<String, dynamic>;
    final olderItem = older['item'] as Map<String, dynamic>;
    olderItem['title_jpn'] = 'older Torrent';
    olderItem['file'] = 'older.torrent';
    olderItem['infohash'] = 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa';
    manifest['entries'] = [older, entries.single];
    final store = EhSubscriptionStore.forTest(
      rules: {'out_dir': r'D:\EhTorrents'},
      manifestReport: EhManifestReport.fromJson(manifest),
    );

    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: SingleChildScrollView(
            child: EhSubscriptionPanel(storeOverride: store),
          ),
        ),
      ),
    );

    expect(find.byKey(const ValueKey('eh-handoff-select-0')), findsOneWidget);
    expect(find.byKey(const ValueKey('eh-handoff-select-1')), findsNothing);
    expect(find.byKey(const ValueKey('eh-handoff-toggle-all')), findsNothing);
    expect(find.text('展开全部'), findsNothing);
  });

  testWidgets('initializing the handoff panel does not call a 115 client', (
    tester,
  ) async {
    var localManifestReads = 0;
    final store = EhSubscriptionStore.forTest(
      rules: {'out_dir': r'D:\EhTorrents'},
      checkedManifestLoader: (outDir) async {
        localManifestReads += 1;
        return jsonEncode({
          'status': 'missing',
          'entries': [],
          'invalid_entries': 0,
        });
      },
    );
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: SingleChildScrollView(
            child: EhSubscriptionPanel(storeOverride: store),
          ),
        ),
      ),
    );
    expect(localManifestReads, 0);
    expect(store.progress.stage, 'idle');
  });
}
