import 'dart:async';
import 'dart:io';

import 'package:app/store/update_manager.dart';
import 'package:app/ui/update_panel.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

const _asset = UpdateAsset(
  name: 'RCH-0.5.8-windows-x64.exe',
  size: 3,
  url: 'https://example.test/RCH-0.5.8-windows-x64.exe',
);

const _info = UpdateInfo(version: '0.5.8', asset: _asset);

void main() {
  group('UpdateManager download and install handoff', () {
    late _FakeUpdatePlatform platform;
    late _FakeDownloadTransport transport;
    late UpdateManager manager;

    setUp(() {
      platform = _FakeUpdatePlatform(UpdatePlatformKind.windows);
      transport = _FakeDownloadTransport();
      manager = UpdateManager.testing(
        platform: platform,
        downloadTransport: transport,
      )..info = _info;
    });

    test('concurrent download requests share one active transfer', () async {
      final first = manager.download();
      final second = manager.download();

      expect(identical(first, second), isTrue);
      await _flush();
      expect(transport.downloadCalls, 1);

      transport.completeDownload();
      await Future.wait([first, second]);
      expect(manager.status.value, UpdateStatus.downloaded);
    });

    test(
      'dialog-confirmed download hands off a verified package once',
      () async {
        final first = manager.confirmDialogDownload();
        final duplicate = manager.confirmDialogDownload();

        expect(identical(first, duplicate), isTrue);
        await _flush();
        transport.completeDownload();
        await first;

        expect(platform.windowsLaunches, 1);
        expect(
          platform.windowsExecutable,
          '/updates/RCH-0.5.8-windows-x64.exe',
        );
        expect(platform.windowsArguments, [
          '/VERYSILENT',
          '/SUPPRESSMSGBOXES',
          '/NORESTART',
          '/SP-',
        ]);
        expect(manager.status.value, UpdateStatus.installing);
      },
    );

    test(
      'manual download leaves the verified package ready for manual install',
      () async {
        final download = manager.download();
        await _flush();
        transport.completeDownload();
        await download;

        expect(platform.windowsLaunches, 0);
        expect(manager.status.value, UpdateStatus.downloaded);
      },
    );

    test(
      'verified package is reused instead of starting a destructive retry',
      () async {
        final download = manager.download();
        await _flush();
        transport.completeDownload();
        await download;

        final callsAfterSuccess = transport.downloadCalls;
        await manager.download();

        expect(transport.downloadCalls, callsAfterSuccess);
        expect(manager.hasVerifiedPackage, isTrue);
        expect(manager.status.value, UpdateStatus.downloaded);
      },
    );

    test(
      'failed platform handoff preserves the verified package for retry',
      () async {
        platform.succeeds = false;
        final download = manager.confirmDialogDownload();
        await _flush();
        transport.completeDownload();
        await download;

        expect(manager.status.value, UpdateStatus.downloaded);
        expect(manager.hasVerifiedPackage, isTrue);
        expect(transport.deletedPaths, isEmpty);
      },
    );

    test('Android handoff failure preserves the APK for retry', () async {
      platform = _FakeUpdatePlatform(UpdatePlatformKind.android)
        ..succeeds = false;
      manager =
          UpdateManager.testing(
              platform: platform,
              downloadTransport: transport,
            )
            ..info = const UpdateInfo(
              version: '0.5.8',
              asset: UpdateAsset(
                name: 'app-arm64-v8a-release.apk',
                size: 3,
                url: 'https://example.test/app-arm64-v8a-release.apk',
              ),
            );

      final download = manager.confirmDialogDownload();
      await _flush();
      transport.completeDownload();
      await download;

      expect(platform.androidLaunches, 1);
      expect(manager.status.value, UpdateStatus.downloaded);
      expect(manager.hasVerifiedPackage, isTrue);
      expect(transport.deletedPaths, isEmpty);
    });

    test(
      'download errors do not expose a mirror URL in manager state',
      () async {
        final download = manager.download();
        await _flush();
        transport.completeDownloadError(
          HttpException('GET https://token@example.test/private-installer'),
        );
        await download;

        expect(manager.status.value, UpdateStatus.error);
        expect(manager.error.value, isNot(contains('https://')));
        expect(manager.error.value, isNot(contains('token@')));
      },
    );
    test('parses a GitHub sha256 digest on the selected asset', () {
      final digest = 'a' * 64;
      final asset = UpdateManager.pickAssetForPlatform([
        {
          'name': 'RCH-0.5.8-windows-x64.exe',
          'size': 3,
          'browser_download_url': 'https://example.test/update.exe',
          'digest': 'sha256:$digest',
        },
      ], 'windows');

      expect(asset?.sha256, digest);
    });

    test('rejects a downloaded package with a mismatched sha256', () async {
      final directory = await Directory.systemTemp.createTemp('rch-update-');
      addTearDown(() => directory.delete(recursive: true));
      final transport = _HashDownloadTransport(directory.path);
      final manager =
          UpdateManager.testing(
              platform: _FakeUpdatePlatform(UpdatePlatformKind.windows),
              downloadTransport: transport,
            )
            ..info = const UpdateInfo(
              version: '0.5.8',
              asset: UpdateAsset(
                name: 'RCH-0.5.8-windows-x64.exe',
                size: 3,
                url: 'https://example.test/update.exe',
                sha256:
                    '0000000000000000000000000000000000000000000000000000000000000000',
              ),
            );

      await manager.download();

      expect(manager.status.value, UpdateStatus.error);
      expect(manager.hasVerifiedPackage, isFalse);
      expect(transport.deletedPaths, isNotEmpty);
    });
  });

  testWidgets(
    'dialog confirmation immediately shows shared download progress',
    (tester) async {
      final transport = _FakeDownloadTransport();
      final manager = UpdateManager.testing(
        platform: _FakeUpdatePlatform(UpdatePlatformKind.windows),
        downloadTransport: transport,
      )..info = _info;

      await tester.pumpWidget(
        MaterialApp(
          home: Builder(
            builder: (context) => FilledButton(
              onPressed: () => showUpdateDialog(context, manager: manager),
              child: const Text('open update'),
            ),
          ),
        ),
      );

      await tester.tap(find.text('open update'));
      await tester.pumpAndSettle();
      await tester.tap(find.text('下载更新'));
      await tester.pump();

      expect(manager.status.value, UpdateStatus.downloading);
      expect(find.byType(LinearProgressIndicator), findsOneWidget);

      await tester.tap(find.text('关闭'));
      await tester.pump();
      expect(manager.status.value, UpdateStatus.downloading);
    },
  );
}

