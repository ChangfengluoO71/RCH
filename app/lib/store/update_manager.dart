import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:crypto/crypto.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:package_info_plus/package_info_plus.dart';
import 'package:path_provider/path_provider.dart';
import 'package:app/store/library_store.dart';

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

/// Platforms supported by the in-app updater.
enum UpdatePlatformKind { windows, android, unsupported }

/// Result of handing a downloaded package to the operating system.
enum UpdateInstallResult { started, permissionRequired, retryableFailure }

typedef UpdateInstallHandler =
    Future<UpdateInstallResult> Function(
      String path,
      UpdatePlatformKind platform,
    );

/// GitHub Release 中的安装包资产。
class UpdateAsset {
  const UpdateAsset({
    required this.name,
    required this.size,
    required this.url,
    this.digest,
  });

  final String name;
  final int size;
  final String url;

  /// GitHub Releases asset digest, usually `sha256:<hex>`.
  final String? digest;
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
/// Windows 打开可见的 Inno Setup 向导并交由 UAC 授权；Android 走系统安装器。
class UpdateManager {
  UpdateManager._({
    this._platformOverride,
    this._downloadDirectoryProvider,
    this._mirrorPrefixProvider,
    this._effectiveMirrorsProvider,
    this._installHandler,
  });

  /// Builds an isolated manager for contract tests. Production uses [instance].
  @visibleForTesting
  UpdateManager.testing({
    required UpdatePlatformKind platform,
    required Future<Directory> Function() downloadDirectoryProvider,
    required String Function() mirrorPrefixProvider,
    required List<MapEntry<String, String>> Function() effectiveMirrorsProvider,
    UpdateInstallHandler? installHandler,
  }) : this._(
         platformOverride: platform,
         downloadDirectoryProvider: downloadDirectoryProvider,
         mirrorPrefixProvider: mirrorPrefixProvider,
         effectiveMirrorsProvider: effectiveMirrorsProvider,
         installHandler: installHandler,
       );

  static final UpdateManager instance = UpdateManager._();

  static const String repoOwner = 'ChangfengluoO71';
  static const String repoName = 'RCH';
  static const String releasesUrl =
      'https://github.com/$repoOwner/$repoName/releases';
  static const String _defaultApiLatest =
      'https://api.github.com/repos/$repoOwner/$repoName/releases/latest';

  /// Overridden only by test builds to exercise the updater against a local release fixture.
  static const String apiLatest = String.fromEnvironment(
    'RCH_UPDATE_LATEST_URL',
    defaultValue: _defaultApiLatest,
  );

  /// 远程镜像列表地址：仓库内 `mirrors.json`，经 jsDelivr CDN 分发
  /// （国内可直连、不依赖 GitHub），应用启动/打开更新面板时自动拉取合并。
  static const String mirrorListUrl =
      'https://cdn.jsdelivr.net/gh/$repoOwner/$repoName@master/mirrors.json';

  /// 下载镜像预设（前缀代理：官方直链前加镜像前缀即可加速）。
  /// 镜像为第三方社区服务，可用性随时可能变化；`ghproxy.link` 会列出最新可用地址。
  static const List<MapEntry<String, String>> mirrorPresets = [
    MapEntry('官方 GitHub（直连）', ''),
    MapEntry('ghproxy.net', 'https://ghproxy.net/'),
    MapEntry('gh-proxy.com', 'https://gh-proxy.com/'),
    MapEntry('ghfast.top', 'https://ghfast.top/'),
    MapEntry('mirror.ghproxy.com', 'https://mirror.ghproxy.com/'),
  ];

  final ValueNotifier<UpdateStatus> status = ValueNotifier(UpdateStatus.idle);
  final ValueNotifier<double> progress = ValueNotifier(0);
  final ValueNotifier<String?> error = ValueNotifier(null);
  final ValueNotifier<String?> localVersion = ValueNotifier(null);

