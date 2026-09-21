import 'dart:async';
import 'dart:ui' as ui;

import 'package:app/src/rust/api/book.dart';
import 'package:app/src/rust/api/remote_cover.dart' as rust;
import 'package:app/src/rust/api/source.dart';
import 'package:app/store/baidu_session.dart';
import 'package:app/store/cloud115_session.dart';
import 'package:app/store/library_store.dart';
import 'package:app/store/models.dart';
import 'package:app/store/quark_session.dart';
import 'package:app/store/remote_cover_repository.dart';
import 'package:app/store/remote_scan_coordinator.dart';
import 'package:app/store/remote_scan_models.dart';
import 'package:app/store/sftp_session.dart';
import 'package:app/ui/common.dart';
import 'package:app/store/webdav_session.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';

class VisibleCoverLease<T> {
  VisibleCoverLease(this.future, this._dispose);

  final Future<T> future;
  final void Function() _dispose;
  bool _disposed = false;

  void dispose() {
    if (_disposed) return;
    _disposed = true;
    _dispose();
  }
}

class VisibleCoverScheduler<T> {
  VisibleCoverScheduler({this.maxConcurrent = 4}) : assert(maxConcurrent > 0);

  final int maxConcurrent;
  final Map<String, _VisibleCoverTask<T>> _tasks = {};
  final List<_VisibleCoverTask<T>> _pending = [];
  int _running = 0;

  VisibleCoverLease<T> acquire(String key, Future<T> Function() load) {
    final existing = _tasks[key];
    final _VisibleCoverTask<T> task;
    if (existing == null) {
      task = _VisibleCoverTask<T>(key, load);
      _tasks[key] = task;
      _pending.add(task);
    } else {
      task = existing;
    }
    task.subscribers++;
    _drain();
    return VisibleCoverLease<T>(task.completer.future, () => _release(task));
  }

  void _release(_VisibleCoverTask<T> task) {
    task.subscribers--;
    if (task.subscribers > 0 || task.running || task.completer.isCompleted) {
      return;
    }
    _pending.remove(task);
    if (identical(_tasks[task.key], task)) _tasks.remove(task.key);
    task.completer.completeError(StateError('cover load cancelled'));
  }

  void _drain() {
    while (_running < maxConcurrent && _pending.isNotEmpty) {
      final task = _pending.removeAt(0);
      if (task.completer.isCompleted || task.subscribers == 0) continue;
      task.running = true;
      _running++;
      Future<T>.sync(task.load)
          .then(task.completer.complete, onError: task.completer.completeError)
          .whenComplete(() {
            task.running = false;
            _running--;
            if (identical(_tasks[task.key], task)) _tasks.remove(task.key);
            _drain();
          });
    }
  }
}

class _VisibleCoverTask<T> {
  _VisibleCoverTask(this.key, this.load);

  final String key;
  final Future<T> Function() load;
  final Completer<T> completer = Completer<T>();
  int subscribers = 0;
  bool running = false;
}

Future<T?> loadCoverWithSafePolicy<T>({
  required bool remoteFetchEnabled,
  bool Function()? remoteFetchEnabledNow,
  required BigInt? liveSession,
  required Future<BigInt> Function() createSession,
  required Future<T> Function(BigInt session, bool remoteFetchEnabled) load,
}) async {
  bool isEnabled() =>
      remoteFetchEnabled && (remoteFetchEnabledNow?.call() ?? true);
  if (!isEnabled()) return null;
  final session = liveSession ?? await createSession();
  if (!isEnabled()) return null;
  return load(session, true);
}

/// Run one remote-cover operation only while the live network gate remains
/// enabled. The check after the await prevents a later step from continuing
/// after settings changed while this operation was queued or in flight.
Future<T?> runRemoteCoverOperationWithGate<T>({
  required bool Function() isEnabled,
  required Future<T> Function() operation,
}) async {
  if (!isEnabled()) return null;
  final value = await operation();
  if (!isEnabled()) return null;
  return value;
}

bool shouldSkipRemoteCoverNetwork({
  required BookSource source,
  required bool remoteCoverFetchEnabled,
}) => source.needsSession && !remoteCoverFetchEnabled;

class SourceCoverNoticeGate {
  final Set<String> _seen = {};

  bool take(String sourceKey) => _seen.add(sourceKey);
}

/// 封面加载任务队列 — 限制并发 FFI 调用数，避免数百个封面同时竞争线程池。
///
/// 设计：
/// - 最大并发数 4：本地封面 open_document + decode 约 30-80ms/本，4 并发足以喂饱 GPU。
/// - 已缓存的任务立即返回（内存缓存命中），不消耗并发槽位。
/// - 滚动时新出现的 Widget 入队，不再可见的 Widget 自动取消（didUpdateWidget dispose）。
class _CoverLoadQueue {
  _CoverLoadQueue._();

  static final VisibleCoverScheduler<ui.Image> scheduler =
      VisibleCoverScheduler<ui.Image>();

  static const int maxConcurrent = 4;

  int _running = 0;
  final List<_QueuedTask> _pending = [];

  Completer<ui.Image> enqueue(String key, Future<ui.Image> Function() task) {
    final c = Completer<ui.Image>();
    final qt = _QueuedTask(key: key, task: task, completer: c);
    _pending.add(qt);
    _drain();
    return c;
  }

  void cancel(String key) {
    _pending.removeWhere((qt) {
      if (qt.key == key) {
        if (!qt.completer.isCompleted) {
          qt.completer.completeError(Exception('cancelled'));
        }
        return true;
      }
      return false;
    });
  }

