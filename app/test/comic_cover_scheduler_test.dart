import 'dart:async';

import 'package:app/store/models.dart';
import 'package:app/ui/comic_cover.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('two subscribers for one logical key run one shared load', () async {
    final scheduler = VisibleCoverScheduler<String>(maxConcurrent: 1);
    final result = Completer<String>();
    var loads = 0;

    final first = scheduler.acquire('webdav|source|book.cbz|0|small|none', () {
      loads++;
      return result.future;
    });
    final second = scheduler.acquire(
      'webdav|source|book.cbz|0|small|none',
      () => Future.value('unexpected'),
    );

    expect(loads, 1);
    result.complete('cover');
    await expectLater(first.future, completion('cover'));
    await expectLater(second.future, completion('cover'));

    first.dispose();
    second.dispose();
  });

  test(
    'disposing the only pending subscriber removes its queued load',
    () async {
      final scheduler = VisibleCoverScheduler<String>(maxConcurrent: 1);
      final blocker = Completer<String>();
      var pendingLoads = 0;

      final running = scheduler.acquire('running', () => blocker.future);
      final pending = scheduler.acquire('pending', () {
        pendingLoads++;
        return Future.value('pending cover');
      });
      final pendingFailure = expectLater(pending.future, throwsStateError);

      pending.dispose();
      blocker.complete('running cover');
      await running.future;
      await pendingFailure;
      await Future<void>.delayed(Duration.zero);

      expect(pendingLoads, 0);
      running.dispose();
    },
  );

  test(
    'disposing one of two pending subscribers keeps their shared load',
    () async {
      final scheduler = VisibleCoverScheduler<String>(maxConcurrent: 1);
      final blocker = Completer<String>();
      var loads = 0;

      final running = scheduler.acquire('running', () => blocker.future);
      final first = scheduler.acquire('shared', () {
        loads++;
        return Future.value('cover');
      });
      final second = scheduler.acquire(
        'shared',
        () => Future.value('unexpected'),
      );

      first.dispose();
      blocker.complete('running cover');
      await running.future;
      await expectLater(second.future, completion('cover'));

      expect(loads, 1);
      running.dispose();
      second.dispose();
    },
  );

  test(
    'disabled remote fetching never asks the session factory to reconnect',
    () async {
      var sessionFactoryCalls = 0;
      var coverCalls = 0;

      final result = await loadCoverWithSafePolicy<String>(
        remoteFetchEnabled: false,
        liveSession: null,
        createSession: () async {
          sessionFactoryCalls++;
          return BigInt.one;
        },
        load: (_, _) async {
          coverCalls++;
          return 'cover';
        },
      );

      expect(result, isNull);
      expect(sessionFactoryCalls, 0);
      expect(coverCalls, 0);
    },
  );

  test('remote cover network gate only suppresses session-backed sources', () {
    final remote = BookSource(id: 'r', type: 'webdav', name: 'remote');
    final local = BookSource(id: 'l', type: 'local', name: 'local');

    expect(
      shouldSkipRemoteCoverNetwork(
        source: remote,
        remoteCoverFetchEnabled: false,
      ),
      isTrue,
    );
    expect(
      shouldSkipRemoteCoverNetwork(
        source: remote,
        remoteCoverFetchEnabled: true,
      ),
      isFalse,
    );
    expect(
      shouldSkipRemoteCoverNetwork(
        source: local,
        remoteCoverFetchEnabled: false,
      ),
      isFalse,
    );
  });

  test('range unsupported notices are once per source browser session', () {
    final notices = SourceCoverNoticeGate();

    expect(notices.take('webdav|first'), isTrue);
    expect(notices.take('webdav|first'), isFalse);
    expect(notices.take('sftp|second'), isTrue);
  });
}
