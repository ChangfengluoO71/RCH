import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:package_info_plus/package_info_plus.dart';
import 'package:path_provider/path_provider.dart';

/// 更新流程状态。
enum UpdateStatus {
  idle,
  checking,
  updateAvailable,
  upToDate,
  downloading,
  downloaded,
  installing,
  error,
}

/// GitHub Release 中的安装包资产。
class UpdateAsset {
  const UpdateAsset({required this.name, required this.size, required this.url});

  final String name;
  final int size;
  final String url;
}

/// 最新版本信息（来自 GitHub Releases latest）。
class UpdateInfo {
  const UpdateInfo({
    required this.version,
    required this.asset,
    this.notes,
    this.publishedAt,
  });

  final String version; // 去掉 v 前缀,如 "0.4.0"
  final UpdateAsset asset;
  final String? notes;
  final DateTime? publishedAt;
}

/// 应用更新管理器：检查 GitHub Releases → 下载对应平台安装包 → 启动安装。
/// Windows 走静默安装（安装器自动关闭并重启应用）；Android 走系统安装器。
class UpdateManager {
  UpdateManager._();

  static final UpdateManager instance = UpdateManager._();

  static const String repoOwner = 'ChangfengluoO71';
  static const String repoName = 'RCH';
  static const String releasesUrl = 'https://github.com/$repoOwner/$repoName/releases';
  static const String _apiLatest =
      'https://api.github.com/repos/$repoOwner/$repoName/releases/latest';

  final ValueNotifier<UpdateStatus> status = ValueNotifier(UpdateStatus.idle);
  final ValueNotifier<double> progress = ValueNotifier(0);
  final ValueNotifier<String?> error = ValueNotifier(null);
  final ValueNotifier<String?> localVersion = ValueNotifier(null);

  UpdateInfo? info;
  String? _downloadedPath;
  bool _initDone = false;

  /// 读取当前安装版本（Windows 取 exe 版本资源，Android 取 versionName）。
  Future<void> init() async {
    if (_initDone) return;
    _initDone = true;
    try {
      final pi = await PackageInfo.fromPlatform();
      localVersion.value = pi.version;
    } catch (_) {
      localVersion.value = null;
    }
  }

  /// "0.4.0" / "0.4.0+400" → [0, 4, 0]。
  static List<int> parseVersion(String v) {
    final main = v.split('+').first.trim();
    final parts = main.split('.').map((p) => int.tryParse(p.trim()) ?? 0).toList();
    while (parts.length < 3) {
      parts.add(0);
    }
    return parts.sublist(0, 3);
  }

  static bool isNewerVersion(String remote, String local) {
    final r = parseVersion(remote);
    final l = parseVersion(local);
    for (var i = 0; i < 3; i++) {
      if (r[i] != l[i]) return r[i] > l[i];
    }
    return false;
  }

  /// 按平台挑选安装包资产：Windows 取 RCH-*-windows-x64.exe；
  /// Android 优先 arm64-v8a，其次任意 app-*-release.apk。
  static UpdateAsset? pickAssetForPlatform(
      List<Map<String, dynamic>> assets, String platform) {
    final list = assets
        .map((a) => UpdateAsset(
              name: a['name'] as String? ?? '',
              size: (a['size'] as num?)?.toInt() ?? 0,
              url: a['browser_download_url'] as String? ?? '',
            ))
        .where((a) => a.name.isNotEmpty && a.url.isNotEmpty)
        .toList();
    if (platform == 'windows') {
      for (final a in list) {
        if (a.name.startsWith('RCH-') && a.name.endsWith('-windows-x64.exe')) {
          return a;
        }
      }
      return null;
    }
    if (platform == 'android') {
      UpdateAsset? fallback;
      for (final a in list) {
        if (!a.name.startsWith('app-') || !a.name.endsWith('-release.apk')) continue;
        fallback ??= a;
        if (a.name.contains('arm64-v8a')) return a;
      }
      return fallback;
    }
    return null;
  }

  static String get _platform {
    if (Platform.isWindows) return 'windows';
    if (Platform.isAndroid) return 'android';
    return 'unsupported';
  }