  void _drain() {
    while (_running < maxConcurrent && _pending.isNotEmpty) {
      final qt = _pending.removeAt(0);
      if (qt.completer.isCompleted) continue; // 已被 cancel
      _running++;
      qt
          .task()
          .then((img) {
            qt.completer.complete(img);
          })
          .catchError((e) {
            if (!qt.completer.isCompleted) qt.completer.completeError(e);
          })
          .whenComplete(() {
            _running--;
            _drain();
          });
    }
  }
}

class _QueuedTask {
  final String key;
  final Future<ui.Image> Function() task;
  final Completer<ui.Image> completer;
  _QueuedTask({required this.key, required this.task, required this.completer});
}

/// 漫画封面：统一本地 / WebDAV 来源，带全局内存缓存。
///
/// StatefulWidget 设计确保：
/// - 加载 Future 只在 initState 中创建一次，父 rebuild 不会重新触发加载。
/// - 并发限制 4 个 FFI 调用 + 队列调度。
/// - 内存缓存命中立即返回，不经过队列。
/// - 滚动时 Widget dispose 自动取消队列中的等待任务。
/// - WebDAV 封面懒加载：未打开过的漫画不主动请求封面。
/// P1-D-2：legacy provider → local-only API 的 `kind`。
///
/// 115 的 **app 模式与 web 模式必须区分**（authority 分别是
/// `115:{app_id}:{root_id}` 与 `115web:{root}`），因此不能映射成同一个 kind。
/// 判定依据沿用仓库既有语义：APP 模式需要 clientId，缺失即为网页 Cookie 模式。
String? legacyCoverKindOf(BookSource source) {
  if (source.isWebDav) return 'webdav';
  if (source.isSftp) return 'sftp';
  if (source.isBaidu) return 'baidu';
  if (source.is115) {
    return (source.clientId ?? '').trim().isEmpty ? '115web' : '115';
  }
  if (source.isQuark) return 'quark';
  return null;
}

/// P1-D-2：legacy 的**既有** session + provider 获取路径（可注入，默认即原实现）。
///
/// 它只负责"local 未命中之后"的联网部分；顺序不变量由 `ComicCover` 的
/// `_load` 保证（local lookup 严格早于本 loader）。
typedef LegacyRemoteCoverLoader =
    Future<ui.Image> Function({
      required BookSource source,
      required String path,
      required int page,
      required int width,
      required int height,
      CropRect? crop,
    });

class ComicCover extends StatefulWidget {
  final BookSource source;
  final String path;
  final BoxFit fit;
  final bool force;

  /// Source-browser cards wait for the unified catalog to provide an asset
  /// id rather than falling back to one provider request per card.
  final bool preferUnifiedRemote;

  /// Asset identity from the unified remote catalog. When present, cloud
  /// covers use the local read/request queue instead of the legacy provider
  /// path-specific fetcher.
  final String? remoteAssetId;
  final BigInt? remoteSession;

  /// Cover repository seam. Tests inject a fake repository so the disk-first
  /// ordering can be asserted without a provider session or network service.
  /// Defaults to the shared [RemoteCoverRepository.instance].
  final RemoteCoverRepository? repository;

  /// P1-D-2：legacy 的 sessionless local-only 查找（默认走 Rust FRB API）。
  /// 测试可注入；生产路径下它**不建 session、不触 provider、不受网络开关影响**。
  final Future<PageImage?> Function(LegacyCoverLocalLookupDto lookup)?
  legacyLocalCoverReader;

  /// P1-D-2：legacy 的既有 session/provider 获取（默认走 `_loadLegacyRemoteCover`）。
  final LegacyRemoteCoverLoader? legacyRemoteCoverLoader;

  const ComicCover({
    super.key,
    required this.source,
    required this.path,
    this.fit = BoxFit.cover,
    this.force = false,
    this.preferUnifiedRemote = false,
    this.remoteAssetId,
    this.remoteSession,
    this.repository,
    this.legacyLocalCoverReader,
    this.legacyRemoteCoverLoader,
  });

  RemoteCoverRepository get coverRepository =>
      repository ?? RemoteCoverRepository.instance;

  @override
  State<ComicCover> createState() => _ComicCoverState();

  // ---- 全局内存缓存（已完成的封面） ----

  static final Map<String, ui.Image> _cache = {};

  static void clear() => _cache.clear();

  static void evict(String key) => _cache.remove(key);

  static void evictAll(String sourceId, String path) {
    _cache.removeWhere((k, _) => k.startsWith('$sourceId|$path'));
  }

  /// 未就绪占位（网盘文件未下载 / 容器文件夹无本地数据 / 未知封面状态**共用**）。
  ///
  /// 2026-09-21 用户决定：**删除原「未缓存」文案**，一律统一显示「等待扫描」。
  /// 原因：封面格子会在「等待获取」与旧「未缓存」之间来回跳，视觉上像两个互相矛盾的
  /// 状态；统一成同一个"等待"语义后不再自相矛盾。
  /// （状态抖动的**根因**另在扫描 reconcile 的档位错位与读路径回写，不是这条文案本身。）
  static Widget waitingScanPlaceholder() => Container(
    color: Colors.black26,
    child: Center(
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          Icon(
            Icons.schedule,
            size: 36,
            color: Colors.lightBlueAccent.withAlpha(120),
          ),
          SizedBox(height: 4),
          Text(
            '等待扫描',
            // TODO(第75轮): 该处在 const 子树内取不到 context ⇒ 暂用中性灰（明暗都可读）；
            // 下一轮把父级 const 拆掉后换回 colorScheme.onSurfaceVariant。
            style: TextStyle(fontSize: 10, color: Colors.grey),
          ),
        ],
      ),
    ),
  );
}

