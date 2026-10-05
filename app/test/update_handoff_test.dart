import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:app/store/update_manager.dart';
import 'package:app/ui/update_panel.dart';
import 'package:crypto/crypto.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

class _DirectHttpOverrides extends HttpOverrides {}

Future<T> _withRealHttp<T>(Future<T> Function() body) =>
    HttpOverrides.runWithHttpOverrides(body, _DirectHttpOverrides());

Future<bool> _waitForFile(File file) async {
  final deadline = DateTime.now().add(const Duration(seconds: 2));
  while (DateTime.now().isBefore(deadline)) {
    if (await file.exists()) return true;
    await Future<void>.delayed(const Duration(milliseconds: 1));
  }
  return false;
}

class _FakeUpdateManager extends UpdateManager {
  _FakeUpdateManager()
    : super.testing(
        platform: UpdatePlatformKind.android,
        downloadDirectoryProvider: () async => Directory.systemTemp,
        mirrorPrefixProvider: () => '',
        effectiveMirrorsProvider: () => const [],
      );

  int downloadCalls = 0;
  int installCalls = 0;
  bool failFirstDownload = false;
  bool failFirstInstall = false;

  @override
  Future<void> download() async {
    downloadCalls++;
    status.value = UpdateStatus.downloading;
    await Future<void>.delayed(Duration.zero);
    if (failFirstDownload && downloadCalls == 1) {
      error.value = 'mock HTTP error';
      status.value = UpdateStatus.error;
      return;
    }
    error.value = null;
    status.value = UpdateStatus.downloaded;
  }

  @override
  Future<void> install() async {
    installCalls++;
    status.value = UpdateStatus.installing;
    await Future<void>.delayed(Duration.zero);
    if (failFirstInstall && installCalls == 1) {
      error.value = 'mock installer cancelled';
      status.value = UpdateStatus.downloaded;
      return;
    }
    error.value = null;
    status.value = UpdateStatus.idle;
  }
}

