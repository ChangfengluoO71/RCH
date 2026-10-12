// Run only with:
//   flutter run -d windows --target test/eh_subscription_handoff_gui.dart
// This standalone Debug entry never imports or invokes the production main().
import 'dart:convert';
import 'dart:io';

import 'package:app/store/eh_subscription_store.dart';
import 'package:app/ui/eh_subscription_panel.dart';
import 'package:flutter/material.dart';

const _infoHash = '54ada1b6bfdf4e7792774b033f0026c616dcd0cb';
const _relativeTorrentPath =
    '\u{4e2d}\u{6587} #1 & [\u{7279}\u{6b8a}\u{5b57}\u{7b26}]/'
    '\u{6f2b}\u{753b} #1 [\u{9a8c}\u{6536}] & (\u{6837}\u{4f8b}).torrent';
const _torrent =
    'd8:announce23:http://tracker/announce4:infod6:lengthi123e4:name4:test12:piece lengthi16384e6:pieces20:aaaaaaaaaaaaaaaaaaaaee';

Future<void> main() async {
  if (const bool.fromEnvironment('dart.vm.product') ||
      const bool.fromEnvironment('dart.vm.profile')) {
    throw StateError('The H0 handoff GUI entry is Debug/test only.');
  }
  WidgetsFlutterBinding.ensureInitialized();

  final rawTextScale = Platform.environment['RCH_H0_GUI_TEXT_SCALE'];
  final textScale = rawTextScale == null ? null : double.tryParse(rawTextScale);
  if (rawTextScale != null &&
      (textScale == null || textScale < 1.0 || textScale > 2.0)) {
    throw StateError('RCH_H0_GUI_TEXT_SCALE must be between 1.0 and 2.0.');
  }
  final startAtHandoff =
      Platform.environment['RCH_H0_GUI_START_AT_HANDOFF'] == '1';

  final root = await Directory.systemTemp.createTemp('RCH H0 GUI Acceptance ');
  final fixture = File(
    '${root.path}${Platform.pathSeparator}$_relativeTorrentPath',
  );
  await fixture.parent.create(recursive: true);
  await fixture.writeAsBytes(utf8.encode(_torrent), flush: true);

  final manifest = File('${root.path}${Platform.pathSeparator}manifest.json');
  await _writeValidManifest(manifest);

  var scanRequests = 0;
  final store = EhSubscriptionStore.forTest(
    rules: {
      'search': 'local GUI fixture only',
      'title_markers': 'GUI fixture',
      'out_dir': root.path,
      'pages': 1,
      'host': 'e-hentai.org',
    },
    checkedManifestLoader: _loadInjectedManifest,
    onScanRequested: () => scanRequests += 1,
  );

  await store.refreshManifest();
  const status =
      'Controlled local fixture injection; no EH scan or 115 API; '
      'scan requests remain zero.';

  stdout.writeln('H0_GUI_TEST_ENTRY=app/test/eh_subscription_handoff_gui.dart');
  stdout.writeln('H0_GUI_TEMP_ROOT=${root.path}');
  stdout.writeln('H0_GUI_TORRENT=${fixture.path}');
  stdout.writeln('H0_GUI_MANIFEST=${manifest.path}');
  stdout.writeln('H0_GUI_INITIAL_STATUS=${store.manifestReport.status.name}');
  stdout.writeln(
    'H0_GUI_INITIAL_ENTRIES=${store.manifestReport.entries.length}',
  );
  stdout.writeln('H0_GUI_SCAN_REQUESTS=$scanRequests');
  stdout.writeln('H0_GUI_TEXT_SCALE=${textScale ?? 'system'}');
  stdout.writeln('H0_GUI_START_AT_HANDOFF=$startAtHandoff');
  stdout.writeln(
    'H0_GUI_READER=controlled fixture injection; no network or 115 API',
  );

  final scrollController = ScrollController();
  runApp(
    _HandoffGuiApp(
      root: root.path,
      fixturePath: fixture.path,
      store: store,
      status: status,
      scanRequests: () => scanRequests,
      textScale: textScale,
      scrollController: scrollController,
    ),
  );
  WidgetsBinding.instance.addPostFrameCallback((_) {
    if (startAtHandoff && scrollController.hasClients) {
      scrollController.jumpTo(scrollController.position.maxScrollExtent);
    }
  });
}