class _ComicCoverState extends State<ComicCover> {
  Future<ui.Image>? _future;
  VisibleCoverLease<ui.Image>? _lease;
  ValueListenable<RemoteScanStatus?>? _remoteScanStatus;
  bool _loadFailed = false;
  int? _lastRetriedScanGeneration;
  Timer? _remoteRetryTimer;

  /// 上一次使用的缓存 key；封面页/裁切/画质等元数据变化时用于触发重载。
  String? _lastCacheKey;
  late final String _remoteConsumerId =
      'cover:${widget.source.id}:${widget.remoteAssetId ?? widget.path}:${identityHashCode(this)}';

  String get _cacheKey {
    final store = LibraryStore.instance;
    final q = store.settings.coverQuality;
    final meta = store.metaOf(widget.source, widget.path);
    final dependencyKey = bookKeyOf(
      widget.source.type,
      widget.source.id,
      widget.path,
    );
    return '$dependencyKey|${widget.remoteAssetId ?? ''}|${q.name}|${meta.coverPage}'
        '|${meta.cropX},${meta.cropY},${meta.cropW},${meta.cropH}';
  }

  bool get _remoteCoverNetworkPaused => shouldSkipRemoteCoverNetwork(
    source: widget.source,
    remoteCoverFetchEnabled:
        LibraryStore.instance.settings.remoteCoverFetchEnabled,
  );

  Future<T> _guardRemoteCoverIo<T>(Future<T> Function() operation) async {
    final value = await runRemoteCoverOperationWithGate(
      isEnabled: () => !_remoteCoverNetworkPaused,
      operation: operation,
    );
    if (value == null) {
      throw const _RemoteCoverFetchDisabled();
    }
    return value;
  }

  @override
  void initState() {
    super.initState();
    LibraryStore.instance.addListener(_onStoreChanged);
    _attachRemoteScanStatus();
    _lastCacheKey = _cacheKey;
    _maybeLoad();
  }

  ValueListenable<int>? _coverRevision;
  int _lastCoverRevision = 0;
  String? _coverState;
  /// P1-E：本卡片是否**已经发出过** requestCover。
  /// 首次 miss 只 request 一次；此后 wake 驱动的刷新只重读 durable state。
  bool _coverRequestIssued = false;

  /// P1-E：订阅 source-level cover revision（wake-up）。事件不携带状态，
  /// 收到后只**重读** durable state（`readCover` 命中即显示）。
  void _attachCoverRevision() {
    // 2026-09-21（真机："详情页出图后海报墙不刷新"）：**不再按 asset id 早退**。
    // 墙上"容器文件夹"漫画这类卡片拿不到稳定的 asset id，旧实现在这里直接 return
    // ⇒ 它们从不挂 revision 监听、封面就绪后永远收不到唤醒（只有等下一次扫描的
    // 大 revision 才顺带更新，表现为"过很久才刷新"）。
    // 没有 asset id 的卡片同样有本地/legacy 取图路径（`_load` 里本地优先），
    // 所以让它们照样被唤醒是安全的：一次唤醒 = 一次本地读（`_CoverLoadQueue` 限流）。
    if (!widget.source.needsSession) return;
    final listenable = RemoteScanCoordinator.instance.coverRevisionFor(
      widget.source.id,
    );
    _coverRevision = listenable;
    _lastCoverRevision = listenable.value;
    listenable.addListener(_onCoverRevisionChanged);
    // missed-event recovery：首次观察该 source 时主动读一次 durable revision
    // （无 timer）。若期间漏过事件，这里会补上并刷新。
    unawaited(
      RemoteScanCoordinator.instance
          .catchUpCoverRevision(widget.source.id)
          .catchError((Object _) {}),
    );
  }

  void _detachCoverRevision() {
    _coverRevision?.removeListener(_onCoverRevisionChanged);
    _coverRevision = null;
  }

  void _onCoverRevisionChanged() {
    final value = _coverRevision?.value ?? 0;
    if (value == _lastCoverRevision) return;
    _lastCoverRevision = value;
    if (!mounted) return;
    // durable cover truth 可能变了 ⇒ 重读一次（**不** request、**不** 轮询）。
    _future = null;
    _loadFailed = false;
    // 2026-09-21（用户报告"封面格子在等待/未缓存之间闪"）：**不再无条件清空 `_coverState`**。
    // 清空会让新状态读回来之前的那一帧掉进占位分支，与上一帧的真实文案来回跳（= 闪）。
    // 但 `build()` 里 `running` 会短路成 spinner（`if (_coverState == 'running') return _loading();`），
    // 保留 `running` 会把 "running → ready" 的转场钉死在转圈上（3 个终端契约测试当场抓到）
    // ⇒ 只对 `running` 保持旧的清空语义，其余状态保留到新状态读回来再替换。
    if (_coverState == 'running') _coverState = null;
    _maybeLoad();
    if (mounted) setState(() {});
  }

  void _attachRemoteScanStatus() {
    if (!widget.source.needsSession) return;
    final listenable = RemoteScanCoordinator.instance.statusFor(
      widget.source.id,
    );
    _remoteScanStatus = listenable;
    listenable.addListener(_onRemoteScanStatusChanged);
    _attachCoverRevision();
  }

