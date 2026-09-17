import 'dart:async';
import 'dart:convert';

import 'package:app/src/rust/api/library.dart' as frb;
import 'package:app/store/models.dart';
import 'package:app/store/remote_scan_coordinator.dart';
import 'package:app/store/remote_scan_models.dart';
import 'package:app/ui/comic_cover.dart';
import 'package:app/ui/source_browser.dart';
import 'package:app/ui/source_tree.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter_rust_bridge/flutter_rust_bridge_for_generated.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test(
    'offline reconnect defers only when the current logical path is the root',
    () {
      expect(
        shouldDeferRemoteRootListing(
          currentLogicalPath: r'\\books\\',
          effectiveRootPath: '/books',
        ),
        isTrue,
      );
      expect(
        shouldDeferRemoteRootListing(
          currentLogicalPath: '/books/series',
          effectiveRootPath: '/books',
        ),
        isFalse,
      );
    },
  );

  test(
    'offline non-root reconnect triggers automatic scan instead of root handoff',
    () {
      expect(
        shouldTriggerRemoteScanAfterReconnect(
          currentLogicalPath: '/books/series',
          effectiveRootPath: '/books',
        ),
        isTrue,
      );
      expect(
        shouldTriggerRemoteScanAfterReconnect(
          currentLogicalPath: '/books',
          effectiveRootPath: '/books',
        ),
        isFalse,
      );
    },
  );

  test('source tree explains when background scanning is disabled', () {
    final source = frb.SourceAvailabilityDto(
      sourceId: 'source-tree',
      fingerprint: 'fingerprint',
      name: '云端书源',
      type: '115',
      path: '/',
      hasLocalSource: true,
      hasLocalResource: false,
      hasCredentials: true,
      deviceId: 'device',
      deviceName: 'device',
      isRemote: false,
      offlineIndexCount: PlatformInt64Util.from(0),
      canBrowseOffline: true,
      requiresNetwork: true,
      status: 'read',
    );

    expect(
      remoteScanIdleMessageFor(source, backgroundScanEnabled: false),
      '后台扫描已关闭',
    );
    expect(
      remoteScanIdleMessageFor(source, backgroundScanEnabled: true),
      '等待扫描',
    );

    final offlineSource = frb.SourceAvailabilityDto(
      sourceId: 'offline-source-tree',
      fingerprint: 'fingerprint',
      name: '离线书源',
      type: '115',
      path: '/',
      hasLocalSource: false,
      hasLocalResource: false,
      hasCredentials: false,
      deviceId: 'device',
      deviceName: 'device',
      isRemote: true,
      offlineIndexCount: PlatformInt64Util.from(1),
      canBrowseOffline: true,
      requiresNetwork: true,
      status: 'index_only',
    );
    expect(
      remoteScanIdleMessageFor(offlineSource, backgroundScanEnabled: false),
      '仅离线索引，不执行在线扫描',
    );
  });

  test(
    'deferred root listing is handed to the first scan exactly once',
    () async {
      final hub = RemoteSessionSuccessHub();
      final completed = Completer<RemoteScanStatus>();
      final calls = <({String rootPath, String? listing, String mode})>[];
      final coordinator = RemoteScanCoordinator(
        sessionHub: hub,
        startWithInitialListing:
            ({
              required source,
              required session,
              required rootPath,
              required mode,
              required initialListingJson,
            }) {
              calls.add((
                rootPath: rootPath,
                listing: initialListingJson,
                mode: mode,
              ));
              return completed.future;
            },
      );
      final source = BookSource(
        id: 'seeded-root',
        type: 'webdav',
        name: 'seeded-root',
        path: '/books',
      );
      final listing = jsonEncode([
        {
          'name': 'series',
          'path': '/books/series',
          'logicalPath': '/books/series',
          'isDir': true,
        },
        {
          'name': 'one.cbz',
          'path': '/books/one.cbz',
          'logicalPath': '/books/one.cbz',
          'isDir': false,
        },
      ]);

      coordinator.deferRootListing(source);
      hub.emit(source, BigInt.one);
      await Future<void>.delayed(Duration.zero);
      expect(calls, isEmpty);

      final scan = coordinator.noteRootListed(
        source,
        BigInt.one,
        initialListingJson: listing,
      );
      await Future<void>.delayed(Duration.zero);
      expect(calls, [(rootPath: '/books', listing: listing, mode: 'full')]);

      completed.complete(
        const RemoteScanStatus(
          sourceId: 'seeded-root',
          status: 'complete',
          mode: 'full',
          generation: 1,
        ),
      );
      await scan;
      await coordinator.dispose();
      await hub.dispose();
    },
  );

  test(
    'cover fetch gate is checked again after asynchronous session creation',
    () async {
      var enabled = true;
      var providerCalls = 0;

      final result = await loadCoverWithSafePolicy<String>(
        remoteFetchEnabled: true,
        remoteFetchEnabledNow: () => enabled,
        liveSession: null,
        createSession: () async {
          enabled = false;
          return BigInt.one;
        },
        load: (_, _) async {
          providerCalls++;
          return 'cover';
        },
      );

      expect(result, isNull);
      expect(providerCalls, 0);
    },
  );

  test(
    'remote scan view state follows dynamic cover-fetch setting changes',
    () async {
      final coverFetchEnabled = ValueNotifier(false);
      final coordinator = RemoteScanCoordinator(
        settingsListenable: coverFetchEnabled,
        coverFetchEnabled: () => coverFetchEnabled.value,
        start:
            ({
              required source,
              required session,
              required rootPath,
              required mode,
            }) async => RemoteScanStatus(
              sourceId: source.id,
              status: 'complete',
              mode: mode,
              generation: 1,
            ),
      );
      final source = BookSource(
        id: 'cover-gate',
        type: 'sftp',
        name: 'cover-gate',
        path: '/',
      );

      await coordinator.rescanFull(source, BigInt.one);
      expect(
        coordinator.viewStateFor(source.id).value?.coverFetchPaused,
        isTrue,
      );

      coverFetchEnabled.value = true;
      expect(
        coordinator.viewStateFor(source.id).value?.coverFetchPaused,
        isFalse,
      );

      await coordinator.dispose();
      coverFetchEnabled.dispose();
    },
  );
}
