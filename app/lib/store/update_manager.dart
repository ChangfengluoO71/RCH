import 'dart:async';
import 'dart:convert';
import 'dart:io';

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

  UpdateInfo? info;
  String? _downloadedPath;
  bool _initDone = false;

  /// 用户选择的镜像前缀（来自设置；可为自定义地址）。
  String get mirrorPrefix {
    final raw = LibraryStore.instance.settings.updateMirror.trim();
    if (raw.isEmpty) return '';
    return raw.endsWith('/') ? raw : '$raw/';
  }

  /// 生效镜像列表：远端拉取（已持久化）在前，内置预设兜底；按 URL 去重。
  List<MapEntry<String, String>> get effectiveMirrors {
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
          jsonDecode(LibraryStore.instance.settings.updateMirrorList)
          as List;
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
      final client = HttpClient()..connectionTimeout = const Duration(seconds: 15);
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
      String selected, List<MapEntry<String, String>> mirrors) {
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
  /// 按「当前选择 → 其余镜像」顺序尝试，单个通道失败自动切换下一个；
  /// 全部失败才报错，错误信息里带尝试过的通道列表。
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
      final candidates = downloadCandidates(mirrorPrefix, effectiveMirrors);
      final tried = <String>[];
      Object? lastErr;
      for (final mirror in candidates) {
        final label = mirror.isEmpty ? '官方直连' : mirror;
        tried.add(label);
        try {
          await _downloadVia(file, i, mirror);
          _downloadedPath = file.path;
          status.value = UpdateStatus.downloaded;
          return;
        } catch (e) {
          lastErr = e;
          if (file.existsSync()) {
            try {
              file.deleteSync();
            } catch (_) {}
          }
          if (candidates.length > 1) {
            error.value = '通道「$label」失败，自动切换下一个…';
          }
        }
      }
      throw HttpException(
          '全部下载通道失败（已尝试：${tried.join('、')}）。$lastErr');
    } catch (e) {
      error.value = '$e';
      status.value = UpdateStatus.error;
    }
  }

  Future<void> _downloadVia(File file, UpdateInfo i, String mirror) async {
    final client = HttpClient()..connectionTimeout = const Duration(seconds: 30);
    try {
      final req =
          await client.getUrl(Uri.parse(buildDownloadUrl(i.asset.url, mirror)));
      req.headers.set(HttpHeaders.userAgentHeader, 'RCH-Updater');
      final resp = await req.close();
      if (resp.statusCode != 200) {
        throw HttpException('HTTP ${resp.statusCode}');
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
      if (i.asset.size > 0 && file.lengthSync() != i.asset.size) {
        throw const FileSystemException('安装包大小校验失败');
      }
    } finally {
      client.close();
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