  void _detachRemoteScanStatus() {
    _remoteScanStatus?.removeListener(_onRemoteScanStatusChanged);
    _remoteScanStatus = null;
    _detachCoverRevision();
  }

  void _onRemoteScanStatusChanged() {
    final status = _remoteScanStatus?.value;
    if (status == null || !_isSuccessfulScan(status.status)) return;
    if (!_loadFailed) return;
    if (_lastRetriedScanGeneration == status.generation) return;
    if (!mounted) return;
    _lastRetriedScanGeneration = status.generation;
    // The scanner materializes the cover after publishing the listing. A
    // visible card may have failed before that write completed; retry once
    // for this generation so it can observe the newly written cache alias.
    _lease?.dispose();
    _lease = null;
    _future = null;
    _loadFailed = false;
    _remoteRetryTimer?.cancel();
    _remoteRetryTimer = null;
    _maybeLoad();
    if (mounted) setState(() {});
  }

  static bool _isSuccessfulScan(String status) {
    final normalized = status.trim().toLowerCase();
    return normalized == 'complete' ||
        normalized == 'completed' ||
        normalized == 'succeeded';
  }

  void _onStoreChanged() {
    final newKey = _cacheKey;
    if (newKey != _lastCacheKey) {
      _lease?.dispose();
      _lease = null;
      _future = null;
      _loadFailed = false;
        _remoteRetryTimer?.cancel();
      _remoteRetryTimer = null;
      _lastCacheKey = newKey;
      _maybeLoad();
      if (mounted) setState(() {});
      return;
    }
    // Toggling the remote network gate must not discard an already loaded
    // cover (or metadata/custom-cover selection). It only affects future I/O.
    if (_future == null) {
      _maybeLoad();
      if (mounted) setState(() {});
    }
  }

