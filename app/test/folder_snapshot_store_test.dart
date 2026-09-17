import 'dart:io';

import 'package:app/src/rust/api/cache.dart';
import 'package:app/src/rust/frb_generated.dart';
import 'package:app/store/folder_snapshot_store.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  setUpAll(() async => RustLib.init());

  test(
    'clear drops snapshot file so lifecycle flush cannot resurrect it',
    () async {
      final root = Directory.systemTemp.createTempSync('rch-folder-snapshot-');
      addTearDown(() async {
        await setCacheRootPath(path: '');
        if (await root.exists()) await root.delete(recursive: true);
      });
      await setCacheRootPath(path: root.path);
      final file = File(
        '${root.path}${Platform.pathSeparator}folder_snapshots.json',
      );
      await file.writeAsString('{"version":1,"folders":[]}');

      await FolderSnapshotStore.instance.clear();

      expect(await file.exists(), isFalse);
      await FolderSnapshotStore.instance.flush();
      expect(await file.exists(), isFalse);
    },
  );
}
