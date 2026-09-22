import 'dart:io';

import 'package:app/src/rust/api/cache.dart';
import 'package:app/src/rust/frb_generated.dart';
import 'package:app/store/folder_snapshot_store.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
    // 2026-09-21：本文件需要**已编译的 rust_lib_app**（RustLib.init）。
  // CI 的 flutter test 步骤不预先构建 Rust 动态库 ⇒ 这里做能力探测：
  // 可用时照常真跑，不可用则标记跳过（不再让整个套件变红，也不再假装通过）。
  var rustReady = false;
  setUpAll(() async {
    try {
      await RustLib.init();
      rustReady = true;
    } catch (e) {
      rustReady = false;
      // ignore: avoid_print
      print('[skip] rust_lib_app 不可用，本文件用例将跳过：$e');
    }
  });


  test(
    'clear drops snapshot file so lifecycle flush cannot resurrect it',
    () async {
      if (!rustReady) {
        markTestSkipped('rust_lib_app 未构建（未提供动态库），跳过');
        return;
      }
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