  final UpdatePlatformKind? _platformOverride;
  final Future<Directory> Function()? _downloadDirectoryProvider;
  final String Function()? _mirrorPrefixProvider;
  final List<MapEntry<String, String>> Function()? _effectiveMirrorsProvider;
  final UpdateInstallHandler? _installHandler;
  Future<void>? _downloadInFlight;
  Future<void>? _installInFlight;

  UpdateInfo? info;
  String? _downloadedPath;
  UpdateAsset? _downloadedAsset;

  /// 最近一次下载的安装包路径（供界面显示"安装包保存位置"，避免用户自己去翻临时目录）。
  String? get downloadPath => _downloadedPath;
  bool _initDone = false;

  /// 用户选择的镜像前缀（来自设置；可为自定义地址）。
  String get mirrorPrefix {
    final provider = _mirrorPrefixProvider;
    if (provider != null) return provider();
    final raw = LibraryStore.instance.settings.updateMirror.trim();
    if (raw.isEmpty) return '';
    return raw.endsWith('/') ? raw : '$raw/';
  }

  /// 生效镜像列表：远端拉取（已持久化）在前，内置预设兜底；按 URL 去重。
  List<MapEntry<String, String>> get effectiveMirrors {
    final provider = _effectiveMirrorsProvider;
    if (provider != null) return provider();
    final byUrl = <String, String>{};
    final order = <String>[];
    void add(String name, String url) {
      final u = url.trim();
      if (u.isEmpty) return;
      if (!byUrl.containsKey(u)) order.add(u);
      byUrl[u] = name;
    }

    try {
      final remote =
          jsonDecode(LibraryStore.instance.settings.updateMirrorList) as List;
      for (final item in remote) {
        final m = (item as Map).cast<String, dynamic>();
        final name = m['name']?.toString() ?? '';
        final url = m['url']?.toString() ?? '';
        if (url.startsWith('https://')) add(name.isEmpty ? url : name, url);
      }
    } catch (_) {
      // 远端列表损坏时忽略，仅用内置预设
    }
    for (final p in mirrorPresets) {
      add(p.key, p.value);
    }
    return order.map((u) => MapEntry(byUrl[u] ?? u, u)).toList();
  }

  /// 上次拉取镜像列表距今是否超过 24 小时。
  bool get remoteMirrorsStale {
    final at = LibraryStore.instance.settings.updateMirrorFetchedAt;
    return DateTime.now().millisecondsSinceEpoch - at >
        const Duration(hours: 24).inMilliseconds;
  }

  /// 从 CDN 拉取最新镜像列表并持久化；失败返回 false（保留旧列表）。
  Future<bool> refreshRemoteMirrors() async {
    try {
      final client = HttpClient()
        ..connectionTimeout = const Duration(seconds: 15);
      final req = await client.getUrl(Uri.parse(mirrorListUrl));
      req.headers.set(HttpHeaders.userAgentHeader, 'RCH-Updater');
      final resp = await req.close();
      final body = await resp.transform(utf8.decoder).join();
      client.close();
      if (resp.statusCode != 200) return false;
      final json = jsonDecode(body) as Map<String, dynamic>;
      final mirrors = (json['mirrors'] as List?) ?? const [];
      final normalized = <Map<String, String>>[];
      for (final item in mirrors) {
        final m = (item as Map).cast<String, dynamic>();
        final name = m['name']?.toString().trim() ?? '';
        final url = m['url']?.toString().trim() ?? '';
        if (name.isNotEmpty && url.startsWith('https://')) {
          normalized.add({'name': name, 'url': url});
        }
      }
      if (normalized.isEmpty) return false;
      final s = LibraryStore.instance.settings;
      s.updateMirrorList = jsonEncode(normalized);
      s.updateMirrorFetchedAt = DateTime.now().millisecondsSinceEpoch;
      LibraryStore.instance.updateSettings(s);
      return true;
    } catch (_) {
      return false;
    }
  }

