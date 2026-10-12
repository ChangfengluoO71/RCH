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
import 'package:app/store/foreground_work_coordinator.dart';
import 'package:app/store/sync_manager.dart';
import 'package:flutter/foundation.dart';

enum SyncRunStatus { completed, remoteUnchanged, skipped, failed }

class SyncRunOutcome {
  const SyncRunOutcome({
    required this.status,
    required this.message,
    this.revision,
    this.changedEntities = const <String, int>{},
    this.changesKnown = true,
  });

  final SyncRunStatus status;
  final String message;
  final int? revision;
  final Map<String, int> changedEntities;
  final bool changesKnown;

  bool get hasChanges => changedEntities.values.any((count) => count > 0);
}

class SyncEngine {
  SyncEngine._();
  static final SyncEngine instance = SyncEngine._();

  static const _kAutoSync = 'sync_auto';

  Timer? _debounce;
  Timer? _poll;
  bool _syncing = false;
  bool _automaticTriggerRunning = false;
  bool _enabled = true;
  bool _managedByCoordinator = false;
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
  Future<void> init({bool managedByCoordinator = false}) async {
    _managedByCoordinator = managedByCoordinator;
    try {
      final entries = await dbapi.dbLoadAllSettings();
      // 默认开启：设置缺失（首次安装）视为开启；显式 'false' 才关闭。
      final auto = entries.where((e) => e.key == _kAutoSync);
      _enabled = auto.isEmpty ? true : auto.first.value != 'false';
    } catch (_) {
      _enabled = true; // 默认开启
    }
    if (!managedByCoordinator) {
      LibraryStore.instance.addListener(markDirty);
    }
    _poll?.cancel();
    _debounce?.cancel();
    if (managedByCoordinator) return;
    // ADR-028 §12.6：轮询走轻量 revision 检查，远端未变化不触发全量同步。
    _poll = Timer.periodic(
      const Duration(seconds: 60),
      (_) => _autoTrigger(quickPoll: true),
    );
    if (_enabled) await syncNow(background: true);
  }

  Future<void> setAutoSync(bool v) async {
    _enabled = v;
    try {
      await dbapi.dbSaveSetting(key: _kAutoSync, value: '$v');
    } catch (_) {}
  }

  /// 本地数据变化 → 防抖合并为一批。
  void markDirty() {
    if (_managedByCoordinator || !_enabled || _syncing) return;
    _debounce?.cancel();
    _debounce = Timer(const Duration(seconds: 2), _autoTrigger);
  }

  /// Coordinator entry point for the lightweight periodic revision check.
  /// It is intentionally separate from [syncNow] so a timer does not perform
  /// a full WebDAV sync when the remote revision is unchanged.
  Future<void> triggerAutomatic({
    bool quickPoll = false,
    bool refreshCatalog = true,
  }) => _autoTrigger(quickPoll: quickPoll, refreshCatalog: refreshCatalog);

  /// Performs the cheap remote revision check before local catalog scanning.
  /// A null result means a full sync should follow; a typed result means the
  /// periodic attempt is safely skipped or the remote revision is unchanged.
  Future<SyncRunOutcome?> probeQuickPoll({bool background = false}) async {
    if (background) {
      await ForegroundWorkCoordinator.instance.waitForMaintenanceTurn();
    }
    if (!_enabled || _syncing) {
      return SyncRunOutcome(
        status: SyncRunStatus.skipped,
        message: lastStatus,
        changesKnown: !_syncing,
      );
    }
    if (_inRateLimitCooldown() || _inBackoffCooldown()) {
      return SyncRunOutcome(status: SyncRunStatus.skipped, message: lastStatus);
    }
    if (_retryCount > 0) {
      return SyncRunOutcome(status: SyncRunStatus.skipped, message: lastStatus);
    }
    final last = _lastAttemptAt;
    if (last != null && DateTime.now().difference(last) < _minAutoInterval) {
      return SyncRunOutcome(status: SyncRunStatus.skipped, message: lastStatus);
    }
    if (await _remoteUnchanged()) {
      return const SyncRunOutcome(
        status: SyncRunStatus.remoteUnchanged,
        message: '远端 revision 未变化',
      );
    }
    return null;
  }

