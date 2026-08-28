import 'dart:io';

import 'package:flutter_test/flutter_test.dart';

void main() {
  test('non-critical startup work is scheduled after runApp', () {
    final source = File('lib/main.dart').readAsStringSync();
    final runAppIndex = source.indexOf('runApp(');
    expect(runAppIndex, greaterThanOrEqualTo(0));

    for (final marker in <String>[
      'FolderSnapshotStore.instance.load()',
      'LibraryCatalogStore.instance.loadTree()',
      'SyncManager.instance.init()',
      'AutomationCoordinator.instance.init()',
      'AiUpscaleManager.instance.init()',
    ]) {
      final index = source.indexOf(marker);
      expect(index, greaterThan(runAppIndex), reason: '$marker must not block first frame');
    }

    expect(source, contains('LibraryStore.instance.load(persist: false)'));
    expect(source, contains('addPostFrameCallback'));
  });
}
