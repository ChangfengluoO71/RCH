import 'dart:async';

import 'package:app/store/models.dart';
import 'package:app/store/remote_scan_coordinator.dart';
import 'package:app/store/remote_scan_models.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test(
    'session success events are delivered after the current call stack',
    () async {
      final hub = RemoteSessionSuccessHub();
      final source = BookSource(
        id: 'async-hub',
        type: '115',
        name: '115',
        path: '/',
      );
      var delivered = false;
      final subscription = hub.events.listen((_) => delivered = true);

      hub.emit(source, BigInt.one);
      expect(delivered, isFalse);
      await Future<void>.delayed(Duration.zero);
      expect(delivered, isTrue);

      await subscription.cancel();
      await hub.dispose();
    },
  );

  test('session triggers coalesce and later trigger is incremental', () async {
    final first = Completer<RemoteScanStatus>();
    final modes = <String>[];
    final coordinator = RemoteScanCoordinator(
      debounce: Duration.zero,
      start:
          ({
            required source,
            required session,
            required rootPath,
            required mode,
          }) {
            modes.add(mode);
            return first.future;
          },
    );
    final source = BookSource(
      id: 'remote-1',
      type: 'webdav',
      name: 'Cloud',
      path: '/',
    );

    final a = coordinator.ensureForSession(source, BigInt.one);
    final b = coordinator.ensureForSession(source, BigInt.one);
    expect(identical(a, b), isTrue);
    expect(modes, ['full']);
    first.complete(
      const RemoteScanStatus(
        sourceId: 'remote-1',
        status: 'complete',
        mode: 'full',
        generation: 1,
      ),
    );
    await a;

    await coordinator.ensureForSession(source, BigInt.one);
    expect(modes, ['full', 'incremental']);
  });

  test(
    'running scan progress is published while native job is pending',
    () async {
      final pending = Completer<RemoteScanStatus>();
      final coordinator = RemoteScanCoordinator(
        progressPollInterval: const Duration(milliseconds: 1),
        statusCall: (_) async => const RemoteScanStatus(
          sourceId: 'progress',
          status: 'running',
          mode: 'full',
          generation: 1,
          processed: 2,
          total: 5,
        ),
        start:
            ({
              required source,
              required session,
              required rootPath,
              required mode,
            }) => pending.future,
      );
      final source = BookSource(
        id: 'progress',
        type: '115',
        name: '115',
        path: '/',
      );

      final scan = coordinator.rescanFull(source, BigInt.one);
      await Future<void>.delayed(const Duration(milliseconds: 10));
      final view = coordinator.viewStateFor(source.id).value;
      expect(view?.status, 'running');
      expect(view?.processed, 2);
      expect(view?.total, 5);

      pending.complete(
        const RemoteScanStatus(
          sourceId: 'progress',
          status: 'complete',
          mode: 'full',
          generation: 1,
          processed: 5,
          total: 5,
        ),
      );
      await scan;
      await coordinator.dispose();
    },
  );

  test(
    'disposing coordinator stops progress polling for a pending native job',
    () async {
      final pending = Completer<RemoteScanStatus>();
      var pollCalls = 0;
      final coordinator = RemoteScanCoordinator(
        progressPollInterval: const Duration(milliseconds: 2),
        statusCall: (_) async {
          pollCalls++;
          return null;
        },
        start:
            ({
              required source,
              required session,
              required rootPath,
              required mode,
            }) => pending.future,
      );
      final source = BookSource(
        id: 'dispose-polling',
        type: 'webdav',
        name: 'dispose-polling',
        path: '/',
      );

      final scan = coordinator.rescanFull(source, BigInt.one);
      try {
        await Future<void>.delayed(const Duration(milliseconds: 20));
        expect(pollCalls, greaterThan(0));

        await coordinator.dispose();
        final callsAfterDispose = pollCalls;
        await Future<void>.delayed(const Duration(milliseconds: 20));
        expect(pollCalls, callsAfterDispose);
      } finally {
        pending.complete(
          const RemoteScanStatus(
            sourceId: 'dispose-polling',
            status: 'complete',
            mode: 'full',
            generation: 1,
          ),
        );
        // The coordinator is intentionally disposed while the native future
        // is pending. Consume the terminal callback so the RED test does not
        // leave an unhandled notifier-disposal error behind.
        await scan.then<void>((_) {}, onError: (_, _) {});
      }
    },
  );

  test(
    'progress polling does not overlap an unresolved status request',
    () async {
      final pending = Completer<RemoteScanStatus>();
      final pollResult = Completer<RemoteScanStatus?>();
      var pollCalls = 0;
      final coordinator = RemoteScanCoordinator(
        progressPollInterval: const Duration(milliseconds: 2),
        statusCall: (_) {
          pollCalls++;
          return pollResult.future;
        },
        start:
            ({
              required source,
              required session,
              required rootPath,
              required mode,
            }) => pending.future,
      );
      final source = BookSource(
        id: 'single-poll-flight',
        type: 'sftp',
        name: 'single-poll-flight',
        path: '/',
      );

      final scan = coordinator.rescanFull(source, BigInt.one);
      try {
        await Future<void>.delayed(const Duration(milliseconds: 20));
        expect(pollCalls, 1);
      } finally {
        pollResult.complete(
          const RemoteScanStatus(
            sourceId: 'single-poll-flight',
            status: 'running',
            mode: 'full',
            generation: 1,
          ),
        );
        pending.complete(
          const RemoteScanStatus(
            sourceId: 'single-poll-flight',
            status: 'complete',
            mode: 'full',
            generation: 1,
          ),
        );
        await scan.then<void>((_) {}, onError: (_, _) {});
        await coordinator.dispose();
      }
    },
  );

  test(
    'paused scan stops polling and resume reattaches it for the next status',
    () async {
      final pending = Completer<RemoteScanStatus>();
      final firstPoll = Completer<void>();
      var pollCalls = 0;
      var returnComplete = false;
      final coordinator = RemoteScanCoordinator(
        progressPollInterval: const Duration(milliseconds: 2),
        statusCall: (_) async {
          pollCalls++;
          if (!firstPoll.isCompleted) firstPoll.complete();
          return RemoteScanStatus(
            sourceId: 'pause-resume-polling',
            status: returnComplete ? 'complete' : 'running',
            mode: 'full',
            generation: 1,
          );
        },
        pauseCall: (_) async {},
        resumeCall: (_) async {},
        start:
            ({
              required source,
              required session,
              required rootPath,
              required mode,
            }) => pending.future,
      );
      final source = BookSource(
        id: 'pause-resume-polling',
        type: '115',
        name: 'pause-resume-polling',
        path: '/',
      );

      final scan = coordinator.rescanFull(source, BigInt.one);
      try {
        await firstPoll.future;
        await coordinator.pause(source.id);
        final callsWhilePaused = pollCalls;
        await Future<void>.delayed(const Duration(milliseconds: 20));
        expect(pollCalls, callsWhilePaused);

        returnComplete = true;
        await coordinator.resume(source.id);
        await Future<void>.delayed(const Duration(milliseconds: 20));
        expect(coordinator.viewStateFor(source.id).value?.status, 'complete');
      } finally {
        returnComplete = true;
        pending.complete(
          const RemoteScanStatus(
            sourceId: 'pause-resume-polling',
            status: 'complete',
            mode: 'full',
            generation: 1,
          ),
        );
        await scan.then<void>((_) {}, onError: (_, _) {});
        await coordinator.dispose();
      }
    },
  );

  test(
    'manual controls validate modes and forward pause resume cancel',
    () async {
      final calls = <String>[];
      final coordinator = RemoteScanCoordinator(
        start:
            ({
              required source,
              required session,
              required rootPath,
              required mode,
            }) async {
              calls.add(mode);
              return RemoteScanStatus(
                sourceId: source.id,
                status: 'running',
                mode: mode,
                generation: 2,
              );
            },
        pauseCall: (id) async => calls.add('pause:$id'),
        resumeCall: (id) async => calls.add('resume:$id'),
        cancelCall: (id) async => calls.add('cancel:$id'),
      );
      final source = BookSource(
        id: 'remote-2',
        type: 'sftp',
        name: 'SSH',
        path: '/books',
      );

      await coordinator.rescan(source, BigInt.two, 'full');
      await expectLater(
        coordinator.rescan(source, BigInt.two, 'snapshot'),
        throwsArgumentError,
      );
      await coordinator.pause(source.id);
      await coordinator.resume(source.id);
      await coordinator.cancel(source.id);
      expect(calls, [
        'full',
        'pause:remote-2',
        'resume:remote-2',
        'cancel:remote-2',
      ]);
    },
  );

  test(
    'provider success hub emits one initial full scan per provider and debounces completion',
    () async {
      final modes = <String, List<String>>{};
      final hub = RemoteSessionSuccessHub();
      final coordinator = RemoteScanCoordinator(
        sessionHub: hub,
        debounce: const Duration(minutes: 1),
        start:
            ({
              required source,
              required session,
              required rootPath,
              required mode,
            }) async {
              modes.putIfAbsent(source.type, () => []).add(mode);
              return RemoteScanStatus(
                sourceId: source.id,
                status: 'complete',
                mode: mode,
                generation: 1,
              );
            },
      );

      for (final type in ['webdav', 'sftp', 'baidu', '115', 'quark']) {
        final source = BookSource(id: type, type: type, name: type, path: '/');
        hub.emit(source, BigInt.one);
        hub.emit(source, BigInt.one);
      }
      await Future<void>.delayed(Duration.zero);
      await Future<void>.delayed(Duration.zero);
      expect(modes, {
        'webdav': ['full'],
        'sftp': ['full'],
        'baidu': ['full'],
        '115': ['full'],
        'quark': ['full'],
      });
      coordinator.dispose();
    },
  );

  test('manual full does not silently join an incremental scan', () async {
    final pending = Completer<RemoteScanStatus>();
    final coordinator = RemoteScanCoordinator(
      start:
          ({
            required source,
            required session,
            required rootPath,
            required mode,
          }) => pending.future,
    );
    final source = BookSource(
      id: 'manual',
      type: 'webdav',
      name: 'manual',
      path: '/',
    );
    unawaited(coordinator.rescanIncremental(source, BigInt.one));
    await expectLater(
      coordinator.rescan(source, BigInt.one, 'full'),
      throwsA(isA<RemoteScanAlreadyRunning>()),
    );
    pending.complete(
      const RemoteScanStatus(
        sourceId: 'manual',
        status: 'complete',
        mode: 'incremental',
        generation: 1,
      ),
    );
  });

  test(
    'persisted running status restarts after a new provider session succeeds',
    () async {
      final hub = RemoteSessionSuccessHub();
      final modes = <String>[];
      final coordinator = RemoteScanCoordinator(
        sessionHub: hub,
        statusCall: (_) async => const RemoteScanStatus(
          sourceId: 'recover',
          status: 'running',
          mode: 'incremental',
          generation: 7,
          checkpoint: '/a',
        ),
        start:
            ({
              required source,
              required session,
              required rootPath,
              required mode,
            }) async {
              modes.add(mode);
              return RemoteScanStatus(
                sourceId: source.id,
                status: 'complete',
                mode: mode,
                generation: 8,
              );
            },
      );
      final source = BookSource(
        id: 'recover',
        type: 'sftp',
        name: 'recover',
        path: '/',
      );
      await coordinator.restoreStatuses([source]);
      hub.emit(source, BigInt.two);
      await Future<void>.delayed(Duration.zero);
      await Future<void>.delayed(Duration.zero);
      expect(modes, ['full']);
      coordinator.dispose();
    },
  );

  test(
    'background setting suppresses automatic session and root triggers',
    () async {
      final hub = RemoteSessionSuccessHub();
      final modes = <String>[];
      final coordinator = RemoteScanCoordinator(
        sessionHub: hub,
        automaticEnabled: () => false,
        start:
            ({
              required source,
              required session,
              required rootPath,
              required mode,
            }) async {
              modes.add(mode);
              return RemoteScanStatus(
                sourceId: source.id,
                status: 'complete',
                mode: mode,
                generation: 1,
              );
            },
      );
      final source = BookSource(
        id: 'disabled',
        type: 'webdav',
        name: 'disabled',
        path: '/',
      );

      hub.emit(source, BigInt.one);
      await coordinator.noteRootListed(source, BigInt.one);
      await Future<void>.delayed(Duration.zero);

      expect(modes, isEmpty);
      coordinator.dispose();
    },
  );

  test(
    'default automatic gate observes setting callback changes after construction',
    () async {
      final previousGate = RemoteScanCoordinator.automaticStartsEnabled;
      final hub = RemoteSessionSuccessHub();
      final modes = <String>[];
      RemoteScanCoordinator.automaticStartsEnabled = () => false;
      final coordinator = RemoteScanCoordinator(
        sessionHub: hub,
        start:
            ({
              required source,
              required session,
              required rootPath,
              required mode,
            }) async {
              modes.add(mode);
              return RemoteScanStatus(
                sourceId: source.id,
                status: 'complete',
                mode: mode,
                generation: 1,
              );
            },
      );
      final source = BookSource(
        id: 'dynamic-gate',
        type: 'webdav',
        name: 'dynamic-gate',
        path: '/',
      );

      hub.emit(source, BigInt.one);
      await Future<void>.delayed(Duration.zero);
      expect(modes, isEmpty);

      RemoteScanCoordinator.automaticStartsEnabled = () => true;
      hub.emit(source, BigInt.one);
      await Future<void>.delayed(Duration.zero);
      expect(modes, ['full']);

      await coordinator.dispose();
      await hub.dispose();
      RemoteScanCoordinator.automaticStartsEnabled = previousGate;
    },
  );

  test(
    'first root open shares the auth full scan and later root open is incremental',
    () async {
      final hub = RemoteSessionSuccessHub();
      final modes = <String>[];
      final first = Completer<RemoteScanStatus>();
      final coordinator = RemoteScanCoordinator(
        sessionHub: hub,
        debounce: Duration.zero,
        start:
            ({
              required source,
              required session,
              required rootPath,
              required mode,
            }) {
              modes.add(mode);
              return first.future;
            },
      );
      final source = BookSource(
        id: 'root-open',
        type: 'sftp',
        name: 'root-open',
        path: '/books',
      );

      hub.emit(source, BigInt.one);
      final rootFuture = coordinator.noteRootListed(source, BigInt.one);
      await Future<void>.delayed(Duration.zero);
      expect(modes, ['full']);

      first.complete(
        const RemoteScanStatus(
          sourceId: 'root-open',
          status: 'complete',
          mode: 'full',
          generation: 1,
        ),
      );
      await rootFuture;

      await coordinator.noteRootListed(source, BigInt.one);
      expect(modes, ['full', 'incremental']);
      coordinator.dispose();
    },
  );

  test('duplicate manual rescans with the same mode join one future', () async {
    final pending = Completer<RemoteScanStatus>();
    var starts = 0;
    final coordinator = RemoteScanCoordinator(
      start:
          ({
            required source,
            required session,
            required rootPath,
            required mode,
          }) {
            starts++;
            return pending.future;
          },
    );
    final source = BookSource(
      id: 'manual-join',
      type: 'webdav',
      name: 'manual-join',
      path: '/',
    );

    final first = coordinator.rescanFull(source, BigInt.one);
    final second = coordinator.rescanFull(source, BigInt.one);
    expect(identical(first, second), isTrue);
    expect(starts, 1);

    pending.complete(
      const RemoteScanStatus(
        sourceId: 'manual-join',
        status: 'complete',
        mode: 'full',
        generation: 3,
      ),
    );
    await first;
  });

  test(
    'manual incremental rescan uses the force-recheck native entry point',
    () async {
      final calls = <String>[];
      final coordinator = RemoteScanCoordinator(
        start:
            ({
              required source,
              required session,
              required rootPath,
              required mode,
            }) async {
              calls.add('automatic:$mode');
              return RemoteScanStatus(
                sourceId: source.id,
                status: 'complete',
                mode: mode,
                generation: 1,
              );
            },
        startManual:
            ({
              required source,
              required session,
              required rootPath,
              required mode,
            }) async {
              calls.add('manual:$mode');
              return RemoteScanStatus(
                sourceId: source.id,
                status: 'complete',
                mode: mode,
                generation: 2,
              );
            },
      );
      final source = BookSource(
        id: 'manual-force',
        type: '115',
        name: '115',
        path: '/',
      );

      await coordinator.rescanIncremental(source, BigInt.one);
      await coordinator.rescanFull(source, BigInt.one);

      expect(calls, ['manual:incremental', 'automatic:full']);
      await coordinator.dispose();
    },
  );

  test('retry preserves the failed full scan mode', () async {
    final modes = <String>[];
    final coordinator = RemoteScanCoordinator(
      start:
          ({
            required source,
            required session,
            required rootPath,
            required mode,
          }) async {
            modes.add(mode);
            return RemoteScanStatus(
              sourceId: source.id,
              status: 'failed',
              mode: mode,
              generation: modes.length,
            );
          },
    );
    final source = BookSource(
      id: 'retry-full',
      type: 'quark',
      name: '夸克',
      path: '/',
    );

    await coordinator.rescanFull(source, BigInt.one);
    await coordinator.retry(source, BigInt.one);

    expect(modes, ['full', 'full']);
    await coordinator.dispose();
  });

  test(
    'native start failure publishes visible failure and releases inflight',
    () async {
      var starts = 0;
      final coordinator = RemoteScanCoordinator(
        start:
            ({
              required source,
              required session,
              required rootPath,
              required mode,
            }) async {
              starts++;
              throw StateError('authorization token=secret');
            },
      );
      final source = BookSource(
        id: 'native-failure',
        type: '115',
        name: '115',
        path: '/',
      );

      final firstResult = await coordinator.rescanFull(source, BigInt.one);
      expect(firstResult.status, anyOf('failed', 'degraded'));

      final failed = coordinator.viewStateFor(source.id).value;
      expect(failed, isNotNull);
      expect(failed!.status, anyOf('failed', 'degraded'));
      expect(failed.errorCode, isNot(contains('secret')));

      final retryResult = await coordinator.rescanFull(source, BigInt.one);
      expect(retryResult.status, anyOf('failed', 'degraded'));
      expect(starts, 2);

      await coordinator.dispose();
    },
  );

  test(
    'failed automatic scan stays full on the next session trigger',
    () async {
      final modes = <String>[];
      final coordinator = RemoteScanCoordinator(
        debounce: Duration.zero,
        start:
            ({
              required source,
              required session,
              required rootPath,
              required mode,
            }) async {
              modes.add(mode);
              return RemoteScanStatus(
                sourceId: source.id,
                status: 'failed',
                mode: mode,
                generation: modes.length,
                errorCode: 'provider',
              );
            },
      );
      final source = BookSource(
        id: 'failed-auto-retry',
        type: 'quark',
        name: '夸克',
        path: '/',
      );

      await coordinator.noteRootListed(source, BigInt.one);
      await coordinator.noteRootListed(source, BigInt.one);

      expect(modes, ['full', 'full']);
      await coordinator.dispose();
    },
  );

  test(
    'a later failed generation invalidates an older incremental baseline',
    () async {
      final modes = <String>[];
      var starts = 0;
      final coordinator = RemoteScanCoordinator(
        debounce: Duration.zero,
        start:
            ({
              required source,
              required session,
              required rootPath,
              required mode,
            }) async {
              starts++;
              modes.add(mode);
              return RemoteScanStatus(
                sourceId: source.id,
                status: starts == 2 ? 'failed' : 'complete',
                mode: mode,
                generation: starts,
              );
            },
      );
      final source = BookSource(
        id: 'failed-after-success',
        type: 'quark',
        name: '夸克',
        path: '/',
      );

      await coordinator.noteRootListed(source, BigInt.one);
      await coordinator.noteRootListed(source, BigInt.one);
      await coordinator.noteRootListed(source, BigInt.one);

      expect(modes, ['full', 'incremental', 'full']);
      await coordinator.dispose();
    },
  );
}