Future<String> _loadInjectedManifest(String outDir) async {
  final root = Directory(outDir).absolute;
  final manifest = File('${root.path}${Platform.pathSeparator}manifest.json');
  if (!await manifest.exists()) {
    return jsonEncode({'status': 'missing', 'entries': <Object>[]});
  }

  late final List<Object?> rows;
  try {
    final decoded = jsonDecode(await manifest.readAsString());
    if (decoded is! List) {
      return jsonEncode({'status': 'damaged', 'entries': <Object>[]});
    }
    rows = decoded;
  } on FormatException {
    return jsonEncode({'status': 'damaged', 'entries': <Object>[]});
  } on FileSystemException {
    return jsonEncode({'status': 'unreadable', 'entries': <Object>[]});
  }

  final entries = <Map<String, Object?>>[];
  var invalidEntries = 0;
  for (final raw in rows) {
    if (raw is! Map) {
      invalidEntries += 1;
      continue;
    }
    final item = Map<String, dynamic>.from(raw);
    final relativePath = item['file'] as String? ?? '';
    if (relativePath != _relativeTorrentPath) {
      invalidEntries += 1;
      continue;
    }

    final file = File(
      '${root.path}${Platform.pathSeparator}$_relativeTorrentPath',
    );
    final present = await file.exists();
    final bytes = present ? await file.readAsBytes() : const <int>[];
    final expected = utf8.encode(_torrent);
    var fixtureMatches = present && bytes.length == expected.length;
    for (var index = 0; fixtureMatches && index < bytes.length; index++) {
      fixtureMatches = bytes[index] == expected[index];
    }
    entries.add({
      'item': item,
      'file_status': present ? 'present' : 'missing',
      'validation_status': !present
          ? 'not_checked'
          : fixtureMatches && item['infohash'] == _infoHash
          ? 'valid'
          : 'invalid_torrent',
      'full_path': file.absolute.path,
    });
  }

  return jsonEncode({
    'status': invalidEntries > 0
        ? 'damaged'
        : rows.isEmpty
        ? 'empty'
        : 'ready',
    'entries': entries,
    'invalid_entries': invalidEntries,
    'error_code': invalidEntries > 0 ? 'test_fixture_entry_invalid' : null,
  });
}

Future<void> _writeValidManifest(File manifest) async {
  await manifest.writeAsString(
    jsonEncode([
      {
        'infohash': _infoHash,
        'gid': 'gui-fixture',
        'title': 'H0 GUI fixture',
        'title_jpn':
            '\u{4e2d}\u{6587}\u{6807}\u{9898} & '
            '\u{7279}\u{6b8a}\u{5b57}\u{7b26} [H0 GUI]',
        'rating': 4.5,
        'downloads': 1,
        'required_dl': 0,
        'posted': null,
        'posted_utc': '2026-10-11',
        'age_years': 0.0,
        'tags': <String>['test:local-only'],
        'category': 'Doujinshi',
        'filecount': 1,
        'filesize': 123,
        'uploader': 'H0 local fixture',
        'torrent_name': 'GUI local fixture',
        'file': _relativeTorrentPath,
        'bytes': utf8.encode(_torrent).length,
        'source_url': 'https://example.invalid/local-test-fixture',
        'saved_at_utc': '2026-10-11T00:00:00Z',
        'semantic': <String, Object?>{},
      },
    ]),
    flush: true,
  );
}

class _HandoffGuiApp extends StatelessWidget {
  const _HandoffGuiApp({
    required this.root,
    required this.fixturePath,
    required this.store,
    required this.status,
    required this.scanRequests,
    required this.textScale,
    required this.scrollController,
  });

  final String root;
  final String fixturePath;
  final EhSubscriptionStore store;
  final String status;
  final int Function() scanRequests;
  final double? textScale;
  final ScrollController scrollController;

  @override
  Widget build(BuildContext context) => MaterialApp(
    title: 'RCH H0 GUI Acceptance (isolated)',
    theme: ThemeData(
      useMaterial3: true,
      colorSchemeSeed: const Color(0xFF6750A4),
    ),
    builder: (context, child) {
      final content = child ?? const SizedBox.shrink();
      final scale = textScale;
      if (scale == null) return content;
      return MediaQuery(
        data: MediaQuery.of(
          context,
        ).copyWith(textScaler: TextScaler.linear(scale)),
        child: content,
      );
    },
    home: Scaffold(
      appBar: AppBar(title: const Text('H0 GUI Acceptance (isolated)')),
      body: SafeArea(
        child: Column(
          children: [
            Padding(
              padding: const EdgeInsets.fromLTRB(16, 8, 16, 8),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text('Test directory: $root'),
                  Text('Test Torrent: $fixturePath'),
                  Text(
                    'Local fixture injection; scan requests: ${scanRequests()}',
                  ),
                  Text(status),
                ],
              ),
            ),
            const Divider(height: 1),
            Expanded(
              child: SingleChildScrollView(
                controller: scrollController,
                padding: const EdgeInsets.all(16),
                child: EhSubscriptionPanel(storeOverride: store),
              ),
            ),
          ],
        ),
      ),
    ),
  );
}
