// 自动同步引擎（ADR-024 Phase 4）。
//
// 触发：本地变更防抖（2s）、启动、定时（60s）、手动立即同步。
// 传输：WebDAV Sync State（复用 SyncManager 的 WebDAV 配置）。

import 'dart:async';
import 'dart:io';

import 'package:app/src/rust/api/db.dart' as dbapi;
import 'package:app/src/rust/api/source.dart';
import 'package:app/src/rust/api/sync.dart' as syncapi;
import 'package:app/store/library_index_service.dart';
import 'package:app/store/library_store.dart';
import 'package:app/store/library_catalog.dart';
import 'package:app/store/sync_manager.dart';
import 'package:flutter/foundation.dart';

class SyncEngine {
  SyncEngine._();
  static final SyncEngine instance = SyncEngine._();

  static const _kAutoSync = 'sync_auto';

  Timer? _debounce;
  Timer? _poll;
  bool _syncing = false;
  bool _enabled = true;
  int _retryCount = 0;
  DateTime? _rateLimitUntil;
  DateTime? _backoffUntil;
  DateTime? _lastAttemptAt;

  /// 自动触发最小间隔：防止本地变更防抖+轮询叠加高频请求。
  static const _minAutoInterval = Duration(seconds: 15);
  /// 失败指数退避上限。
  static const _maxBackoff = Duration(minutes: 15);

  bool get autoSync => _enabled;
  String lastStatus = '尚未同步';
  int lastAt = 0;

  /// 启动：读开关、注册变更监听与定时轮询；由 main 调用。
  Future<void> init() async {
    try {
      final entries = await dbapi.dbLoadAllSettings();
      // 默认开启：设置缺失（首次安装）视为开启；显式 'false' 才关闭。
      final auto = entries.where((e) => e.key == _kAutoSync);
      _enabled = auto.isEmpty ? true : auto.first.value != 'false';
    } catch (_) {
      _enabled = true; // 默认开启
    }
    LibraryStore.instance.addListener(markDirty);
    _poll?.cancel();
    // ADR-028 §12.6：轮询走轻量 revision 检查，远端未变化不触发全量同步。
    _poll = Timer.periodic(const Duration(seconds: 60), (_) => _autoTrigger(quickPoll: true));
    if (_enabled) await syncNow();
  }

  Future<void> setAutoSync(bool v) async {
    _enabled = v;
    try {
      await dbapi.dbSaveSetting(key: _kAutoSync, value: '$v');
    } catch (_) {}
  }

  /// 本地数据变化 → 防抖合并为一批。
  void markDirty() {
    if (!_enabled || _syncing) return;
    _debounce?.cancel();
    _debounce = Timer(const Duration(seconds: 2), _autoTrigger);
  }

  Future<void> _autoTrigger({bool fromBackoff = false, bool quickPoll = false}) async {
    if (!_enabled || _syncing) return;
    if (_inRateLimitCooldown()) return;
    // 失败退避期间，定时轮询/变更防抖都不得触发新一轮同步；
    // 否则每 60 秒盲重试会把服务端打到限流（截图中的每分钟失败循环）。
    if (_inBackoffCooldown()) return;
    // 失败后重试节奏完全交给退避定时器（fromBackoff），轮询/防抖让位。
    if (!fromBackoff && _retryCount > 0) return;
    final last = _lastAttemptAt;
    if (last != null && DateTime.now().difference(last) < _minAutoInterval) return;
    if (quickPoll && await _remoteUnchanged()) return;
    await syncNow();
  }

  bool _inRateLimitCooldown() {
    final until = _rateLimitUntil;
    return until != null && DateTime.now().isBefore(until);
  }

  bool _inBackoffCooldown() {
    final until = _backoffUntil;
    return until != null && DateTime.now().isBefore(until);
  }

  /// 轻量轮询：远端 manifest revision 与本机一致 → 无变化，跳过全量同步。
  /// 本机从未同步成功（revision=0）或读取失败时返回 false（必须全量走一次）。
  Future<bool> _remoteUnchanged() async {
    try {
      final mgr = SyncManager.instance;
      final url = mgr.webdavUrl.trim();
      if (url.isEmpty) return true;
      final st = await syncapi.syncStatus();
      if (st.revision <= 0) return false;
      final session = (await webdavConnect(
        url: url,
        username: mgr.webdavUsername.trim(),
        password: mgr.webdavPassword,
      ))
          .id;
      try {
        final dir = mgr.webdavDir.trim().isEmpty ? 'RCH/sync' : mgr.webdavDir.trim();
        final rev = await syncapi.syncRemoteRevision(session: session, dir: dir);
        return rev == st.revision;
      } finally {
        await webdavDisconnect(id: session);
      }
    } catch (_) {
      return false;
    }
  }

