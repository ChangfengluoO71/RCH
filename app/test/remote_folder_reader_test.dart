import 'package:app/store/models.dart';
import 'package:app/store/remote_cache_cleanup.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('remote image folder uses a completion lease even in stream mode', () async {
    final source = BookSource(id: 'folder-source', type: 'webdav', name: 'remote');
    final registry = RemoteBookUseRegistry.instance;
    final key = bookKeyOf(source.type, source.id, '/Series/Book');

    final lease = registry.acquire(
      source: source,
      path: '/Series/Book',
      enabled: true,
      strategy: BookOpenStrategy.stream,
      isImageFolder: true,
    );

    expect(lease.enabled, isTrue);
    expect(registry.activeCount(key), 1);
    await lease.release(completionCandidate: false);
    expect(registry.activeCount(key), 0);
  });

  test('stable last page marks completion for a remote folder', () {
    final state = ReadingCompletionState(pageCount: 3);
    state.observeStablePage(2);
    expect(state.completionCandidate, isTrue);
  });
}