  @override
  void didUpdateWidget(covariant ComicCover oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.source.id != widget.source.id) {
      _detachRemoteScanStatus();
      _lastRetriedScanGeneration = null;
      _attachRemoteScanStatus();
    }
    final newKey = _cacheKey;
    if (oldWidget.source.id != widget.source.id ||
        oldWidget.path != widget.path ||
        oldWidget.force != widget.force ||
        oldWidget.preferUnifiedRemote != widget.preferUnifiedRemote ||
        // 第 79 轮续5：目录视图稍后补上 asset id 时必须重载 —— 否则卡片会一直停在
        // "回退 legacy" 的那条路径上，切不回统一路径（统一路径先读缓存，不重复下载）。
        oldWidget.remoteAssetId != widget.remoteAssetId ||
        newKey != _lastCacheKey) {
      // 路径变化：取消旧队列任务，重新加载
      if (widget.remoteAssetId != oldWidget.remoteAssetId) {
        // A reused grid slot may point at another remote asset. Release the
        // old consumer before attaching the new demand so stale viewport
        // ownership cannot accumulate in the process registry.
        unawaited(
          widget.coverRepository
              .release(consumerId: _remoteConsumerId)
              .catchError((_) {}),
        );
      }
      _lease?.dispose();
      _lease = null;
      _future = null;
      _loadFailed = false;
        _remoteRetryTimer?.cancel();
      _remoteRetryTimer = null;
      _lastCacheKey = newKey;
      _maybeLoad();
    }
  }

  @override
  void dispose() {
    // Widget 不可见时取消队列中的等待任务（已经开始的 FFI 调用不中断）
    LibraryStore.instance.removeListener(_onStoreChanged);
    _detachRemoteScanStatus();
    _remoteRetryTimer?.cancel();
    _remoteRetryTimer = null;
    _lease?.dispose();
    if (widget.remoteAssetId != null) {
      unawaited(
        widget.coverRepository
            .release(consumerId: _remoteConsumerId)
            .catchError((_) {}),
      );
    }
    super.dispose();
  }

  /// 磁盘优先：先读已落地的本地封面（内存 / cover 磁盘缓存 / 原始本地缓存），
  /// 命中即立即返回；只有未命中时才允许联网开关决定是否发起远程请求。
  ///
  /// 因此关闭联网开关不会隐藏磁盘上已存在的封面。
  Future<ui.Image?> _readLocalDiskCover({
    required int page,
    required CropRect? crop,
    required int width,
    required int height,
  }) async {
    final remoteAssetId = widget.remoteAssetId;
    if (remoteAssetId == null || !widget.source.needsSession) return null;
    final cached = await widget.coverRepository.readLocalCover(
      sourceId: widget.source.id,
      assetId: remoteAssetId,
      selection: rust.CoverSelectionDto(
        page: page,
        crop: crop,
        revision: _selectionRevision(page, crop),
      ),
      profile: rust.CoverProfileDto(
        width: width,
        height: height,
        decoderVersion: 1,
      ),
    );
    if (cached == null) return null;
    return rgbaToImage(cached.rgba, cached.width, cached.height);
  }

  void _maybeLoad() {
    if (_future != null) return;
    final key = _cacheKey;

    // 1) 内存缓存命中 → 立即完成
    final cached = ComicCover._cache[key];
    if (cached != null) {
      _future = Future.value(cached);
      return;
    }
    // 2) 磁盘优先：**先读本地证据**，只有本地未命中才轮到联网开关。
    //
    //    P1-D-2：这里**不再**按联网开关提前 return —— 那会把"已经存在于本地的
    //    封面"也一并隐藏，并让 custom/local(`bookCover`) 这种**根本不触网**的路径
    //    无法执行。offline 的控制改在 `_load` 内**本地未命中之后**进行：
    //    · legacy：local miss + offline → 直接抛 `_RemoteCoverFetchDisabled`（不取 session）
    //    · local/custom：无网络能力，直接执行
    //    · unified：本地读在 `_loadUnifiedRemoteCover` 内部先于开关判定
    // 第 79 轮续5（用户确认）：`preferUnifiedRemote && needsSession && remoteAssetId == null`
    // 过去在这里**直接 return**（"等目录视图补上稳定的 asset id"），代价是这类卡片
    // （典型：容器文件夹卡 = 用第一个漫画文件当封面）**永远停在占位**。
    // 现在改为**回退 legacy 取图**（与详情页同一条"没缓存就获取"的路径）：
    //   · 并发由 `_CoverLoadQueue.scheduler` 统一限流 —— 这正是当年担心的"legacy 请求风暴"的护栏；
    //   · 目录视图随后补上 asset id 时，`didUpdateWidget` 会重载并切回统一路径
    //     （统一路径先读缓存，因此不会重复下载）。

    // 入队：并发控制在队列内部
    _lease = _CoverLoadQueue.scheduler.acquire(key, _load);
    _future = _lease!.future;
    _attachLoadResult(key);
  }

  void _attachLoadResult(String key) {
    _future!
        .then((img) {
          ComicCover._cache[key] = img;
                _remoteRetryTimer?.cancel();
          _remoteRetryTimer = null;
        })
        .catchError((Object error) {
          _loadFailed = true;
          // P1-E：把 `requestCover` 返回的 **durable state** 落回 UI 状态。
          // 这一步必须在错误路径里做 —— `FutureBuilder` 看到 error 会走
          // `_placeholder()`，若此时不带上 state，`running` 就会丢失它的 spinner，
          // `pending`/`failed`/... 也会丢失各自文案。
          if (error is _RemoteCoverStateException && error.state.isNotEmpty) {
            _coverState = error.state;
          }
          // The scan completion notification and a failed visible-cover
          // request can arrive in either order. Re-check here so the cache
          // retry is not lost when the notification won the race.
          _onRemoteScanStatusChanged();
          _scheduleUnifiedRetry();
          if (mounted) setState(() {});
        });
  }

  /// 扫描器可能先发布目录终态，封面 worker 随后才完成。这里仅重读本地
  /// 缓存并在必要时重新提交同一持久任务键，次数有界，不为每张卡片创建
  /// 独立的 provider 请求。
  /// P1-E：**删除 8 × 900ms 的"保险式"重复 request**。
  ///
  /// 状态推进改由 source-level cover revision wake 驱动
  ///（`_onCoverRevisionChanged`）。这里不再有任何 Timer，也绝不重复
  /// `requestCover` —— 人工 retry 属另一条显式操作，不受此限。
  void _scheduleUnifiedRetry() {}

  /// 实际的封面加载逻辑（不包含队列调度）。
  Future<ui.Image> _load() async {
    final store = LibraryStore.instance;
    final q = store.settings.coverQuality;
    final (w, h) = q.size;
    final meta = store.metaOf(widget.source, widget.path);
    final crop = meta.hasCrop
        ? CropRect(
            x: meta.cropX!,
            y: meta.cropY!,
            w: meta.cropW!,
            h: meta.cropH!,
          )
        : null;

    // 磁盘优先：本地已存在的封面立即返回，联网开关对此没有否决权。
    final localCover = await _readLocalDiskCover(
      page: meta.coverPage,
      crop: crop,
      width: w,
      height: h,
    );
    if (localCover != null) return localCover;

    final remoteAssetId = widget.remoteAssetId;
    if (remoteAssetId != null && widget.source.needsSession) {
      return _loadUnifiedRemoteCover(
        assetId: remoteAssetId,
        session: widget.remoteSession,
        page: meta.coverPage,
        crop: crop,
        width: w,
        height: h,
      );
    }

    // P1-D-2：**需要 session 的 legacy 源**才做 local-only 查找；
    // 纯本地源（kind == null，最终走 bookCover）没有网络能力，直接放行不受开关阻止。
    if (legacyCoverKindOf(widget.source) != null) {
      final legacyLocalCover = await _readLegacyCoverLocal(meta.coverPage, w, h, crop);
      if (legacyLocalCover != null) return legacyLocalCover;

      // 本地未命中：offline 时**直接停止**，不去 session helper 里换一个异常
      // （这样 session=0 / provider=0 是清晰的控制流结论，而不是异常副作用），
      // 也绝不因此建立 session。表现为普通 placeholder。
      if (_remoteCoverNetworkPaused) {
        throw const _RemoteCoverFetchDisabled();
      }
    }

    // online：走**既有** session getter → 既有 provider cover path（不新增网络入口）。
    return await (widget.legacyRemoteCoverLoader ?? _loadLegacyRemoteCover)(
      source: widget.source,
      path: widget.path,
      page: meta.coverPage,
      width: w,
      height: h,
      crop: crop,
    );
  }

  /// P1-D-2：调用 sessionless local-only 查找；miss 返回 null（**不是错误**）。
  Future<ui.Image?> _readLegacyCoverLocal(
    int page,
    int w,
    int h,
    CropRect? crop,
  ) async {
    final lookup = _legacyLocalLookup(page, w, h, crop);
    if (lookup == null) return null;
    final reader =
        widget.legacyLocalCoverReader ??
        (LegacyCoverLocalLookupDto value) => readLegacyCoverLocal(lookup: value);
    final image = await reader(lookup);
    if (image == null) return null;
    return rgbaToImage(image.rgba, image.width, image.height);
  }

  /// Dart **只填 logical fields**：不构造 endpoint / origin / raw cache path /
  /// cover path / hash —— 这些全部由 Rust 作为 cache authority owner 派生。
  LegacyCoverLocalLookupDto? _legacyLocalLookup(
    int page,
    int w,
    int h,
    CropRect? crop,
  ) {
    final kind = legacyCoverKindOf(widget.source);
    if (kind == null) return null;
    // host/port 复用**同一个** Dart parser（薄包装），不复制解析逻辑。
    final (host, port) = kind == 'sftp' ? sftpHostPortOf(widget.source) : ('', 22);
    final String root;
    switch (kind) {
      case 'baidu':
        root = widget.source.path;
      case '115web':
      case 'quark':
        root = widget.source.rootId ?? '0';
      default:
        root = '';
    }
    return LegacyCoverLocalLookupDto(
      kind: kind,
      url: widget.source.url ?? '',
      host: host,
      port: port,
      appKey: widget.source.clientId ?? '',
      appId: widget.source.clientId ?? '',
      rootId: widget.source.rootId ?? '',
      root: root,
      logicalPath: widget.path,
      page: page,
      width: w,
      height: h,
      crop: crop,
    );
  }

  /// P1-D-2：legacy 的**既有** session + provider 获取路径（原样搬迁，语义不变）。
  Future<ui.Image> _loadLegacyRemoteCover({
    required BookSource source,
    required String path,
    required int page,
    required int width,
    required int height,
    CropRect? crop,
  }) async {
    if (source.isWebDav) {
      final session = await _guardRemoteCoverIo(
        () => webdavSessionFor(source),
      );
      final p = await _guardRemoteCoverIo(
        () => webdavCover(
          session: session,
          path: path,
          page: page,
          width: width,
          height: height,
          crop: crop,
        ),
      );
      return await rgbaToImage(p.rgba, p.width, p.height);
    } else if (source.isSftp) {
      final session = await _guardRemoteCoverIo(
        () => sftpSessionFor(source),
      );
      final p = await _guardRemoteCoverIo(
        () => sftpCover(
          session: session,
          path: path,
          page: page,
          width: width,
          height: height,
          crop: crop,
        ),
      );
      return await rgbaToImage(p.rgba, p.width, p.height);
    } else if (source.isBaidu) {
      final session = await _guardRemoteCoverIo(
        () => baiduSessionFor(source),
      );
      final p = await _guardRemoteCoverIo(
        () => baiduCover(
          session: session,
          path: path,
          page: page,
          width: width,
          height: height,
          crop: crop,
        ),
      );
      return await rgbaToImage(p.rgba, p.width, p.height);
    } else if (source.is115) {
      final session = await _guardRemoteCoverIo(
        () => cloud115SessionFor(source),
      );
      final p = await _guardRemoteCoverIo(
        () => cloud115CoverFor(
          source,
          session: session,
          path: path,
          page: page,
          width: width,
          height: height,
          crop: crop,
        ),
      );
      return await rgbaToImage(p.rgba, p.width, p.height);
    } else if (source.isQuark) {
      final session = await _guardRemoteCoverIo(
        () => quarkSessionFor(source),
      );
      final p = await _guardRemoteCoverIo(
        () => quarkCover(
          session: session,
          path: path,
          page: page,
          width: width,
          height: height,
          crop: crop,
        ),
      );
      return await rgbaToImage(p.rgba, p.width, p.height);
    } else {
      final p = await _guardRemoteCoverIo(
        () => bookCover(
          path: path,
          page: page,
          width: width,
          height: height,
          crop: crop,
        ),
      );
      return await rgbaToImage(p.rgba, p.width, p.height);
    }
  }

  /// 候选 profile：本档优先，其后是其余标准档（先大后小，尺寸去重）。
  List<rust.CoverProfileDto> _profileCandidates(rust.CoverProfileDto exact) {
    const standard = [(340, 480), (510, 720), (170, 240)];
    final result = <rust.CoverProfileDto>[exact];
    for (final (w, h) in standard) {
      if (w == exact.width && h == exact.height) continue;
      result.add(
        rust.CoverProfileDto(
          width: w,
          height: h,
          decoderVersion: exact.decoderVersion,
        ),
      );
    }
    return result;
  }

  /// 跨 profile 读取**已缓存**的封面（纯本地读，零网络）。
  ///
  /// 第 79 轮续6（真机 + 用户确认）：后台扫描按固定 340×480 抓图，而卡片档位由设置
  /// `coverQuality` 决定（"低" = 170×240）⇒ 已经抓好的封面躺在另一个 profile 下，
  /// 卡片完全用不上（真机实测：586 本 PDF 里 340 档 ready 142 本，170 档只有 42 本）。
  /// 这里"本档优先、其余标准档依次尝试"，命中即用 —— `RawImage(BoxFit.cover)` 会把它
  /// 缩放到卡片尺寸，等于"有图就用，别等重抓"。
  Future<ui.Image?> _readAnyCachedCover({
    required RemoteCoverRepository repository,
    required String assetId,
    required rust.CoverSelectionDto selection,
    required rust.CoverProfileDto profile,
  }) async {
    for (final candidate in _profileCandidates(profile)) {
      final cached = await repository.readCover(
        sourceId: widget.source.id,
        assetId: assetId,
        selection: selection,
        profile: candidate,
      );
      if (cached != null) {
        return rgbaToImage(cached.rgba, cached.width, cached.height);
      }
    }
    return null;
  }

  Future<ui.Image> _loadUnifiedRemoteCover({
    required String assetId,
    required BigInt? session,
    required int page,
    required CropRect? crop,
    required int width,
    required int height,
  }) async {
    final selection = rust.CoverSelectionDto(
      page: page,
      crop: crop,
      revision: _selectionRevision(page, crop),
    );
    final profile = rust.CoverProfileDto(
      width: width,
      height: height,
      decoderVersion: 1,
    );
    final repository = widget.coverRepository;
    final cached = await _readAnyCachedCover(
      repository: repository,
      assetId: assetId,
      selection: selection,
      profile: profile,
    );
    if (cached != null) return cached;
    if (_remoteCoverNetworkPaused) throw const _RemoteCoverFetchDisabled();
    final liveSession = session ?? await _createRemoteSession();
    // P1-E：**删除 30 × 350ms 轮询**。`requestCover` 只负责"首次确保任务存在"；
    // 之后的状态推进一律由 source-level cover revision wake 驱动
    //（`RemoteScanCoordinator.coverRevisionFor` → `_onCoverRevisionChanged`），
    // 绝不再轮询。不变量（第 79 轮续8 收窄并写明，与 E-REQUEST-ONCE 一致）：
    // **唤醒只重读 durable state；仅当"state 已 ready 但本地读不到任何缓存字节"
    // 时，才允许重新物化一次**（否则卡片会永远停在占位）。
    if (_coverRequestIssued) {
      // P1-E：wake 驱动的刷新**只重读** durable state，绝不重复 requestCover。
      // 第 79 轮续：必须带上本卡片**实际使用**的 selection + profile —— 否则会读到
      // 另一 profile 的旧状态（真机 bug：170 的图已 ready，墙面却按 340 的 failed
      // 显示"获取失败"）。
      //
      // 第 79 轮续4（真机 bug"海报墙封面没读取"）：state 变 `ready` 之后必须
      // **直接读缓存图**。旧实现无论 state 是什么都抛异常 ⇒ 图已经抓好、卡片却永远
      // 停在占位（用户原话："没缓存就获取，有缓存就直读"）。`ready` 但读不到缓存
      // （blob 被清理/迁移）时**不抛**，落到下面的 request 分支重新物化一次，
      // 正好是"没缓存就获取"。
      final current = await repository.readState(
        sourceId: widget.source.id,
        assetId: assetId,
        selection: selection,
        profile: profile,
      );
      _coverState = current?.state;
      if (current?.ready ?? false) {
        // 跨 profile 回退：本档的图读不到时，用其它档现成的（第 79 轮续6）。
        final cached = await _readAnyCachedCover(
          repository: repository,
          assetId: assetId,
          selection: selection,
          profile: profile,
        );
        if (cached != null) return cached;
      } else {
        // 第 82 轮补（D4）：**没有 durable 行 ≠ 没有字节**。换档 purge 只删
        // `remote_cover_job/variant`，磁盘上的 `.cover-v2` 与 `remote_cover_ref`
        // 都还在（真机实测：variant=0 而 blob/ref=1061、共 1.2 GB）⇒ 卡片此前
        // 在这里直接抛异常、永远显示占位，尽管图就在本地。
        // 这里先做一次**纯本地读**（零网络、不产生任何 provider 请求；Rust 侧会按
        // ref 反推 content_revision 读同一份字节），命中就直接出图；确实没有才回退占位。
        final local = await _readAnyCachedCover(
          repository: repository,
          assetId: assetId,
          selection: selection,
          profile: profile,
        );
        if (local != null) return local;
        // 2026-09-21（真机："详情页已经有封面了，海报墙却显示获取失败"）：
        // unified 缓存与 **legacy 缓存是两套互不相通的东西** —— Rust `read_legacy_cover_local`
        // 以 `authority + 逻辑路径` 为键读 legacy 封面缓存，且硬契约是**纯本地**：
        // 不建 session、不联网、不建 job、不 wake worker、不改任何 durable state。
        // 详情页（不传 `remoteAssetId`）走的正是 legacy 路径 ⇒ 同一本书它能出图，
        // 而墙上卡片走 unified 路径却停在"获取失败"。
        // ⇒ 抛之前补一次 legacy 纯本地回退：零网络成本，命中即出图。
        final legacy = await _readLegacyCoverFallback(page, width, height, crop);
        if (legacy != null) return legacy;
        throw _RemoteCoverStateException(
          current?.state ?? '',
          current?.errorCode,
        );
      }
    }
    _coverRequestIssued = true;
    final durable = await repository.requestCover(
      source: widget.source,
      session: liveSession,
      assetId: assetId,
      consumerId: _remoteConsumerId,
      selection: selection,
      profile: profile,
    );
    _coverState = durable.state;
    if (durable.ready) {
      final image = await repository.readCover(
        sourceId: widget.source.id,
        assetId: assetId,
        selection: selection,
        profile: profile,
      );
      if (image != null) {
        return rgbaToImage(image.rgba, image.width, image.height);
      }
    }
    final legacyAfterRequest = await _readLegacyCoverFallback(
      page,
      width,
      height,
      crop,
    );
    if (legacyAfterRequest != null) return legacyAfterRequest;
    throw _RemoteCoverStateException(durable.state, durable.errorCode);
  }

  /// unified 路径确实拿不到字节时的**最后一道纯本地回退**（legacy 封面缓存）。
  ///
  /// 为什么需要（2026-09-21，真机）：详情页（legacy 路径、不传 asset id）能显示封面，
  /// 墙上卡片（unified 路径）却显示"获取失败" —— 两套缓存互不相通，而 legacy 那份
  /// 字节本来就在本地。`read_legacy_cover_local` 是纯本地读，因此这里**零网络成本**。
  Future<ui.Image?> _readLegacyCoverFallback(
    int page,
    int w,
    int h,
    CropRect? crop,
  ) async {
    if (legacyCoverKindOf(widget.source) == null) return null;
    try {
      return await _readLegacyCoverLocal(page, w, h, crop);
    } catch (_) {
      // 回退失败不影响主流程：仍旧走原来的占位/失败语义。
      return null;
    }
  }

  Future<BigInt> _createRemoteSession() async {
    if (widget.source.isWebDav) return webdavSessionFor(widget.source);
    if (widget.source.isSftp) return sftpSessionFor(widget.source);
    if (widget.source.isBaidu) return baiduSessionFor(widget.source);
    if (widget.source.is115) return cloud115SessionFor(widget.source);
    if (widget.source.isQuark) return quarkSessionFor(widget.source);
    throw StateError('远程书源会话不可用');
  }

  static String _selectionRevision(int page, CropRect? crop) {
    if (page == 0 && crop == null) return 'default';
    final cropKey = crop == null
        ? ''
        : '${crop.x.toStringAsFixed(5)},${crop.y.toStringAsFixed(5)},'
              '${crop.w.toStringAsFixed(5)},${crop.h.toStringAsFixed(5)}';
    return 'page:$page|crop:$cropKey';
  }

  @override
  Widget build(BuildContext context) {
    // P1-E：**只有 `running` 显示 spinner**；其余非 ready 状态渲染各自文案。
    if (_coverState == 'running') return _loading();
    if (_future == null) return _loading();

    return FutureBuilder<ui.Image>(
      future: _future,
      builder: (context, snap) {
        if (snap.hasData) {
          return RawImage(image: snap.data, fit: widget.fit);
        }
        if (snap.hasError) {
          return _placeholder();
        }
        return _loading();
      },
    );
  }

  Widget _loading() => Container(
    color: Colors.black26,
    child: const Center(
      child: SizedBox(
        width: 22,
        height: 22,
        child: CircularProgressIndicator(strokeWidth: 2),
      ),
    ),
  );

  Widget _placeholder() {
    final label = switch (_coverState) {
      'pending' => '等待获取',
      'retry_wait' => '等待重试',
      'failed' => '获取失败',
      'unsupported' => '暂不支持',
      'blocked' => '暂不可用',
      _ => null,
    };
    if (label == null) return ComicCover.waitingScanPlaceholder();
    return Container(
      color: Colors.black26,
      alignment: Alignment.center,
      padding: const EdgeInsets.all(4),
      child: Text(
        label,
        textAlign: TextAlign.center,
        style: TextStyle(color: Theme.of(context).colorScheme.onSurfaceVariant, fontSize: 12),
      ),
    );
  }
}

