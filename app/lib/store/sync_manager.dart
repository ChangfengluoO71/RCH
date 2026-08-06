// 备份/同步管理（P2）：双通道 push/pull、恢复、定时同步与状态。

import 'dart:async';
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
const _kInterval = 'sync_interval_minutes';
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
  int intervalMinutes = 0;
  int lastAt = 0;
  String lastStatus = '尚未同步';
  int ignoredCopies = 0;
  bool busy = false;
  bool crossDeviceSearch = true;
  BigInt? _webdavSession;

  /// 设备 id → 名称（幽灵书源来源展示）。
  final Map<String, String> deviceNames = {};

  Timer? _timer;

  /// 应用启动时调用：加载配置；定时开启（间隔 > 0）时启动即拉取一次。
  ///
  /// 定时同步逻辑（备注）：
  /// 1. 前提：模式非 off 且 `intervalMinutes > 0`，否则只保留手动按钮。
  /// 2. 启动时先 `pullNow()` 一次（不等待首个周期），随后 `_restartTimer()`
  ///    用 `Timer.periodic` 每 N 分钟触发 `autoSync()`。
  /// 3. 每次 `autoSync()` = 先拉取（LWW 合并远端包）→ 再推送（自游标增量导出）。
  /// 4. 防重入：同步中（busy）新触发的周期直接跳过，不叠加、不排队。
  /// 5. 失败只写入 `lastStatus`，不影响后续周期；改模式/间隔会重建定时器。
  Future<void> init() async {
    await load();
    _restartTimer();
    if (mode != SyncMode.off && intervalMinutes > 0) {
      unawaited(pullNow());
    }
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
      intervalMinutes = int.tryParse(map[_kInterval] ?? '') ?? 0;
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
    await dbSaveSetting(key: _kInterval, value: '$intervalMinutes');
    await dbSaveSetting(key: _kLastAt, value: '$lastAt');
    await dbSaveSetting(key: _kLastStatus, value: lastStatus);
    await dbSaveSetting(key: _kCrossDeviceSearch, value: '$crossDeviceSearch');
    notifyListeners();
  }

  Future<void> setMode(SyncMode m) async {
    mode = m;
    _restartTimer();
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

  Future<void> setInterval(int minutes) async {
    intervalMinutes = minutes;
    _restartTimer();
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

  /// 定时任务：先拉远端（LWW 合并），再推本地（增量 + 归档）。
  ///
  /// 顺序原因：先把其他设备的变更吸收进本地，再以本地为准推增量；
  /// 拉取引入的行会因 `updated_at` 较新而自然进入下次增量，属于无害回显。
  Future<void> autoSync() async {
    await pullNow();
    await pushNow();
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

  void _restartTimer() {
    _timer?.cancel();
    _timer = null;
    if (mode != SyncMode.off && intervalMinutes > 0) {
      _timer = Timer.periodic(
        Duration(minutes: intervalMinutes),
        (_) => autoSync(),
      );
      debugPrint(
        '[SyncManager] 定时同步已开启：每 $intervalMinutes 分钟（拉取→推送）',
      );
    }
  }
}