Future<void> _flush() => Future<void>.delayed(Duration.zero);

class _FakeUpdatePlatform implements UpdatePlatform {
  _FakeUpdatePlatform(this.kind);

  @override
  final UpdatePlatformKind kind;
  bool succeeds = true;
  int windowsLaunches = 0;
  int androidLaunches = 0;
  String? windowsExecutable;
  List<String>? windowsArguments;

  @override
  Future<bool> launchAndroidInstaller(String apkPath) async {
    androidLaunches++;
    return succeeds;
  }

  @override
  Future<bool> launchWindowsInstaller(
    String executable,
    List<String> arguments,
  ) async {
    windowsLaunches++;
    windowsExecutable = executable;
    windowsArguments = arguments;
    return succeeds;
  }
}

class _FakeDownloadTransport implements UpdateDownloadTransport {
  final _completion = Completer<void>();
  int downloadCalls = 0;
  final List<String> deletedPaths = [];

  void completeDownload() {
    if (!_completion.isCompleted) _completion.complete();
  }

  void completeDownloadError(Object error) {
    if (!_completion.isCompleted) _completion.completeError(error);
  }

  @override
  Future<void> delete(String destinationPath) async {
    deletedPaths.add(destinationPath);
  }

  @override
  Future<int> download({
    required String destinationPath,
    required UpdateInfo info,
    required String mirror,
    required ValueChanged<double> onProgress,
  }) async {
    downloadCalls++;
    onProgress(0.4);
    await _completion.future;
    onProgress(1);
    return info.asset.size;
  }

  @override
  Future<String> prepareDestination(
    UpdateAsset asset,
    UpdatePlatformKind platform,
  ) async {
    return '/updates/${asset.name}';
  }
}

class _HashDownloadTransport extends _FakeDownloadTransport {
  _HashDownloadTransport(this.root);

  final String root;

  @override
  Future<String> prepareDestination(
    UpdateAsset asset,
    UpdatePlatformKind platform,
  ) async {
    return '$root/${asset.name}';
  }

  @override
  Future<int> download({
    required String destinationPath,
    required UpdateInfo info,
    required String mirror,
    required ValueChanged<double> onProgress,
  }) async {
    await File(destinationPath).writeAsBytes([1, 2, 3], flush: true);
    return 3;
  }
}