  /// 检查最新版本。silent=true 时不把 error 状态暴露给 UI（自动检查用）。
  Future<void> check({bool silent = false}) async {
    if (_platform == 'unsupported') return;
    final cur = status.value;
    if (cur == UpdateStatus.checking || cur == UpdateStatus.downloading) return;
    await init();
    status.value = UpdateStatus.checking;
    error.value = null;
    try {
      final client = HttpClient()..connectionTimeout = const Duration(seconds: 15);
      final req = await client.getUrl(Uri.parse(_apiLatest));
      req.headers.set(HttpHeaders.userAgentHeader, 'RCH-Updater');
      req.headers.set(HttpHeaders.acceptHeader, 'application/vnd.github+json');
      final resp = await req.close();
      final body = await resp.transform(utf8.decoder).join();
      client.close();
      if (resp.statusCode != 200) {
        throw HttpException('GitHub API 返回 ${resp.statusCode}');
      }
      final json = jsonDecode(body) as Map<String, dynamic>;
      final tag = (json['tag_name'] as String? ?? '').replaceFirst('v', '');
      final assets = (json['assets'] as List?)
              ?.map((a) => (a as Map).cast<String, dynamic>())
              .toList() ??
          const <Map<String, dynamic>>[];
      final asset = pickAssetForPlatform(assets, _platform);
      if (asset == null) {
        throw const FormatException('当前平台没有可用的安装包');
      }
      info = UpdateInfo(
        version: tag,
        asset: asset,
        notes: json['body'] as String?,
        publishedAt: DateTime.tryParse(json['published_at'] as String? ?? ''),
      );
      final local = localVersion.value;
      if (local != null && !isNewerVersion(tag, local)) {
        status.value = UpdateStatus.upToDate;
        return;
      }
      status.value = UpdateStatus.updateAvailable;
    } catch (e) {
      error.value = '$e';
      status.value = silent ? UpdateStatus.idle : UpdateStatus.error;
    }
  }

  /// 下载安装包到本地（Windows: 临时目录; Android: 应用外部目录）。
  Future<void> download() async {
    final i = info;
    if (i == null) return;
    status.value = UpdateStatus.downloading;
    progress.value = 0;
    error.value = null;
    try {
      final dir = Platform.isAndroid
          ? (await getExternalStorageDirectory() ?? await getTemporaryDirectory())
          : await getTemporaryDirectory();
      await dir.create(recursive: true);
      final file = File('${dir.path}${Platform.pathSeparator}${i.asset.name}');
      final client = HttpClient()..connectionTimeout = const Duration(seconds: 30);
      final req = await client.getUrl(Uri.parse(i.asset.url));
      req.headers.set(HttpHeaders.userAgentHeader, 'RCH-Updater');
      final resp = await req.close();
      if (resp.statusCode != 200) {
        throw HttpException('下载失败：HTTP ${resp.statusCode}');
      }
      final total = resp.contentLength;
      final sink = file.openWrite();
      var got = 0;
      await for (final chunk in resp) {
        got += chunk.length;
        sink.add(chunk);
        if (total > 0) progress.value = (got / total).clamp(0.0, 1.0);
      }
      await sink.close();
      client.close();
      if (i.asset.size > 0 && file.lengthSync() != i.asset.size) {
        throw const FileSystemException('安装包大小校验失败，请重试');
      }
      _downloadedPath = file.path;
      status.value = UpdateStatus.downloaded;
    } catch (e) {
      error.value = '$e';
      status.value = UpdateStatus.error;
    }
  }

  /// 启动安装。Windows 静默安装器会自动关闭并重启应用；Android 拉起系统安装器。
  Future<void> install() async {
    final path = _downloadedPath;
    if (path == null) return;
    status.value = UpdateStatus.installing;
    try {
      if (Platform.isWindows) {
        final script =
            "Start-Process -FilePath '$path' -ArgumentList '/VERYSILENT','/SUPPRESSMSGBOXES','/NORESTART','/SP-'";
        await Process.start('powershell.exe', ['-NoProfile', '-Command', script]);
        // 用户取消 UAC 时应用不会退出：10 秒后回到可重试状态。
        unawaited(Future.delayed(const Duration(seconds: 10), () {
          if (status.value == UpdateStatus.installing) {
            status.value = UpdateStatus.downloaded;
          }
        }));
        return;
      }
      if (Platform.isAndroid) {
        final ok = await _invokeAndroidInstall(path);
        if (!ok) {
          error.value = '请先在系统设置中允许安装未知来源应用，再点击安装';
          status.value = UpdateStatus.downloaded;
          return;
        }
        status.value = UpdateStatus.idle;
      }
    } catch (e) {
      error.value = '$e';
      status.value = UpdateStatus.error;
    }
  }

  Future<bool> _invokeAndroidInstall(String path) async {
    const channel = MethodChannel('rch/updater');
    try {
      return await channel.invokeMethod<bool>('installApk', {'path': path}) ?? false;
    } on PlatformException catch (e) {
      if (e.code == 'unknown_sources') {
        return false;
      }
      rethrow;
    }
  }
}
