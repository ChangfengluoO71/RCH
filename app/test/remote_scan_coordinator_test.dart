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
}
