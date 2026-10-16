import 'dart:convert';
import 'dart:io';

import 'package:flutter_rust_bridge/flutter_rust_bridge_for_generated.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:app/src/rust/api/eh_subscription.dart';
import 'package:app/src/rust/frb_generated.dart';

const _torrent =
    'd8:announce23:http://tracker/announce4:infod6:lengthi123e4:name4:test12:piece lengthi16384e6:pieces20:aaaaaaaaaaaaaaaaaaaaee';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  test('real Rust FFI reads valid and damaged isolated manifests', () async {
    final dll = File('rust/target/debug/rust_lib_app.dll').absolute;
    expect(
      dll.existsSync(),
      isTrue,
      reason: 'Build the current workspace DLL first.',
    );

    final root = await Directory.systemTemp.createTemp('rch-r0-manifest-ffi-');
    try {
      final torrent = File('${root.path}${Platform.pathSeparator}ok.torrent');
      await torrent.writeAsString(_torrent, flush: true);
      final manifest = File(
        '${root.path}${Platform.pathSeparator}manifest.json',
      );

      await RustLib.init(externalLibrary: ExternalLibrary.open(dll.path));

      final directoryInventory = jsonDecode(
        await ehTorrentsChecked(outDir: root.path),
      );
      expect(directoryInventory['status'], 'ready');
      expect(directoryInventory['entries'], hasLength(1));
      expect(directoryInventory['entries'][0]['item']['file'], 'ok.torrent');
      expect(directoryInventory['entries'][0]['validation_status'], 'valid');
      expect(
        File('${root.path}${Platform.pathSeparator}manifest.json').existsSync(),
        isFalse,
        reason: 'Directory inventory must not depend on manifest.json.',
      );
      stdout.writeln('H0_FFI_DIRECTORY_TORRENTS=1');
      stdout.writeln('H0_FFI_DIRECTORY_STATUS=${directoryInventory['status']}');

      await manifest.writeAsString(
        jsonEncode([
          {
            'infohash': '54ada1b6bfdf4e7792774b033f0026c616dcd0cb',
            'gid': 'ffi-fixture',
            'title': 'R0 FFI fixture',
            'title_jpn': '中文 & FFI fixture',
            'rating': 4.5,
            'downloads': 1,
            'required_dl': 0,
            'posted': null,
            'posted_utc': '2026-10-11',
            'age_years': null,
            'tags': <String>['test:local-only'],
            'category': 'Doujinshi',
            'filecount': 1,
            'filesize': 123,
            'uploader': 'local fixture',
            'torrent_name': 'local fixture',
            'file': 'ok.torrent',
            'bytes': utf8.encode(_torrent).length,
            'source_url': 'https://example.invalid/ffi-fixture',
            'saved_at_utc': '2026-10-11T00:00:00Z',
          },
        ]),
        flush: true,
      );

      final valid = jsonDecode(await ehManifestChecked(outDir: root.path));
      expect(valid['status'], 'ready');
      expect(valid['entries'], hasLength(1));
      expect(valid['entries'][0]['validation_status'], 'valid');
      stdout.writeln('H0_FFI_VALID_STATUS=${valid['status']}');
      stdout.writeln(
        'H0_FFI_VALID_TORRENT=${valid['entries'][0]['validation_status']}',
      );

      await manifest.writeAsString('{broken json', flush: true);
      final damaged = jsonDecode(await ehManifestChecked(outDir: root.path));
      expect(damaged['status'], 'damaged');
      expect(damaged['error_code'], 'manifest_json_invalid');
      stdout.writeln('H0_FFI_DAMAGED_STATUS=${damaged['status']}');
      stdout.writeln('H0_FFI_DAMAGED_CODE=${damaged['error_code']}');
      stdout.writeln('H0_FFI_DLL=${dll.path}');
    } finally {
      if (RustLib.instance.initialized) RustLib.dispose();
      await root.delete(recursive: true);
    }
  });
}
