import 'package:app/store/models.dart';
import 'package:app/store/remote_scan_coordinator.dart';
import 'package:app/store/remote_scan_models.dart';
import 'package:app/ui/comic_cover.dart';
import 'package:app/ui/remote_scan_status.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

const _providerLabels = <({String type, String label})>[
  (type: 'webdav', label: 'WebDAV 无Range'),
  (type: 'sftp', label: 'SFTP'),
  (type: 'baidu', label: '百度网盘'),
  (type: '115', label: '115 网盘'),
  (type: 'quark', label: '夸克网盘'),
];

void main() {
  test('all remote providers expose stable labels and session boundaries', () {
    for (final provider in _providerLabels) {
      final source = BookSource(
        id: 'contract-${provider.type}',
        type: provider.type,
        name: provider.label,
        path: '/',
      );

      expect(source.needsSession, isTrue, reason: provider.label);
      expect(
        source.capabilityDisplay.label,
        provider.label,
        reason: '${provider.type} label changed',
      );
    }
  });

  test(
    'each provider follows full, incremental, and explicit manual scan modes',
    () async {
      for (final provider in _providerLabels) {
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
                  status: 'complete',
                  mode: mode,
                  generation: modes.length,
                );
              },
        );
        final source = BookSource(
          id: 'lifecycle-${provider.type}',
          type: provider.type,
          name: provider.label,
          path: '/',
        );

        await coordinator.ensureForSession(source, BigInt.one);
        await coordinator.ensureForSession(source, BigInt.one);
        await coordinator.rescanIncremental(source, BigInt.one);
        await coordinator.rescanFull(source, BigInt.one);

        expect(modes, [
          'full',
          'incremental',
          'incremental',
          'full',
        ], reason: provider.label);
        await coordinator.dispose();
      }
    },
  );

  test('duplicate manual starts join the same fake provider job', () async {
    final pending = Future<RemoteScanStatus>.delayed(
      const Duration(milliseconds: 1),
      () => const RemoteScanStatus(
        sourceId: 'join',
        status: 'complete',
        mode: 'full',
        generation: 1,
      ),
    );
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
            return pending;
          },
    );
    final source = BookSource(
      id: 'join',
      type: 'quark',
      name: 'Quark',
      path: '/',
    );

    final first = coordinator.rescanFull(source, BigInt.one);
    final second = coordinator.rescanFull(source, BigInt.one);
    expect(identical(first, second), isTrue);
    await first;
    expect(starts, 1);
    await coordinator.dispose();
  });

  test(
    'automatic gate suppresses session/root triggers but manual scan remains available',
    () async {
      final hub = RemoteSessionSuccessHub();
      final automaticModes = <String>[];
      final manualModes = <String>[];
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
              automaticModes.add(mode);
              return RemoteScanStatus(
                sourceId: source.id,
                status: 'complete',
                mode: mode,
                generation: 1,
              );
            },
      );
      final source = BookSource(
        id: 'paused-auto',
        type: 'sftp',
        name: 'SFTP',
        path: '/books',
      );

      hub.emit(source, BigInt.one);
      await coordinator.noteRootListed(source, BigInt.one);
      expect(automaticModes, isEmpty);

      final manuallyStarted = await coordinator.rescanIncremental(
        source,
        BigInt.one,
      );
      manualModes.add(manuallyStarted.mode);
      expect(manualModes, ['incremental']);

      await coordinator.dispose();
      await hub.dispose();
    },
  );

  test(
    'cover fetch pause leaves an existing cover retained and marks view state',
    () async {
      final settings = ValueNotifier<int>(0);
      final retainedCovers = <String>{'webdav|source|book.cbz'};
      var coverFetchEnabled = false;
      final coordinator = RemoteScanCoordinator(
        settingsListenable: settings,
        coverFetchEnabled: () => coverFetchEnabled,
        start:
            ({
              required source,
              required session,
              required rootPath,
              required mode,
            }) async {
              expect(retainedCovers, contains('webdav|source|book.cbz'));
              return const RemoteScanStatus(
                sourceId: 'source',
                status: 'complete',
                mode: 'full',
                generation: 1,
              );
            },
      );
      final source = BookSource(
        id: 'source',
        type: 'webdav',
        name: 'WebDAV',
        path: '/',
      );

      await coordinator.rescanFull(source, BigInt.one);
      expect(
        coordinator.viewStateFor(source.id).value?.coverFetchPaused,
        isTrue,
      );

      coverFetchEnabled = true;
      settings.value++;
      expect(
        coordinator.viewStateFor(source.id).value?.coverFetchPaused,
        isFalse,
      );
      expect(retainedCovers, contains('webdav|source|book.cbz'));

      await coordinator.dispose();
      settings.dispose();
    },
  );

  test(
    'disabled cover gate does not run destructive-looking cover work',
    () async {
      final retainedCovers = <String>{'quark|source|folder'};
      var operationCalls = 0;

      final result = await runRemoteCoverOperationWithGate<bool>(
        isEnabled: () => false,
        operation: () async {
          operationCalls++;
          retainedCovers.clear();
          return true;
        },
      );

      expect(result, isNull);
      expect(operationCalls, 0);
      expect(retainedCovers, contains('quark|source|folder'));
    },
  );

  testWidgets(
    'provider status redacts credentials, headers, and private URLs',
    (tester) async {
      final state = ValueNotifier(
        const RemoteScanViewState(
          sourceId: 'redacted',
          status: 'degraded',
          mode: 'incremental',
          errorCode:
              'provider failure Authorization=auth-value; '
              'Bearer bearer-value; Cookie: session-cookie; token=token-value; '
              'https://user:password@private.example.invalid/library',
        ),
      );

      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: RemoteScanStatusPanel(
              sourceName: 'Quark',
              stateListenable: state,
            ),
          ),
        ),
      );

      expect(find.textContaining('Quark：'), findsOneWidget);
      expect(find.textContaining('bearer-value'), findsNothing);
      expect(find.textContaining('auth-value'), findsNothing);
      expect(find.textContaining('session-cookie'), findsNothing);
      expect(find.textContaining('token-value'), findsNothing);
      expect(find.textContaining('password'), findsNothing);
      expect(find.textContaining('private.example.invalid'), findsNothing);
      expect(find.textContaining('https://'), findsNothing);

      state.dispose();
    },
  );
}