/// 更新交接契约：纯函数、下载、文件提交、并发合并、安装重试与 UI 流程。
/// 下载测试使用本地 HTTP 服务器和真实临时文件；平台安装器通过与生产入口
/// 同签名的 handler seam 注入，设备级系统安装器行为另做 Windows/MuMu 冒烟。
void main() {
  group('版本比较（是否有新版本）', () {
    test('parseVersion 取 3 段并容忍非数字与 build 号', () {
      expect(UpdateManager.parseVersion('0.6.0+100600'), [0, 6, 0]);
      expect(UpdateManager.parseVersion('1.2'), [1, 2, 0]);
      expect(UpdateManager.parseVersion(' 2.3.4 '), [2, 3, 4]);
      expect(UpdateManager.parseVersion('x.y.z'), [0, 0, 0]);
    });

    test('isNewerVersion 只在远端更高时为真', () {
      expect(UpdateManager.isNewerVersion('0.6.0', '0.5.8'), isTrue);
      expect(UpdateManager.isNewerVersion('0.6.1', '0.6.0'), isTrue);
      expect(UpdateManager.isNewerVersion('1.0.0', '0.9.9'), isTrue);
      expect(UpdateManager.isNewerVersion('0.6.0', '0.6.0'), isFalse);
      expect(UpdateManager.isNewerVersion('0.5.8', '0.6.0'), isFalse);
    });
  });

  group('按平台挑选资产', () {
    final assets = <Map<String, dynamic>>[
      {
        'name': 'app-arm64-v8a-release.apk',
        'size': 10,
        'browser_download_url': 'u-arm64',
      },
      {
        'name': 'app-armeabi-v7a-release.apk',
        'size': 20,
        'browser_download_url': 'u-v7a',
      },
      {
        'name': 'RCH-0.6.0-windows-x64.exe',
        'size': 30,
        'browser_download_url': 'u-exe',
      },
      {'name': 'notes.txt', 'size': 1, 'browser_download_url': 'u-txt'},
      {'name': '', 'size': 0, 'browser_download_url': 'u-empty'},
    ];

    test('Windows 选 RCH-*-windows-x64.exe', () {
      final picked = UpdateManager.pickAssetForPlatform(assets, 'windows');
      expect(picked?.name, 'RCH-0.6.0-windows-x64.exe');
      expect(picked?.url, 'u-exe');
    });

    test('Android 优先 arm64-v8a', () {
      final picked = UpdateManager.pickAssetForPlatform(assets, 'android');
      expect(picked?.name, 'app-arm64-v8a-release.apk');
    });

    test('没有匹配资产时返回 null（不猜、不退回任意文件）', () {
      expect(
        UpdateManager.pickAssetForPlatform(<Map<String, dynamic>>[], 'windows'),
        isNull,
      );
      expect(
        UpdateManager.pickAssetForPlatform(<Map<String, dynamic>>[
          {'name': 'notes.txt', 'size': 1, 'browser_download_url': 'u'},
        ], 'windows'),
        isNull,
      );
    });
  });

  group('下载候选与镜像拼接', () {
    test('downloadCandidates 保留顺序、去重、含选中项', () {
      final urls = UpdateManager.downloadCandidates('official', [
        const MapEntry('m1', 'mirror1'),
        const MapEntry('m2', 'mirror1'),
        const MapEntry('m3', 'mirror2'),
      ]);
      expect(urls, ['official', 'mirror1', 'mirror2']);
    });

    test('buildDownloadUrl 仅在镜像非空时加前缀，并处理尾斜杠', () {
      expect(
        UpdateManager.buildDownloadUrl('https://x/y.exe', ''),
        'https://x/y.exe',
      );
      expect(
        UpdateManager.buildDownloadUrl('https://x/y.exe', '  '),
        'https://x/y.exe',
      );
      expect(
        UpdateManager.buildDownloadUrl('https://x/y.exe', 'https://m'),
        'https://m/https://x/y.exe',
      );
      expect(
        UpdateManager.buildDownloadUrl('https://x/y.exe', 'https://m/'),
        'https://m/https://x/y.exe',
      );
    });
  });

  test('Windows 安装器命令保留 UAC 提权且不插入原始路径文本', () {
    const path = r"C:\Users\test-user\Temp Folder\RCH Setup.exe";
    final args = UpdateManager.windowsInstallerProcessArguments(path);
    final command = args.last;

    expect(command, contains(base64Encode(utf8.encode(path))));
    expect(command, contains('-Verb RunAs'));
    expect(command, contains('-PassThru'));
    expect(command, contains('.WaitForExit()'));
    expect(command, contains(r'$installer.ExitCode'));
    expect(command, contains('exit 255'));
    expect(command, contains('/NORESTART'));
    expect(command, isNot(contains(' -Wait')));
    expect(command, isNot(contains(path)));
  });

  test('Windows installer exit codes keep failures retryable', () {
    expect(
      UpdateManager.windowsInstallerResultForExitCode(0),
      UpdateInstallResult.started,
    );
    for (final code in [1, 2, 5, 255]) {
      expect(
        UpdateManager.windowsInstallerResultForExitCode(code),
        UpdateInstallResult.retryableFailure,
      );
    }
  });

  test('Windows helper command parses without launching an installer', () async {
    if (!Platform.isWindows) return;
    const path = r'C:\Temp\RCH Setup.exe';
    final script = UpdateManager.windowsInstallerProcessArguments(path).last;
    final encodedScript = base64Encode(utf8.encode(script));
    final parseCommand =
        r'$script = [System.Text.Encoding]::UTF8.GetString([System.Convert]::FromBase64String("' +
        encodedScript +
        r'")); $tokens = $null; $errors = $null; [System.Management.Automation.Language.Parser]::ParseInput($script, [ref]$tokens, [ref]$errors) | Out-Null; if ($errors.Count -gt 0) { $errors | ForEach-Object { [Console]::Error.WriteLine($_.Message) }; exit 1 }; exit 0';

    final result = await Process.run('powershell.exe', [
      '-NoProfile',
      '-NonInteractive',
      '-WindowStyle',
      'Hidden',
      '-Command',
      parseCommand,
    ]);

    expect(result.exitCode, 0, reason: '${result.stderr}');
  });

  group('下载与安装交接（本地 HTTP 服务器 + 真实文件系统）', () {
    late Directory tempDir;
    late HttpServer server;
    late Uri baseUri;

    setUp(() async {
      tempDir = await Directory.systemTemp.createTemp('rch-update-test-');
      server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
      baseUri = Uri.parse('http://${server.address.address}:${server.port}');
    });

    tearDown(() async {
      await server.close(force: true);
      if (await tempDir.exists()) await tempDir.delete(recursive: true);
    });

    UpdateManager manager({
      required UpdatePlatformKind platform,
      UpdateInstallHandler? installHandler,
    }) => UpdateManager.testing(
      platform: platform,
      downloadDirectoryProvider: () async => tempDir,
      mirrorPrefixProvider: () => '',
      effectiveMirrorsProvider: () => const [],
      installHandler: installHandler,
    );

    UpdateInfo info({
      required String name,
      required String path,
      required int size,
      String? digest,
    }) => UpdateInfo(
      version: '0.6.3',
      asset: UpdateAsset(
        name: name,
        size: size,
        url: baseUri.resolve(path).toString(),
        digest: digest,
      ),
    );

    test(
      '并发点击共用一次下载，完整落盘后才出现最终安装包',
      () => _withRealHttp(() async {
        const bytes = <int>[1, 2, 3, 4, 5];
        var requestCount = 0;
        final requestStarted = Completer<void>();
        final allowResponse = Completer<void>();
        server.listen((request) async {
          requestCount++;
          request.response.contentLength = bytes.length;
          request.response.add([bytes.first]);
          await request.response.flush();
          if (!requestStarted.isCompleted) requestStarted.complete();
          await allowResponse.future;
          request.response.add(bytes.skip(1).toList());
          await request.response.close();
        });

        final m = manager(platform: UpdatePlatformKind.android);
        m.info = info(
          name: 'update.apk',
          path: '/update.apk',
          size: bytes.length,
          digest: 'sha256:${sha256.convert(bytes)}',
        );
        final first = m.download();
        final second = m.download();

        await requestStarted.future;
        final part = File(
          '${tempDir.path}${Platform.pathSeparator}update.apk.part',
        );
        expect(await _waitForFile(part), isTrue);
        expect(
          File(
            '${tempDir.path}${Platform.pathSeparator}update.apk',
          ).existsSync(),
          isFalse,
        );
        allowResponse.complete();
        await Future.wait([first, second]);

        expect(requestCount, 1);
        expect(m.status.value, UpdateStatus.downloaded);
        expect(await File(m.downloadPath!).readAsBytes(), bytes);
        expect(File('${m.downloadPath}.part').existsSync(), isFalse);
      }),
    );

    test(
      'SHA-256 不匹配时拒绝提交安装包',
      () => _withRealHttp(() async {
        const bytes = <int>[1, 2, 3, 4];
        server.listen((request) async {
          request.response.contentLength = bytes.length;
          request.response.add(bytes);
          await request.response.close();
        });

        final m = manager(platform: UpdatePlatformKind.android);
        m.info = info(
          name: 'update.apk',
          path: '/update.apk',
          size: bytes.length,
          digest: 'sha256:${sha256.convert(const <int>[9, 8, 7, 6])}',
        );
        await m.download();

        expect(m.status.value, UpdateStatus.error);
        expect(m.downloadPath, isNull);
        expect(m.error.value, contains('SHA-256'));
        expect(
          File(
            '${tempDir.path}${Platform.pathSeparator}update.apk',
          ).existsSync(),
          isFalse,
        );
        expect(
          File(
            '${tempDir.path}${Platform.pathSeparator}update.apk.part',
          ).existsSync(),
          isFalse,
        );
      }),
    );

    test(
      '安装前再次校验文件，拒绝下载后被改动的包',
      () => _withRealHttp(() async {
        const bytes = <int>[5, 6, 7, 8];
        var installAttempts = 0;
        server.listen((request) async {
          request.response.contentLength = bytes.length;
          request.response.add(bytes);
          await request.response.close();
        });
        final m = manager(
          platform: UpdatePlatformKind.android,
          installHandler: (path, _) async {
            installAttempts++;
            return UpdateInstallResult.started;
          },
        );
        m.info = info(
          name: 'update.apk',
          path: '/update.apk',
          size: bytes.length,
          digest: 'sha256:${sha256.convert(bytes)}',
        );
        await m.download();
        await File(m.downloadPath!).writeAsBytes(const <int>[8, 7, 6, 5]);

        await m.install();

        expect(installAttempts, 0);
        expect(m.status.value, UpdateStatus.error);
        expect(m.error.value, contains('SHA-256'));
      }),
    );

    test(
      '重复请求复用大小校验通过的已下载安装包',
      () => _withRealHttp(() async {
        const bytes = <int>[4, 5, 6, 7];
        var requestCount = 0;
        server.listen((request) async {
          requestCount++;
          request.response.contentLength = bytes.length;
          request.response.add(bytes);
          await request.response.close();
        });

        final m = manager(platform: UpdatePlatformKind.android);
        m.info = info(
          name: 'update.apk',
          path: '/update.apk',
          size: bytes.length,
          digest: 'sha256:${sha256.convert(bytes)}',
        );
        await m.download();
        await m.download();

        expect(requestCount, 1);
        expect(m.status.value, UpdateStatus.downloaded);
        expect(await File(m.downloadPath!).readAsBytes(), bytes);
      }),
    );

    test(
      '新下载失败时保留先前已验证的安装包与路径',
      () => _withRealHttp(() async {
        server.listen((request) async {
          final bytes = request.uri.path == '/old.apk'
              ? const <int>[1, 2, 3]
              : const <int>[4, 5, 6]; // 新资产故意比声明的大小短
          request.response.contentLength = bytes.length;
          request.response.add(bytes);
          await request.response.close();
        });

        final m = manager(platform: UpdatePlatformKind.android);
        m.info = info(name: 'old.apk', path: '/old.apk', size: 3);
        await m.download();
        final previousPath = m.downloadPath!;

        m.info = info(name: 'new.apk', path: '/new.apk', size: 5);
        await m.download();

        expect(m.status.value, UpdateStatus.error);
        expect(m.downloadPath, previousPath);
        expect(await File(previousPath).readAsBytes(), const <int>[1, 2, 3]);
        expect(
          File('${tempDir.path}${Platform.pathSeparator}new.apk').existsSync(),
          isFalse,
        );
        expect(
          File(
            '${tempDir.path}${Platform.pathSeparator}new.apk.part',
          ).existsSync(),
          isFalse,
        );
      }),
    );

    test(
      '安卓未知来源未授权时保留 APK，授权后可重新打开安装器',
      () => _withRealHttp(() async {
        var attempts = 0;
        final m = manager(
          platform: UpdatePlatformKind.android,
          installHandler: (path, platform) async {
            expect(platform, UpdatePlatformKind.android);
            expect(await File(path).exists(), isTrue);
            attempts++;
            return attempts == 1
                ? UpdateInstallResult.permissionRequired
                : UpdateInstallResult.started;
          },
        );
        server.listen((request) async {
          const bytes = <int>[7, 8, 9];
          request.response.contentLength = bytes.length;
          request.response.add(bytes);
          await request.response.close();
        });
        m.info = info(name: 'update.apk', path: '/update.apk', size: 3);
        await m.download();
        final apkPath = m.downloadPath!;

        await m.install();
        expect(m.status.value, UpdateStatus.downloaded);
        expect(m.error.value, isNotNull);
        expect(await File(apkPath).exists(), isTrue);

        await m.install();
        expect(attempts, 2);
        expect(m.status.value, UpdateStatus.idle);
        expect(m.error.value, isNull);
        expect(await File(apkPath).exists(), isTrue);
      }),
    );

    test(
      '同时触发的安装请求只交接一次',
      () => _withRealHttp(() async {
        const bytes = <int>[1, 2, 3];
        server.listen((request) async {
          request.response.contentLength = bytes.length;
          request.response.add(bytes);
          await request.response.close();
        });
        var attempts = 0;
        final started = Completer<void>();
        final releaseInstaller = Completer<UpdateInstallResult>();
        final m = manager(
          platform: UpdatePlatformKind.android,
          installHandler: (path, _) {
            attempts++;
            if (!started.isCompleted) started.complete();
            return releaseInstaller.future;
          },
        );
        m.info = info(
          name: 'update.apk',
          path: '/update.apk',
          size: bytes.length,
        );
        await m.download();

        final first = m.install();
        final second = m.install();
        await started.future;
        releaseInstaller.complete(UpdateInstallResult.started);
        await Future.wait([first, second]);

        expect(attempts, 1);
        expect(m.status.value, UpdateStatus.idle);
      }),
    );

    test(
      'Windows 安装器取消或失败后保留已校验文件并允许重新安装',
      () => _withRealHttp(() async {
        const bytes = <int>[2, 4, 6, 8];
        var attempts = 0;
        server.listen((request) async {
          request.response.contentLength = bytes.length;
          request.response.add(bytes);
          await request.response.close();
        });
        final m = manager(
          platform: UpdatePlatformKind.windows,
          installHandler: (path, platform) async {
            expect(platform, UpdatePlatformKind.windows);
            expect(await File(path).exists(), isTrue);
            attempts++;
            return attempts == 1
                ? UpdateInstallResult.retryableFailure
                : UpdateInstallResult.started;
          },
        );
        m.info = info(
          name: 'update.exe',
          path: '/update.exe',
          size: bytes.length,
          digest: 'sha256:${sha256.convert(bytes)}',
        );
        await m.download();
        final packagePath = m.downloadPath!;

        await m.install();

        expect(m.status.value, UpdateStatus.downloaded);
        expect(m.error.value, contains('可重试安装'));
        expect(await File(packagePath).readAsBytes(), bytes);

        await m.install();

        expect(attempts, 2);
        expect(m.status.value, UpdateStatus.upToDate);
        expect(m.localVersion.value, '0.6.3');
        expect(await File(packagePath).exists(), isTrue);
      }),
    );
  });

  group('更新器界面流程', () {
    UpdateInfo updateInfo() => const UpdateInfo(
      version: '0.6.3',
      asset: UpdateAsset(name: 'RCH-test.exe', size: 12, url: 'unused'),
    );

    testWidgets('启动提示详情与设置页共用下载并安装流程', (tester) async {
      final manager = _FakeUpdateManager()..info = updateInfo();
      await tester.pumpWidget(
        MaterialApp(
          home: Builder(
            builder: (context) => Scaffold(
              body: TextButton(
                onPressed: () => showUpdateDialog(context, manager: manager),
                child: const Text('打开更新详情'),
              ),
            ),
          ),
        ),
      );

      await tester.tap(find.text('打开更新详情'));
      await tester.pumpAndSettle();
      expect(find.text('发现新版本 v0.6.3'), findsOneWidget);

      await tester.tap(find.widgetWithText(FilledButton, '下载并安装'));
      await tester.pumpAndSettle();

      expect(manager.downloadCalls, 1);
      expect(manager.installCalls, 1);
      expect(find.text('下载并安装更新'), findsNothing);
    });

    testWidgets('下载失败留在进度窗口，用户可重试并继续安装', (tester) async {
      final manager = _FakeUpdateManager()
        ..info = updateInfo()
        ..failFirstDownload = true;
      await tester.pumpWidget(
        MaterialApp(
          home: Builder(
            builder: (context) => Scaffold(
              body: TextButton(
                onPressed: () => showUpdateFlow(context, manager: manager),
                child: const Text('开始更新'),
              ),
            ),
          ),
        ),
      );

      await tester.tap(find.text('开始更新'));
      await tester.pump();
      await tester.pump();
      await tester.pumpAndSettle();
      expect(find.textContaining('mock HTTP error'), findsOneWidget);
      expect(find.text('重新下载并安装'), findsOneWidget);

      await tester.tap(find.text('重新下载并安装'));
      await tester.pumpAndSettle();
      expect(manager.downloadCalls, 2);
      expect(manager.installCalls, 1);
      expect(find.text('下载并安装更新'), findsNothing);
    });

    testWidgets('安装器取消后保留进度窗口，用户可重试安装', (tester) async {
      final manager = _FakeUpdateManager()
        ..info = updateInfo()
        ..failFirstInstall = true;
      await tester.pumpWidget(
        MaterialApp(
          home: Builder(
            builder: (context) => Scaffold(
              body: TextButton(
                onPressed: () => showUpdateFlow(context, manager: manager),
                child: const Text('开始更新'),
              ),
            ),
          ),
        ),
      );

      await tester.tap(find.text('开始更新'));
      await tester.pumpAndSettle();
      expect(find.text('mock installer cancelled'), findsOneWidget);
      expect(find.text('重试安装'), findsOneWidget);

      await tester.tap(find.text('重试安装'));
      await tester.pumpAndSettle();
      expect(manager.installCalls, 2);
      expect(manager.status.value, UpdateStatus.idle);
      expect(find.text('下载并安装更新'), findsNothing);
    });
  });
}