  Future<void> _autoTrigger({
    bool fromBackoff = false,
    bool quickPoll = false,
    bool refreshCatalog = true,
  }) async {
    if (!_enabled || _syncing || _automaticTriggerRunning) return;
    _automaticTriggerRunning = true;
    try {
      await ForegroundWorkCoordinator.instance.waitForMaintenanceTurn();
      if (!_enabled || _syncing || _inRateLimitCooldown()) return;
      // 失败退避期间，定时轮询/变更防抖都不得触发新一轮同步；
      // 否则每 60 秒盲重试会把服务端打到限流（截图中的每分钟失败循环）。
      if (_inBackoffCooldown()) return;
      // 失败后重试节奏完全交给退避定时器（fromBackoff），轮询/防抖让位。
      if (!fromBackoff && _retryCount > 0) return;
      final last = _lastAttemptAt;
      if (last != null && DateTime.now().difference(last) < _minAutoInterval) {
        return;
      }
      if (quickPoll && await _remoteUnchanged()) return;
      await syncNow(refreshCatalog: refreshCatalog, background: true);
    } finally {
      _automaticTriggerRunning = false;
    }
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
      )).id;
      try {
        final dir = mgr.webdavDir.trim().isEmpty
            ? 'RCH/sync'
            : mgr.webdavDir.trim();
        final rev = await syncapi.syncRemoteRevision(
          session: session,
          dir: dir,
        );
        return rev == st.revision;
      } finally {
        await webdavDisconnect(id: session);
      }
    } catch (_) {
      return false;
    }
  }

  /// Refresh local sources and persisted remote snapshots without opening a
  /// sync transport. Automatic catalog scraping uses this phase separately.
  Future<void> refreshCatalogIndexes({bool background = false}) async {
    for (final s in LibraryStore.instance.sources) {
      if (background) {
        await ForegroundWorkCoordinator.instance.waitForMaintenanceTurn();
      }
      try {
        if (s.isLocalFs) {
          await LibraryIndexService.instance.refreshSourceIndex(
            source: s,
            force: false,
          );
        } else {
          await LibraryIndexService.buildIndexFromSnapshots(s);
        }
      } catch (e) {
        debugPrint('[SyncEngine] source index refresh failed ${s.name}: $e');
      }
    }
  }

  /// 立即同步（手动入口与自动共用）。
  Future<String> syncNow({
    bool refreshCatalog = true,
    bool background = false,
  }) async => (await syncNowWithOutcome(
    refreshCatalog: refreshCatalog,
    background: background,
  )).message;

  Future<SyncRunOutcome> syncNowWithOutcome({
    bool refreshCatalog = true,
    bool background = false,
  }) {
    Future<SyncRunOutcome> run() => _runSyncNowWithOutcome(
      refreshCatalog: refreshCatalog,
      background: background,
    );
    if (background) return run();
    return ForegroundWorkCoordinator.instance.runForeground(run);
  }

  Future<SyncRunOutcome> _runSyncNowWithOutcome({
    required bool refreshCatalog,
    required bool background,
  }) async {
    if (background) {
      await ForegroundWorkCoordinator.instance.waitForMaintenanceTurn();
    }
    final stopwatch = Stopwatch()..start();
    final mgr = SyncManager.instance;
    final url = mgr.webdavUrl.trim();
    if (_syncing) {
      return SyncRunOutcome(
        status: SyncRunStatus.skipped,
        message: '正在同步…',
        changesKnown: false,
      );
    }
    if (_inRateLimitCooldown()) {
      return SyncRunOutcome(
        status: SyncRunStatus.skipped,
        message: '服务器限流中，15 分钟后自动重试',
      );
    }
    _syncing = true;
    _lastAttemptAt = DateTime.now();
    BigInt? session;
    try {
      if (refreshCatalog) {
        await refreshCatalogIndexes(background: background);
      }
      if (url.isEmpty) {
        lastStatus = '未配置 WebDAV 同步';
        return SyncRunOutcome(
          status: SyncRunStatus.skipped,
          message: lastStatus,
        );
      }
      session = (await webdavConnect(
        url: url,
        username: mgr.webdavUsername.trim(),
        password: mgr.webdavPassword,
      )).id;
      final dir = mgr.webdavDir.trim().isEmpty
          ? 'RCH/sync'
          : mgr.webdavDir.trim();
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
      final changedEntities = <String, int>{
        for (final entity in out.changedEntities)
          if (entity.count > 0) entity.entity: entity.count,
      };
      if (changedEntities.isNotEmpty) {
        // Rust committed authoritative SQLite changes. Reload only when the
        // typed result reports changed entities; no-op syncs need no full store
        // and catalog reload.
        await LibraryStore.instance.load(force: true, persist: false);
        await LibraryCatalogStore.instance.loadTree();
      }
      return SyncRunOutcome(
        status: changedEntities.isEmpty
            ? SyncRunStatus.remoteUnchanged
            : SyncRunStatus.completed,
        message: lastStatus,
        revision: out.revision,
        changedEntities: changedEntities,
      );
    } catch (e) {
      lastStatus = '同步失败: $e';
      final msg = '$e';
      if (msg.contains('503') ||
          msg.contains('429') ||
          msg.contains('Too many requests')) {
        // 坚果云等限流：进入 15 分钟冷却，不短周期重试轰炸。
        _rateLimitUntil = DateTime.now().add(const Duration(minutes: 15));
        _retryCount = 0;
      } else {
        _scheduleBackoffRetry();
      }
      try {
        await syncapi.syncSetLastError(message: '$e');
      } catch (_) {}
      return SyncRunOutcome(
        status: SyncRunStatus.failed,
        message: lastStatus,
        changesKnown: false,
      );
    } finally {
      if (session != null) {
        try {
          await webdavDisconnect(id: session);
        } catch (_) {}
      }
      _syncing = false;
      if (kProfileMode) {
        debugPrint(
          '[performance] phase=sync_attempt elapsed_ms=${stopwatch.elapsedMilliseconds}',
        );
      }
    }
  }

  /// 失败指数退避重试：30s → 1m → 2m → 4m → 8m → 15m（封顶），成功重置。
  void _scheduleBackoffRetry() {
    if (!_enabled) return;
    _retryCount++;
    final exp = (_retryCount - 1).clamp(0, 5).toInt();
    final backoffMs = (30000 * (1 << exp))
        .clamp(0, _maxBackoff.inMilliseconds)
        .toInt();
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
