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

  const ComicCover({
    super.key,
    required this.source,
    required this.path,
    this.fit = BoxFit.cover,
    this.force = false,
    this.preferUnifiedRemote = false,
    this.remoteAssetId,
    this.remoteSession,
  });

  @override
  State<ComicCover> createState() => _ComicCoverState();

  // ---- 全局内存缓存（已完成的封面） ----

  static final Map<String, ui.Image> _cache = {};

  static void clear() => _cache.clear();

  static void evict(String key) => _cache.remove(key);

  static void evictAll(String sourceId, String path) {
    _cache.removeWhere((k, _) => k.startsWith('$sourceId|$path'));
  }

  /// “未缓存”占位（网盘文件未下载 / 网盘文件夹无本地数据共用）。
  static Widget uncachedPlaceholder() => Container(
    color: Colors.black26,
    child: Center(
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          Icon(
            Icons.cloud_download_outlined,
            size: 36,
            color: Colors.lightBlueAccent.withAlpha(120),
          ),
          const SizedBox(height: 4),
          const Text(
            '未缓存',
            style: TextStyle(fontSize: 10, color: Colors.white38),
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
  int _remoteRetryAttempts = 0;

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

  void _attachRemoteScanStatus() {
    if (!widget.source.needsSession) return;
    final listenable = RemoteScanCoordinator.instance.statusFor(
      widget.source.id,
    );
    _remoteScanStatus = listenable;
    listenable.addListener(_onRemoteScanStatusChanged);
  }

  void _detachRemoteScanStatus() {
    _remoteScanStatus?.removeListener(_onRemoteScanStatusChanged);
    _remoteScanStatus = null;
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
    _remoteRetryAttempts = 0;
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
      _remoteRetryAttempts = 0;
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
        newKey != _lastCacheKey) {
      // 路径变化：取消旧队列任务，重新加载
      if (widget.remoteAssetId != oldWidget.remoteAssetId) {
        // A reused grid slot may point at another remote asset. Release the
        // old consumer before attaching the new demand so stale viewport
        // ownership cannot accumulate in the process registry.
        unawaited(
          RemoteCoverRepository.instance
              .release(consumerId: _remoteConsumerId)
              .catchError((_) {}),
        );
      }
      _lease?.dispose();
      _lease = null;
      _future = null;
      _loadFailed = false;
      _remoteRetryAttempts = 0;
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
        RemoteCoverRepository.instance
            .release(consumerId: _remoteConsumerId)
            .catchError((_) {}),
      );
    }
    super.dispose();
  }

  void _maybeLoad() {
    if (_future != null) return;
    final key = _cacheKey;

    // 内存缓存命中 → 立即完成
    final cached = ComicCover._cache[key];
    if (cached != null) {
      _future = Future.value(cached);
      return;
    }
    if (_remoteCoverNetworkPaused) return;
    if (widget.preferUnifiedRemote &&
        widget.source.needsSession &&
        widget.remoteAssetId == null) {
      // The online listing arrives before the local route table. Keep this
      // card on the placeholder until the catalog refresh supplies a stable
      // asset id, preventing a burst of legacy provider cover requests.
      return;
    }

    // 入队：并发控制在队列内部
    _lease = _CoverLoadQueue.scheduler.acquire(key, _load);
    _future = _lease!.future;
    _future!
        .then((img) {
          ComicCover._cache[key] = img;
          _remoteRetryAttempts = 0;
          _remoteRetryTimer?.cancel();
          _remoteRetryTimer = null;
        })
        .catchError((_) {
          _loadFailed = true;
          // The scan completion notification and a failed visible-cover
          // request can arrive in either order. Re-check here so the cache
          // retry is not lost when the notification won the race.
          _onRemoteScanStatusChanged();
          _scheduleUnifiedRetry();
        });
  }

  /// 扫描器可能先发布目录终态，封面 worker 随后才完成。这里仅重读本地
  /// 缓存并在必要时重新提交同一持久任务键，次数有界，不为每张卡片创建
  /// 独立的 provider 请求。
  void _scheduleUnifiedRetry() {
    if (!mounted || widget.remoteAssetId == null || _remoteCoverNetworkPaused) {
      return;
    }
    if (_remoteRetryAttempts >= 8 || _remoteRetryTimer != null) return;
    _remoteRetryAttempts++;
    _remoteRetryTimer = Timer(const Duration(milliseconds: 900), () {
      _remoteRetryTimer = null;
      if (!mounted || widget.remoteAssetId == null) return;
      _future = null;
      _loadFailed = false;
      _maybeLoad();
      if (mounted) setState(() {});
    });
  }

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

    if (widget.source.isWebDav) {
      final session = await _guardRemoteCoverIo(
        () => webdavSessionFor(widget.source),
      );
      final p = await _guardRemoteCoverIo(
        () => webdavCover(
          session: session,
          path: widget.path,
          page: meta.coverPage,
          width: w,
          height: h,
          crop: crop,
        ),
      );
      return await rgbaToImage(p.rgba, p.width, p.height);
    } else if (widget.source.isSftp) {
      final session = await _guardRemoteCoverIo(
        () => sftpSessionFor(widget.source),
      );
      final p = await _guardRemoteCoverIo(
        () => sftpCover(
          session: session,
          path: widget.path,
          page: meta.coverPage,
          width: w,
          height: h,
          crop: crop,
        ),
      );
      return await rgbaToImage(p.rgba, p.width, p.height);
    } else if (widget.source.isBaidu) {
      final session = await _guardRemoteCoverIo(
        () => baiduSessionFor(widget.source),
      );
      final p = await _guardRemoteCoverIo(
        () => baiduCover(
          session: session,
          path: widget.path,
          page: meta.coverPage,
          width: w,
          height: h,
          crop: crop,
        ),
      );
      return await rgbaToImage(p.rgba, p.width, p.height);
    } else if (widget.source.is115) {
      final session = await _guardRemoteCoverIo(
        () => cloud115SessionFor(widget.source),
      );
      final p = await _guardRemoteCoverIo(
        () => cloud115CoverFor(
          widget.source,
          session: session,
          path: widget.path,
          page: meta.coverPage,
          width: w,
          height: h,
          crop: crop,
        ),
      );
      return await rgbaToImage(p.rgba, p.width, p.height);
    } else if (widget.source.isQuark) {
      final session = await _guardRemoteCoverIo(
        () => quarkSessionFor(widget.source),
      );
      final p = await _guardRemoteCoverIo(
        () => quarkCover(
          session: session,
          path: widget.path,
          page: meta.coverPage,
          width: w,
          height: h,
          crop: crop,
        ),
      );
      return await rgbaToImage(p.rgba, p.width, p.height);
    } else {
      final p = await _guardRemoteCoverIo(
        () => bookCover(
          path: widget.path,
          page: meta.coverPage,
          width: w,
          height: h,
          crop: crop,
        ),
      );
      return await rgbaToImage(p.rgba, p.width, p.height);
    }
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
    final repository = RemoteCoverRepository.instance;
    final cached = await repository.readCover(
      sourceId: widget.source.id,
      assetId: assetId,
      selection: selection,
      profile: profile,
    );
    if (cached != null) {
      return rgbaToImage(cached.rgba, cached.width, cached.height);
    }
    if (_remoteCoverNetworkPaused) throw const _RemoteCoverFetchDisabled();
    final liveSession = session ?? await _createRemoteSession();
    await repository.requestCover(
      source: widget.source,
      session: liveSession,
      assetId: assetId,
      consumerId: _remoteConsumerId,
      selection: selection,
      profile: profile,
    );
    // Request is intentionally non-blocking on the Rust side. Polling only
    // the local cache keeps provider I/O in one queue and lets a background
    // scan publish the same result without duplicate extraction.
    for (var attempt = 0; attempt < 30; attempt++) {
      if (_remoteCoverNetworkPaused) {
        throw const _RemoteCoverFetchDisabled();
      }
      if (attempt > 0) {
        await Future<void>.delayed(const Duration(milliseconds: 350));
      }
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
    throw StateError('封面仍在队列中');
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
    if (_remoteCoverNetworkPaused && _future == null) return _placeholder();
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

  Widget _placeholder() => ComicCover.uncachedPlaceholder();
}

class _RemoteCoverFetchDisabled implements Exception {
  const _RemoteCoverFetchDisabled();
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
                      style: const TextStyle(
                        fontSize: 10,
                        color: Colors.white54,
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
