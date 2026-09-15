import 'package:app/store/models.dart';
import 'package:app/store/remote_cache_cleanup.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test(
    'completion candidate requires a stable logical last page and resets on backtrack',
    () {
      final state = ReadingCompletionState(pageCount: 5);
      state.observeStablePage(4);
      expect(state.completionCandidate, isTrue);
      state.observeStablePage(3);
      expect(state.completionCandidate, isFalse);
      state.observeStablePage(4);
      expect(state.completionCandidate, isTrue);
    },
  );

  test(
    'completion from an earlier window is latched until the final lease closes',
    () async {
      final source = BookSource(id: 'account', type: 'webdav', name: 'test');
      var cleanupCalls = 0;
      final registry = RemoteBookUseRegistry(
        cleanup: (source, path, imageFolder) async {
          cleanupCalls += 1;
          return BigInt.from(42);
        },
      );
      final key = bookKeyOf(source.type, source.id, '/book.cbz');
      final first = registry.acquire(
        source: source,
        path: '/book.cbz',
        enabled: true,
        strategy: BookOpenStrategy.download,
      );
      final second = registry.acquire(
        source: source,
        path: '/book.cbz',
        enabled: true,
        strategy: BookOpenStrategy.download,
      );
      expect(registry.activeCount(key), 2);
      expect(await first.release(completionCandidate: true), isNull);
      expect(registry.activeCount(key), 1);
      // The second window did not itself finish, but the first window's stable
      // completion must remain latched until this final lease closes.
      final result = await second.release(completionCandidate: false);
      expect(result, isNotNull);
      expect(result!.succeeded, isTrue);
      expect(cleanupCalls, 1);
      expect(registry.activeCount(key), 0);
    },
  );

  test(
    'failed completion cleanup retries on the next eligible final lease',
    () async {
      final source = BookSource(
        id: 'retry-account',
        type: 'webdav',
        name: 'test',
      );
      var cleanupCalls = 0;
      final registry = RemoteBookUseRegistry(
        cleanup: (source, path, imageFolder) async {
          cleanupCalls += 1;
          if (cleanupCalls == 1) throw StateError('cache is busy');
          return BigInt.from(17);
        },
      );

      final first = registry.acquire(
        source: source,
        path: '/retry.cbz',
        enabled: true,
        strategy: BookOpenStrategy.download,
      );
      final failed = await first.release(completionCandidate: true);
      expect(failed, isNotNull);
      expect(failed!.succeeded, isFalse);
      expect(cleanupCalls, 1);

      final retry = registry.acquire(
        source: source,
        path: '/retry.cbz',
        enabled: true,
        strategy: BookOpenStrategy.download,
      );
      final succeeded = await retry.release(completionCandidate: false);
      expect(succeeded, isNotNull);
      expect(succeeded!.succeeded, isTrue);
      expect(succeeded.freedBytes, BigInt.from(17));
      expect(cleanupCalls, 2);
    },
  );

  test('non-download and local leases never request cleanup', () async {
    final source = BookSource(id: 'local', type: 'local', name: 'local');
    final lease = RemoteBookUseRegistry.instance.acquire(
      source: source,
      path: '/local.cbz',
      enabled: true,
      strategy: BookOpenStrategy.download,
    );
    expect(lease.enabled, isFalse);
    expect(await lease.release(completionCandidate: true), isNull);
  });

  test(
    'auto strategy also honors cleanup because it may materialize raw cache',
    () async {
      final source = BookSource(
        id: 'auto-cleanup',
        type: 'webdav',
        name: 'test',
      );
      final registry = RemoteBookUseRegistry.instance;
      final key = bookKeyOf(source.type, source.id, '/auto.cbz');
      final lease = registry.acquire(
        source: source,
        path: '/auto.cbz',
        enabled: true,
        strategy: BookOpenStrategy.auto,
      );

      expect(lease.enabled, isTrue);
      expect(registry.activeCount(key), 1);
      await lease.release(completionCandidate: false);
      expect(registry.activeCount(key), 0);
    },
  );
}
