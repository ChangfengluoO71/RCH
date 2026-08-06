// 备份/同步管理（P2）：双通道 push/pull、恢复、定时同步与状态。

import 'dart:async';
import 'dart:io';

import 'package:app/src/rust/api/db.dart';
import 'package:app/src/rust/api/package.dart';
import 'package:app/src/rust/api/source.dart';
import 'package:app/store/library_store.dart';
import 'package:app/store/models.dart';
import 'package:app/store/sync_paths.dart';
import 'package:app/store/webdav_session.dart';
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
const _kWebdavSourceId = 'sync_webdav_source_id';
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
  String webdavSourceId = '';
  int intervalMinutes = 0;
  int lastAt = 0;
  String lastStatus = '尚未同步';
  int ignoredCopies = 0;
  bool busy = false;
  bool crossDeviceSearch = true;

  /// 设备 id → 名称（幽灵书源来源展示）。
  final Map<String, String> deviceNames = {};

  Timer? _timer;

  /// 应用启动时调用：加载配置；定时开启时启动即拉取一次。
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
      webdavSourceId = map[_kWebdavSourceId] ?? '';
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
    await dbSaveSetting(key: _kWebdavSourceId, value: webdavSourceId);
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

  Future<void> setWebdavSourceId(String id) async {
    webdavSourceId = id;
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

  /// 定时任务：先拉远端，再推本地。
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
    final source = await _webdavSource();
    if (source == null) throw '未选择 WebDAV 书源';
    final session = await webdavSessionFor(source);
    await webdavMakeDir(session: session, path: remoteRchDir(source.path));
    await webdavMakeDir(session: session, path: remoteSyncDir(source.path));
    final tmp = File(
      '${Directory.systemTemp.path}${Platform.pathSeparator}rch_push_${DateTime.now().millisecondsSinceEpoch}.rchpkg',
    );
    try {
      final info = await rchpkgExport(path: tmp.path, incremental: true);
      final bytes = await tmp.readAsBytes();
      await webdavUploadFile(
        session: session,
        path: remoteLatestPath(source.path),
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
    final source = await _webdavSource();
    if (source == null) throw '未选择 WebDAV 书源';
    final session = await webdavSessionFor(source);
    final bytes = await webdavDownloadFile(
      session: session,
      path: remoteLatestPath(source.path),
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

  Future<BookSource?> _webdavSource() async {
    final sources = LibraryStore.instance.sources;
    if (webdavSourceId.isNotEmpty) {
      for (final s in sources) {
        if (s.id == webdavSourceId && s.isWebDav) return s;
      }
    }
    for (final s in sources) {
      if (s.isWebDav) return s;
    }
    return null;
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
    }
  }
}
