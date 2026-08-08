// 备份/同步管理（P2）：双通道 push/pull、恢复与状态（手动触发）。

import 'dart:io';

import 'package:app/src/rust/api/db.dart';
import 'package:app/src/rust/api/package.dart';
import 'package:app/src/rust/api/source.dart';
import 'package:app/store/sync_paths.dart';
import 'package:flutter/foundation.dart';

enum SyncMode {
  off('关闭'),
  folder('同步盘目录'),
  webdav('WebDAV');

  const SyncMode(this.label);
  final String label;
}

const _kMode = 'sync_mode';
const _kDir = 'sync_dir';
const _kWebdavUrl = 'sync_webdav_url';
const _kWebdavUsername = 'sync_webdav_username';
const _kWebdavPassword = 'sync_webdav_password';
const _kWebdavDir = 'sync_webdav_dir';
const _kLastAt = 'sync_last_at';
const _kLastStatus = 'sync_last_status';
const _kCrossDeviceSearch = 'sync_cross_device_search';

/// 同步管理器（单例）：配置存 app_settings，同步包走 `.rchpkg` 标准格式。
///
/// P2 阶段拉取为"包覆盖本地"语义（按行 upsert），合并引擎在 P3。
class SyncManager extends ChangeNotifier {
  SyncManager._();
  static final SyncManager instance = SyncManager._();

  SyncMode mode = SyncMode.off;
  String dir = '';
  String webdavUrl = '';
  String webdavUsername = '';
  String webdavPassword = '';
  String webdavDir = 'RCH/sync';
  int lastAt = 0;
  String lastStatus = '尚未同步';
  int ignoredCopies = 0;
  bool busy = false;
  bool crossDeviceSearch = true;
  BigInt? _webdavSession;

  /// 设备 id → 名称（幽灵书源来源展示）。
  final Map<String, String> deviceNames = {};

  /// 应用启动时调用：加载同步配置（当前仅手动同步，定时逻辑已移除）。
  Future<void> init() async {
    await load();
  }

  Future<void> load() async {
    try {
      final entries = await dbLoadAllSettings();
      final map = {for (final e in entries) e.key: e.value};
      mode = SyncMode.values.firstWhere(
        (m) => m.name == map[_kMode],
        orElse: () => SyncMode.off,
      );
      dir = map[_kDir] ?? '';
      webdavUrl = map[_kWebdavUrl] ?? '';
      webdavUsername = map[_kWebdavUsername] ?? '';
      webdavPassword = map[_kWebdavPassword] ?? '';
      webdavDir = map[_kWebdavDir] ?? 'RCH/sync';
      _webdavSession = null;
      lastAt = int.tryParse(map[_kLastAt] ?? '') ?? 0;
      lastStatus = map[_kLastStatus] ?? '尚未同步';
      crossDeviceSearch = map[_kCrossDeviceSearch] != 'false';
      deviceNames.clear();
      for (final d in await dbListDevices()) {
        deviceNames[d.id] = d.name;
      }
    } catch (e) {
      debugPrint('[SyncManager] load failed: $e');
    }
    notifyListeners();
  }

  Future<void> save() async {
    await dbSaveSetting(key: _kMode, value: mode.name);
    await dbSaveSetting(key: _kDir, value: dir);
    await dbSaveSetting(key: _kWebdavUrl, value: webdavUrl);
    await dbSaveSetting(key: _kWebdavUsername, value: webdavUsername);
    await dbSaveSetting(key: _kWebdavPassword, value: webdavPassword);
    await dbSaveSetting(key: _kWebdavDir, value: webdavDir);
    await dbSaveSetting(key: _kLastAt, value: '$lastAt');
    await dbSaveSetting(key: _kLastStatus, value: lastStatus);
    await dbSaveSetting(key: _kCrossDeviceSearch, value: '$crossDeviceSearch');
    notifyListeners();
  }

  Future<void> setMode(SyncMode m) async {
    mode = m;
    await save();
  }

  Future<void> setDir(String d) async {
    dir = d;
    await save();
  }

  Future<void> setWebdavConfig({
    required String url,
    required String username,
    required String password,
    required String dir,
  }) async {
    webdavUrl = url;
    webdavUsername = username;
    webdavPassword = password;
    webdavDir = dir;
    _webdavSession = null;
    await save();
  }

  Future<void> setCrossDeviceSearch(bool v) async {
    crossDeviceSearch = v;
    await save();
  }

  /// 幽灵书源来源设备显示名。
  String deviceNameOf(String? deviceId) {
    if (deviceId == null || deviceId.isEmpty) return '其他设备';
    return deviceNames[deviceId] ?? '其他设备';
  }

  /// 推送本地数据到同步目标（增量）。
  Future<String> pushNow() async {
    if (busy) return '正在同步，请稍候';
    busy = true;
    notifyListeners();
    try {
      final msg = switch (mode) {
        SyncMode.folder => await _pushFolder(),
        SyncMode.webdav => await _pushWebdav(),
        SyncMode.off => '未启用同步',
      };
      await _finish(msg);
      return msg;
    } catch (e) {
      final msg = '推送失败: $e';
      await _finish(msg);
      return msg;
    } finally {
      busy = false;
      notifyListeners();
    }
  }