class _RemoteCoverFetchDisabled implements Exception {
  const _RemoteCoverFetchDisabled();
}

/// P1-E：把 `requestCover` 返回的 **durable state** 交给 UI 渲染。
///
/// 它**不是**错误语义、更不改动任何 durable state —— Dart 不猜状态、不写 failed。
/// `running` 由 `build()` 渲染为 spinner，其余映射为对应等待/失败文案。
class _RemoteCoverStateException implements Exception {
  const _RemoteCoverStateException(this.state, this.errorCode);
  final String state;
  final String? errorCode;
}

/// 漫画卡片：封面 + 标题 + 副标题，海报墙通用。
class ComicCard extends StatelessWidget {
  final BookSource source;
  final String path;
  final String title;
  final String? subtitle;
  final VoidCallback onTap;
  final String? remoteAssetId;
  final BigInt? remoteSession;
  final bool preferUnifiedRemote;

  const ComicCard({
    super.key,
    required this.source,
    required this.path,
    required this.title,
    this.subtitle,
    required this.onTap,
    this.remoteAssetId,
    this.remoteSession,
    this.preferUnifiedRemote = false,
  });

  @override
  Widget build(BuildContext context) {
    return Card(
      clipBehavior: Clip.antiAlias,
      elevation: 3,
      shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(10)),
      child: InkWell(
        onTap: onTap,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Expanded(
              child: ComicCover(
                source: source,
                path: path,
                remoteAssetId: remoteAssetId,
                remoteSession: remoteSession,
                preferUnifiedRemote: preferUnifiedRemote,
              ),
            ),
            Container(
              color: Colors.black45,
              padding: const EdgeInsets.fromLTRB(6, 5, 6, 6),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    title,
                    maxLines: 2,
                    overflow: TextOverflow.ellipsis,
                    style: const TextStyle(fontSize: 12, height: 1.2),
                  ),
                  if (subtitle != null) ...[
                    const SizedBox(height: 2),
                    Text(
                      subtitle!,
                      style: TextStyle(
                        fontSize: 10,
                        color: Theme.of(context).colorScheme.onSurfaceVariant,
                      ),
                    ),
                  ],
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }
}
