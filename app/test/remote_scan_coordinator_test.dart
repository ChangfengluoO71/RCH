import 'dart:async';

import 'package:app/store/models.dart';
import 'package:app/store/remote_scan_coordinator.dart';
import 'package:app/store/remote_scan_models.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
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
}
