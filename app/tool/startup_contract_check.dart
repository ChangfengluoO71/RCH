import 'dart:io';

Never _fail(String message) {
  stderr.writeln(message);
  exit(1);
}

void main() {
  final source = File('lib/main.dart').readAsStringSync();
  final runAppIndex = source.indexOf('runApp(');
  if (runAppIndex < 0) _fail('runApp not found');

  for (final marker in <String>[
    'FolderSnapshotStore.instance.load()',
    'LibraryCatalogStore.instance.loadTree()',
    'SyncManager.instance.init()',
    'AutomationCoordinator.instance.init()',
    'AiUpscaleManager.instance.init()',
  ]) {
    final index = source.indexOf(marker);
    if (index <= runAppIndex) {
      _fail('$marker blocks first frame');
    }
  }

  if (!source.contains('LibraryStore.instance.load(persist: false)')) {
    _fail('LibraryStore startup load must use persist: false');
  }
  if (!source.contains('addPostFrameCallback')) {
    _fail('post-frame scheduling is missing');
  }

  stdout.writeln('startup contract satisfied');
}