  /// 立即同步（手动入口与自动共用）。
  Future<String> syncNow() async {
    final mgr = SyncManager.instance;
    final url = mgr.webdavUrl.trim();
    if (url.isEmpty) return '未配置 WebDAV 同步';
    if (_syncing) return '正在同步…';
    if (_inRateLimitCooldown()) return '服务器限流中，15 分钟后自动重试';
    _syncing = true;
    _lastAttemptAt = DateTime.now();
    BigInt? session;
    try {
      // 同步前为本地书源生成/增量刷新 library_index（L2 目录索引随同步包走；
      // 首次全量、之后按目录 mtime 增量；云端源由用户"重建离线索引"触发）。
      for (final s in LibraryStore.instance.sources) {
        try {
          if (s.isLocalFs) {
            await LibraryIndexService.instance.refreshSourceIndex(source: s, force: false);
          } else {
            // ADR-029：云端源同步前把本地浏览快照补进离线索引（零网络），
            // 让"浏览/触及过"的书随同步传播到其他设备。
            await LibraryIndexService.buildIndexFromSnapshots(s);
          }
        } catch (e) {
          debugPrint('[SyncEngine] 书源索引刷新失败 ${s.name}: $e');
        }
      }
      session = (await webdavConnect(
        url: url,
        username: mgr.webdavUsername.trim(),
        password: mgr.webdavPassword,
      ))
          .id;
      final dir = mgr.webdavDir.trim().isEmpty ? 'RCH/sync' : mgr.webdavDir.trim();
      final out = await syncapi.syncNow(
        session: session,
        dir: dir,
        platform: Platform.operatingSystem,
      );
      lastAt = DateTime.now().millisecondsSinceEpoch;
      final counts = out.changedEntities
          .map((e) => '${e.entity}=${e.count}')
          .join(' ');
      lastStatus = '同步完成 v${out.revision}${counts.isEmpty ? '' : '（$counts）'}';
      await syncapi.syncClearLastError();
      _retryCount = 0;
      _rateLimitUntil = null;
      _backoffUntil = null;
      // 刷新 UI 内存态（apply 发生在 Rust 侧；重载会触发 notify，
      // 但此时 _syncing=true，markDirty 直接忽略，不会造成循环触发）。
      // ADR-028 §12.5：apply 发生在 Rust 侧，Dart 内存态必须强制重载
      // （默认 load 有 _loaded 短路，直接调用不会刷新）。
      await LibraryStore.instance.load(force: true);
      await LibraryCatalogStore.instance.loadTree();
      return lastStatus;
    } catch (e) {
      lastStatus = '同步失败: $e';
      final msg = '$e';
      if (msg.contains('503') || msg.contains('429') || msg.contains('Too many requests')) {
        // 坚果云等限流：进入 15 分钟冷却，不短周期重试轰炸。
        _rateLimitUntil = DateTime.now().add(const Duration(minutes: 15));
        _retryCount = 0;
      } else {
        _scheduleBackoffRetry();
      }
      try {
        await syncapi.syncSetLastError(message: '$e');
      } catch (_) {}
      return lastStatus;
    } finally {
      if (session != null) {
        try {
          await webdavDisconnect(id: session);
        } catch (_) {}
      }
      _syncing = false;
    }
  }

  /// 失败指数退避重试：30s → 1m → 2m → 4m → 8m → 15m（封顶），成功重置。
  void _scheduleBackoffRetry() {
    if (!_enabled) return;
    _retryCount++;
    final exp = (_retryCount - 1).clamp(0, 5).toInt();
    final backoffMs =
        (30000 * (1 << exp)).clamp(0, _maxBackoff.inMilliseconds).toInt();
    final delay = Duration(milliseconds: backoffMs);
    _backoffUntil = DateTime.now().add(delay);
    lastStatus = '同步失败，${delay.inSeconds} 秒后重试';
    _debounce?.cancel();
    _debounce = Timer(delay, () => _autoTrigger(fromBackoff: true));
  }

  /// 设置页状态文本。
  Future<String> statusText() async {
    try {
      final st = await syncapi.syncStatus();
      if (st.lastError.isNotEmpty) return '上次同步失败: ${st.lastError}';
      if (!st.initialized) return '尚未同步';
      final t = DateTime.fromMillisecondsSinceEpoch(st.lastSyncAt).toLocal();
      final s = t.toString();
      return '上次同步 ${s.substring(0, s.length > 19 ? 19 : s.length)}（v${st.revision}）';
    } catch (_) {
      return lastStatus;
    }
  }
}