  /// 下载通道候选（当前选择优先，其余镜像兜底，去重）。
  @visibleForTesting
  static List<String> downloadCandidates(
    String selected,
    List<MapEntry<String, String>> mirrors,
  ) {
    final urls = <String>[];
    void add(String u) {
      final t = u.trim();
      if (!urls.contains(t)) urls.add(t);
    }

    add(selected);
    for (final m in mirrors) {
      add(m.value);
    }
    return urls;
  }

  /// 官方直链套镜像前缀（仅下载用）；镜像为空时原样返回。
  @visibleForTesting
  static String buildDownloadUrl(String officialUrl, String mirror) {
    final m = mirror.trim();
    if (m.isEmpty) return officialUrl;
    return '${m.endsWith('/') ? m : '$m/'}$officialUrl';
  }

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
    final parts = main
        .split('.')
        .map((p) => int.tryParse(p.trim()) ?? 0)
        .toList();
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
    List<Map<String, dynamic>> assets,
    String platform,
  ) {
    final list = assets
        .map(
          (a) => UpdateAsset(
            name: a['name'] as String? ?? '',
            size: (a['size'] as num?)?.toInt() ?? 0,
            url: a['browser_download_url'] as String? ?? '',
            digest: a['digest'] as String?,
          ),
        )
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
        if (!a.name.startsWith('app-') || !a.name.endsWith('-release.apk')) {
          continue;
        }
        fallback ??= a;
        if (a.name.contains('arm64-v8a')) return a;
      }
      return fallback;
    }
    return null;
  }

  String get _platform {
    final override = _platformOverride;
    if (override != null) return override.name;
    if (Platform.isWindows) return 'windows';
    if (Platform.isAndroid) return 'android';
    return 'unsupported';
  }

  /// 检查最新版本。silent=true 时不把 error 状态暴露给 UI（自动检查用）。
  Future<void> check({bool silent = false}) async {
    if (_platform == 'unsupported') return;
    final cur = status.value;
    if (cur == UpdateStatus.checking ||
        cur == UpdateStatus.downloading ||
        cur == UpdateStatus.installing) {
      return;
    }
    await init();
    status.value = UpdateStatus.checking;
    error.value = null;
    try {
      final client = HttpClient()
        ..connectionTimeout = const Duration(seconds: 15);
      final req = await client.getUrl(Uri.parse(apiLatest));
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
      final assets =
          (json['assets'] as List?)
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
  /// 按「当前选择 → 其余镜像」顺序尝试，单个通道失败自动切换下一个；
  /// 全部失败才报错，错误信息里带尝试过的通道列表。
  Future<void> download() {
    final active = _downloadInFlight;
    if (active != null) return active;

    late final Future<void> flight;
    flight = _downloadImpl().whenComplete(() {
      if (identical(_downloadInFlight, flight)) _downloadInFlight = null;
    });
    _downloadInFlight = flight;
    return flight;
  }

  Future<void> _downloadImpl() async {
    final i = info;
    if (i == null) return;
    status.value = UpdateStatus.downloading;
    progress.value = 0;
    error.value = null;
    try {
      final dir = await _downloadDirectory();
      await dir.create(recursive: true);
      final file = File('${dir.path}${Platform.pathSeparator}${i.asset.name}');
      if (await _isVerifiedPackage(file, i.asset)) {
        _downloadedPath = file.path;
        _downloadedAsset = i.asset;
        progress.value = 1;
        status.value = UpdateStatus.downloaded;
        return;
      }

      final part = File('${file.path}.part');
      final candidates = downloadCandidates(mirrorPrefix, effectiveMirrors);
      final tried = <String>[];
      Object? lastErr;
      for (final mirror in candidates) {
        final label = mirror.isEmpty ? '官方直连' : mirror;
        tried.add(label);
        try {
          await _downloadVia(part, i, mirror);
          final integrityError = await _packageIntegrityError(part, i.asset);
          if (integrityError != null) {
            throw FileSystemException(integrityError);
          }
          await _commitDownload(part, file);
          _downloadedPath = file.path;
          _downloadedAsset = i.asset;
          progress.value = 1;
          status.value = UpdateStatus.downloaded;
          return;
        } catch (e) {
          lastErr = e;
          if (await part.exists()) {
            try {
              await part.delete();
            } catch (_) {}
          }
          if (candidates.length > 1) {
            error.value = '通道「$label」失败，自动切换下一个…';
          }
        }
      }
      throw HttpException('全部下载通道失败（已尝试：${tried.join('、')}）。$lastErr');
    } catch (e) {
      error.value = '$e';
      status.value = UpdateStatus.error;
    }
  }

  Future<Directory> _downloadDirectory() async {
    final provider = _downloadDirectoryProvider;
    if (provider != null) return provider();
    if (_platform == 'android') {
      return await getExternalStorageDirectory() ??
          await getTemporaryDirectory();
    }
    return getTemporaryDirectory();
  }

  Future<bool> _isVerifiedPackage(File file, UpdateAsset asset) async {
    return await _packageIntegrityError(file, asset) == null;
  }

  Future<String?> _packageIntegrityError(File file, UpdateAsset asset) async {
    if (!await file.exists()) return '安装包不存在';
    final actualSize = await file.length();
    if (asset.size > 0 && actualSize != asset.size) {
      return '安装包大小校验失败';
    }
    final expectedDigest = _expectedSha256(asset.digest);
    if (expectedDigest == null) {
      return asset.size > 0 ? null : '安装包缺少可验证的大小或 SHA-256 摘要';
    }
    final actualDigest = await sha256.bind(file.openRead()).first;
    if (actualDigest.toString() != expectedDigest) {
      return '安装包 SHA-256 校验失败';
    }
    return null;
  }

  String? _expectedSha256(String? digest) {
    final raw = digest?.trim();
    if (raw == null || raw.isEmpty) return null;
    final normalized = raw.toLowerCase();
    if (!normalized.startsWith('sha256:')) {
      throw FormatException('安装包摘要算法不受支持：$raw');
    }
    final value = normalized.substring('sha256:'.length);
    if (!RegExp(r'^[0-9a-f]{64}$').hasMatch(value)) {
      throw const FormatException('安装包 SHA-256 摘要格式无效');
    }
    return value;
  }

  Future<void> _commitDownload(File part, File destination) async {
    try {
      await part.rename(destination.path);
    } on FileSystemException {
      // Windows may not replace an existing file with rename. At this point the
      // new .part has passed size validation; an existing wrong-sized package
      // can be removed and the complete file committed.
      if (!await destination.exists()) rethrow;
      await destination.delete();
      await part.rename(destination.path);
    }
  }

  Future<void> _downloadVia(File file, UpdateInfo i, String mirror) async {
    final client = HttpClient()
      ..connectionTimeout = const Duration(seconds: 30);
    try {
      final req = await client.getUrl(
        Uri.parse(buildDownloadUrl(i.asset.url, mirror)),
      );
      req.headers.set(HttpHeaders.userAgentHeader, 'RCH-Updater');
      final resp = await req.close();
      if (resp.statusCode != 200) {
        throw HttpException('HTTP ${resp.statusCode}');
      }
      final total = resp.contentLength;
      final sink = file.openWrite();
      var got = 0;
      try {
        await for (final chunk in resp) {
          got += chunk.length;
          sink.add(chunk);
          if (total > 0) progress.value = (got / total).clamp(0.0, 1.0);
        }
      } finally {
        await sink.close();
      }
    } finally {
      client.close();
    }
  }

  /// Hand the completed package to Windows or Android's installer.
  Future<void> install() {
    final active = _installInFlight;
    if (active != null) return active;

    late final Future<void> flight;
    flight = _installImpl().whenComplete(() {
      if (identical(_installInFlight, flight)) _installInFlight = null;
    });
    _installInFlight = flight;
    return flight;
  }

  Future<void> _installImpl() async {
    final path = _downloadedPath;
    final asset = _downloadedAsset;
    final version = info?.version;
    if (path == null || asset == null) {
      error.value = '请先下载有效的安装包';
      status.value = UpdateStatus.error;
      return;
    }
    try {
      final integrityError = await _packageIntegrityError(File(path), asset);
      if (integrityError != null) {
        error.value = '$integrityError，请重新下载';
        status.value = UpdateStatus.error;
        return;
      }
      status.value = UpdateStatus.installing;
      error.value = null;
      final result = _installHandler == null
          ? await _launchInstaller(path)
          : await _installHandler(path, _platformKind);
      if (result == UpdateInstallResult.permissionRequired) {
        error.value = '请先在系统设置中允许安装未知来源应用，再点击重试安装';
        status.value = UpdateStatus.downloaded;
        return;
      }
      if (result == UpdateInstallResult.retryableFailure) {
        error.value = '安装器已取消或未能完成，安装包已保留，可重试安装';
        status.value = UpdateStatus.downloaded;
        return;
      }
      if (_platform == 'windows') {
        // The Windows helper waits for the installer. A clean exit means the
        // new package was applied; don't expose a second Install button.
        if (version != null) localVersion.value = version;
        status.value = UpdateStatus.upToDate;
      } else if (_platform == 'android') {
        status.value = UpdateStatus.idle;
      }
    } catch (e) {
      error.value = '$e';
      status.value = UpdateStatus.error;
    }
  }

  UpdatePlatformKind get _platformKind => switch (_platform) {
    'windows' => UpdatePlatformKind.windows,
    'android' => UpdatePlatformKind.android,
    _ => UpdatePlatformKind.unsupported,
  };

  /// Arguments for the Windows elevation helper. The path is base64 encoded so
  /// PowerShell never interpolates user-controlled quotes or metacharacters.
  @visibleForTesting
  static List<String> windowsInstallerProcessArguments(String path) {
    final encodedPath = base64Encode(utf8.encode(path));
    final script =
        r'$installerPath = [System.Text.Encoding]::UTF8.GetString([System.Convert]::FromBase64String("' +
        encodedPath +
        r'")); try { $installer = Start-Process -FilePath $installerPath -ArgumentList @("/NORESTART") -Verb RunAs -PassThru -ErrorAction Stop; if ($null -eq $installer) { exit 255 }; $installer.WaitForExit(); exit $installer.ExitCode } catch { exit 255 }';
    return ['-NoProfile', '-WindowStyle', 'Hidden', '-Command', script];
  }

  Future<UpdateInstallResult> _launchInstaller(String path) async {
    if (_platform == 'windows') {
      // ShellExecute via Start-Process is needed to show UAC for the Inno Setup
      // installer, which declares administrative privileges. WaitForExit on
      // the returned Process covers the installer alone: Start-Process -Wait
      // waits for descendants too, including the post-install app launched by
      // setup. The helper uses exit 255 for start errors/UAC cancellation and
      // forwards the installer's own exit code otherwise.
      final result = await Process.run(
        'powershell.exe',
        windowsInstallerProcessArguments(path),
      );
      return windowsInstallerResultForExitCode(result.exitCode);
    }
    if (_platform == 'android') {
      return await _invokeAndroidInstall(path)
          ? UpdateInstallResult.started
          : UpdateInstallResult.permissionRequired;
    }
    throw UnsupportedError('当前平台不支持应用内更新');
  }

  /// Treat any Windows installer non-zero exit as retryable while keeping the
  /// verified package. Inno Setup reports cancellation and failures with
  /// non-zero codes; 255 is reserved by the helper for start/UAC errors.
  @visibleForTesting
  static UpdateInstallResult windowsInstallerResultForExitCode(int exitCode) =>
      exitCode == 0
      ? UpdateInstallResult.started
      : UpdateInstallResult.retryableFailure;

  Future<bool> _invokeAndroidInstall(String path) async {
    const channel = MethodChannel('rch/updater');
    try {
      return await channel.invokeMethod<bool>('installApk', {'path': path}) ??
          false;
    } on PlatformException catch (e) {
      if (e.code == 'unknown_sources') {
        return false;
      }
      rethrow;
    }
  }
}