  /// 从同步目标拉取并导入。
  Future<String> pullNow() async {
    if (busy) return '正在同步，请稍候';
    busy = true;
    notifyListeners();
    try {
      final msg = switch (mode) {
        SyncMode.folder => await _pullFolder(),
        SyncMode.webdav => await _pullWebdav(),
        SyncMode.off => '未启用同步',
      };
      await _finish(msg);
      return msg;
    } catch (e) {
      final msg = '拉取失败: $e';
      await _finish(msg);
      return msg;
    } finally {
      busy = false;
      notifyListeners();
    }
  }

  /// 从任意 `.rchpkg` 文件恢复（保留本地书源凭据）。
  Future<String> restoreFrom(String path) async {
    if (busy) return '正在同步，请稍候';
    busy = true;
    notifyListeners();
    try {
      final stats = await rchpkgImport(path: path, force: true);
      final msg =
          '恢复成功（${stats.sources.toInt()} 书源 / ${stats.metas.toInt()} 详情 / ${stats.tags.toInt()} 标签）'
          '${stats.ghosts.toInt() > 0 ? '，其中 ${stats.ghosts.toInt()} 个为其他设备书源' : ''}';
      await _finish(msg);
      return msg;
    } catch (e) {
      final msg = '恢复失败: $e';
      await _finish(msg);
      return msg;
    } finally {
      busy = false;
      notifyListeners();
    }
  }

  /// 从任意 `.rchpkg` 文件恢复，并应用包内的加密书源凭据（需导出时设置的口令）。
  Future<String> restoreFromWithCredentials(String path, String passphrase) async {
    if (busy) return '正在同步，请稍候';
    busy = true;
    notifyListeners();
    try {
      final stats = await rchpkgImportWithCredentials(path: path, passphrase: passphrase);
      final msg =
          '恢复成功（${stats.sources.toInt()} 书源 / ${stats.metas.toInt()} 详情 / ${stats.tags.toInt()} 标签）· 加密凭据已应用';
      await _finish(msg);
      return msg;
    } catch (e) {
      final msg = '恢复失败: $e';
      await _finish(msg);
      return msg;
    } finally {
      busy = false;
      notifyListeners();
    }
  }

  /// 导出标准 `.rchpkg` 包到任意文件（全量快照，便于离线备份 / 手动传机）。
  ///
  /// `passphrase` 为空时导出不含敏感凭据的包；非空时附带 AES-256-GCM
  /// 加密的书源凭据分块（导入时需同一口令）。
  Future<String> exportToFile(String path, {String passphrase = ''}) async {
    if (busy) return '正在同步，请稍候';
    busy = true;
    notifyListeners();
    try {
      final info = await rchpkgExportSnapshot(path: path, passphrase: passphrase);
      final msg = '导出成功：${info.sources.toInt()} 书源 / ${info.metas.toInt()} 详情 / '
          '${info.tags.toInt()} 标签${passphrase.isNotEmpty ? '（含加密凭据）' : ''}';
      await _finish(msg);
      return msg;
    } catch (e) {
      final msg = '导出失败: $e';
      await _finish(msg);
      return msg;
    } finally {
      busy = false;
      notifyListeners();
    }
  }

  /// 清理归档副本（archive/ 下全部 .rchpkg，保留当前 latest.rchpkg）。
  Future<({int deleted, String message})> cleanArchives() async {
    if (busy) return (deleted: 0, message: '正在同步，请稍候');
    busy = true;
    notifyListeners();
    try {
      return switch (mode) {
        SyncMode.folder => await _cleanFolderArchives(),
        SyncMode.webdav => await _cleanWebdavArchives(),
        SyncMode.off => (deleted: 0, message: '未启用同步'),
      };
    } catch (e) {
      return (deleted: 0, message: '清理失败: $e');
    } finally {
      busy = false;
      notifyListeners();
    }
  }

  Future<({int deleted, String message})> _cleanFolderArchives() async {
    final archiveDir = Directory(syncArchiveDir(dir));
    if (!await archiveDir.exists()) return (deleted: 0, message: '没有归档副本');
    var n = 0;
    await for (final e in archiveDir.list()) {
      if (e is File && e.path.toLowerCase().endsWith('.rchpkg')) {
        await e.delete();
        n++;
      }
    }
    return (deleted: n, message: '已清理 $n 个归档副本');
  }

  Future<({int deleted, String message})> _cleanWebdavArchives() async {
    final session = await _webdavSessionFor();
    final dir = normalizeRemoteDir(webdavDir.isEmpty ? 'RCH/sync' : webdavDir);
    final archiveDir = '$dir/archive';
    final entries = await webdavList(session: session, path: archiveDir);
    var n = 0;
    for (final e in entries) {
      if (!e.isDir && e.name.toLowerCase().endsWith('.rchpkg')) {
        await webdavDeleteFile(session: session, path: e.path);
        n++;
      }
    }
    return (deleted: n, message: '已清理 $n 个远程归档副本');
  }

  Future<String> _pushFolder() async {
    if (dir.isEmpty) throw '未设置同步目录';
    final d = Directory(dir);
    if (!await d.exists()) await d.create(recursive: true);
    final tmp = '${syncLatestPath(dir)}.tmp';
    final info = await rchpkgExport(path: tmp, incremental: true);
    await File(tmp).rename(syncLatestPath(dir));
    final ts = formatSyncTimestamp(
      DateTime.fromMillisecondsSinceEpoch(info.createdAt.toInt()),
    );
    final archive = Directory(syncArchiveDir(dir));
    if (!await archive.exists()) await archive.create(recursive: true);
    await File(syncLatestPath(dir)).copy(syncArchivePath(dir, ts));
    _scanIgnored(dir);
    return '推送成功（${info.sources.toInt()} 书源 / ${info.metas.toInt()} 详情 / ${info.tags.toInt()} 标签）';
  }

  Future<String> _pushWebdav() async {
    final session = await _webdavSessionFor();
    final dir = normalizeRemoteDir(webdavDir.isEmpty ? 'RCH/sync' : webdavDir);
    for (final level in remoteDirLevels(dir)) {
      await webdavMakeDir(session: session, path: level);
    }
    final tmp = File(
      '${Directory.systemTemp.path}${Platform.pathSeparator}rch_push_${DateTime.now().millisecondsSinceEpoch}.rchpkg',
    );
    try {
      final info = await rchpkgExport(path: tmp.path, incremental: true);
      final bytes = await tmp.readAsBytes();
      await webdavUploadFile(
        session: session,
        path: remoteJoin(dir, kSyncLatestName),
        data: bytes,
      );
      return '推送成功（${info.sources.toInt()} 书源 / ${info.metas.toInt()} 详情 / ${info.tags.toInt()} 标签）';
    } finally {
      if (await tmp.exists()) await tmp.delete();
    }
  }

  Future<String> _pullFolder() async {
    if (dir.isEmpty) throw '未设置同步目录';
    final latest = syncLatestPath(dir);
    if (!await File(latest).exists()) return '目录中没有可拉取的包';
    _scanIgnored(dir);
    final stats = await rchpkgImport(path: latest, force: false);
    return '拉取成功（${stats.sources.toInt()} 书源 / ${stats.metas.toInt()} 详情 / ${stats.tags.toInt()} 标签）'
        '${stats.ghosts.toInt() > 0 ? '，含 ${stats.ghosts.toInt()} 个其他设备书源' : ''}';
  }

  Future<String> _pullWebdav() async {
    final session = await _webdavSessionFor();
    final dir = normalizeRemoteDir(webdavDir.isEmpty ? 'RCH/sync' : webdavDir);
    final bytes = await webdavDownloadFile(
      session: session,
      path: remoteJoin(dir, kSyncLatestName),
    );
    if (bytes.isEmpty) return '远程没有可拉取的包';
    final tmp = File(
      '${Directory.systemTemp.path}${Platform.pathSeparator}rch_pull_${DateTime.now().millisecondsSinceEpoch}.rchpkg',
    );
    try {
      await tmp.writeAsBytes(bytes);
      final stats = await rchpkgImport(path: tmp.path, force: false);
      return '拉取成功（${stats.sources.toInt()} 书源 / ${stats.metas.toInt()} 详情 / ${stats.tags.toInt()} 标签）'
          '${stats.ghosts.toInt() > 0 ? '，含 ${stats.ghosts.toInt()} 个其他设备书源' : ''}';
    } finally {
      if (await tmp.exists()) await tmp.delete();
    }
  }

  Future<BigInt> _webdavSessionFor() async {
    if (_webdavSession != null) return _webdavSession!;
    final url = webdavUrl.trim();
    if (url.isEmpty) throw '未填写 WebDAV 地址';
    final s = await webdavConnect(
      url: url,
      username: webdavUsername.trim(),
      password: webdavPassword,
    );
    _webdavSession = s.id;
    return s.id;
  }

  /// 测试 WebDAV 连接（使用当前配置建立会话）。
  Future<String> testWebdavConnection() async {
    try {
      await _webdavSessionFor();
      return '连接成功';
    } catch (e) {
      return '连接失败: $e';
    }
  }

  void _scanIgnored(String dirPath) {
    try {
      final d = Directory(dirPath);
      if (!d.existsSync()) {
        ignoredCopies = 0;
        return;
      }
      final names = d
          .listSync()
          .map((e) => e.path.split(Platform.pathSeparator).last)
          .toList();
      ignoredCopies = countIgnoredSyncFiles(names);
    } catch (_) {
      ignoredCopies = 0;
    }
  }

  Future<void> _finish(String msg) async {
    lastStatus = msg;
    lastAt = DateTime.now().millisecondsSinceEpoch;
    try {
      await save();
    } catch (e) {
      debugPrint('[SyncManager] 状态保存失败: $e');
    }
  }

}
