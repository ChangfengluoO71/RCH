import 'dart:async';

import 'package:app/src/rust/api/remote_cover.dart' as rust_cover;
import 'package:app/src/rust/api/remote_scan.dart' as rust;
import 'package:app/store/library_store.dart';
import 'package:app/store/models.dart';
import 'package:app/store/remote_scan_models.dart';
import 'package:flutter/foundation.dart';

typedef RemoteScanStart =
    Future<RemoteScanStatus> Function({
      required BookSource source,
      required BigInt session,
      required String rootPath,
      required String mode,
    });
typedef RemoteScanStartWithInitialListing =
    Future<RemoteScanStatus> Function({
      required BookSource source,
      required BigInt session,
      required String rootPath,
      required String mode,
      required String? initialListingJson,
    });
typedef RemoteScanControl = Future<void> Function(String sourceId);
typedef RemoteScanStatusLoader =
    Future<RemoteScanStatus?> Function(String sourceId);

class RemoteScanAlreadyRunning implements Exception {
  const RemoteScanAlreadyRunning(this.sourceId, this.requestedMode);
  final String sourceId;
  final String requestedMode;
}

class RemoteSessionSuccess {
  const RemoteSessionSuccess(this.source, this.session);
  final BookSource source;
  final BigInt session;
}

/// 把"某书源已获得/更新为当前有效 session"这一**事实**交给 Rust（P1-C）。
typedef RemoteSessionReadyNotifier =
    Future<void> Function(String sourceId, BigInt session);

/// 生产实现：调用 Rust 的 source-session lifecycle API。
///
/// 这是 best-effort 通知：失败不得影响会话获取本身，也**不**在这里做任何
/// retry / 6 小时 / unsupported / blocked / worker 决策 —— 那些全部归 Rust 拥有。
Future<void> _nativeNotifySourceSessionReady(
  String sourceId,
  BigInt session,
) async {
  try {
    await rust.notifySourceSessionReady(sourceId: sourceId, session: session);
  } catch (_) {
    // 生命周期通知是尽力而为，绝不向上抛。
  }
}

class RemoteSessionSuccessHub {
  RemoteSessionSuccessHub({RemoteSessionReadyNotifier? notifyReady})
    : _notifyReady = notifyReady ?? _nativeNotifySourceSessionReady;

  // Session callbacks can originate while a widget is being built (for
  // example, a cover that hits a cached provider session). Delivering those
  // callbacks asynchronously prevents the scan coordinator from mutating a
  // ValueNotifier during the build phase.
  final _events = StreamController<RemoteSessionSuccess>.broadcast();
  final RemoteSessionReadyNotifier _notifyReady;
  Stream<RemoteSessionSuccess> get events => _events.stream;

  /// 由各 provider 的 session manager 在**确实建立/更新了当前有效 session**
  /// 之后调用（缓存命中路径不会走到这里）。
  void emit(BookSource source, BigInt session) {
    _events.add(RemoteSessionSuccess(source, session));
    // P1-C：同一个 lifecycle fact 交给 Rust，由 Rust 负责验证绑定、做
    // bounded 的 source-scoped reconciliation，并在有可 claim 工作时唤醒 worker。
    // 契约上就是 best-effort：即使注入口实现自己抛错，也不得影响会话投递。
    unawaited(_notifyReady(source.id, session).catchError((Object _) {}));
  }

  Future<void> dispose() => _events.close();
}

final remoteSessionSuccessHub = RemoteSessionSuccessHub();

class RemoteScanCoordinator {
  RemoteScanCoordinator({
    RemoteScanStart? start,
    RemoteScanStart? startManual,
    RemoteScanStartWithInitialListing? startWithInitialListing,
    RemoteScanControl? pauseCall,
    RemoteScanControl? resumeCall,
    RemoteScanControl? cancelCall,
    RemoteScanStatusLoader? statusCall,
    RemoteSessionSuccessHub? sessionHub,
    bool Function()? automaticEnabled,
    bool Function()? coverFetchEnabled,
    Listenable? settingsListenable,
    this._debounce = const Duration(seconds: 2),
    this.progressPollInterval = const Duration(milliseconds: 500),
    DateTime Function()? clock,
  }) : _start = start ?? _nativeStart,
       _startManual =
           startManual ?? (start == null ? _nativeStartManual : null),
       _startWithInitialListing =
           startWithInitialListing ??
           (start == null ? _nativeStartWithInitialListing : null),
       _pause = pauseCall ?? _nativePause,
       _resume = resumeCall ?? _nativeResume,
       _cancel = cancelCall ?? _nativeCancel,
       _status = statusCall ?? _nativeStatus,
       _automaticEnabled = automaticEnabled ?? (() => automaticStartsEnabled()),
       _coverFetchEnabled =
           coverFetchEnabled ??
           (() => LibraryStore.instance.settings.remoteCoverFetchEnabled),
       _settingsListenable = settingsListenable ?? LibraryStore.instance,
       _clock = clock ?? DateTime.now {
    _settingsListenable.addListener(_onSettingsChanged);
    _sessionSubscription = (sessionHub ?? remoteSessionSuccessHub).events
        .listen((event) {
          if (!_automaticEnabled() ||
              _deferredRootListings.contains(event.source.id)) {
            return;
          }
          unawaited(
            ensureForSession(
              event.source,
              event.session,
            ).then<void>((_) {}, onError: (_) {}),
          );
        });
  }

  static final instance = RemoteScanCoordinator();
  static bool Function() automaticStartsEnabled = () => true;

  final RemoteScanStart _start;
  final RemoteScanStart? _startManual;
  final RemoteScanStartWithInitialListing? _startWithInitialListing;
  final RemoteScanControl _pause;
  final RemoteScanControl _resume;
  final RemoteScanControl _cancel;
  final RemoteScanStatusLoader _status;
  final bool Function() _automaticEnabled;
  final bool Function() _coverFetchEnabled;
  final Listenable _settingsListenable;
  final Duration _debounce;
  final Duration progressPollInterval;
  final DateTime Function() _clock;
  late final StreamSubscription<RemoteSessionSuccess> _sessionSubscription;
  final Map<String, Future<RemoteScanStatus>> _inflight = {};
  final Map<String, String> _inflightModes = {};
  final Set<String> _observedSources = {};
  final Map<String, ValueNotifier<RemoteScanStatus?>> _statuses = {};
  final Map<String, ValueNotifier<RemoteScanViewState?>> _viewStates = {};
  final Map<String, DateTime> _completedAt = {};
  final Map<String, RemoteScanStatus> _lastCompleted = {};
  final Set<String> _recoveringSources = {};
  // --- P1-E：cover durable-truth wake-up bridge -------------------------
  //
  // 语义（冻结）：事件只表示«该 source 的 cover durable truth 可能变了»，**不携带**
  //           authoritative 状态；consumer 收到后必须自己重读 durable state。
  // 去重：durable revision token（remoteViewRevision）—— <= lastSeen 视为重复。
  // 禁止：periodic timer 轮询 revision。
  final Map<String, int> _lastSeenCoverRevision = {};
  final Map<String, ValueNotifier<int>> _coverRevisions = {};
  StreamSubscription<rust_cover.CoverRevisionEvent>? _coverSubscription;
  /// 测试注入点：默认走真实 FRB（subscribeCoverRevisions / remoteViewRevision）。
  Stream<rust_cover.CoverRevisionEvent>? debugCoverRevisionStream;
  Future<int> Function(String sourceId)? debugCoverRevisionReader;
  Future<void> Function(String sourceId)? debugCoverAggregateRefresh;
  final Set<String> _deferredRootListings = {};
  final Map<String, Timer> _progressTimers = {};
  final Set<String> _progressPollInFlight = {};
  bool _disposed = false;

  /// 某 source 的 cover revision（单调递增 durable token；**仅用于 wake-up**）。
  ValueListenable<int> coverRevisionFor(String sourceId) =>
      _coverRevisions.putIfAbsent(sourceId, () => ValueNotifier(0));

  int lastSeenCoverRevision(String sourceId) =>
      _lastSeenCoverRevision[sourceId] ?? 0;

  /// 订阅**唯一**的 cover revision stream（进程级只建立一次）。
  /// RG-B 可诊断性修复（B）：把原生启动失败**映射为安全枚举码**。
  ///
  /// 原生 `start_job` 的失败文本只可能是固定常量（见 `api/remote_scan.rs`），但为避免任何
  /// 泄露风险，这里**只输出枚举码**，绝不把原生文本带进 UI。
  static String scanStartRejectionCode(Object error) {
    final text = error.toString().toLowerCase();
    if (text.contains('session unavailable')) return 'scanStartRejected:session';
    if (text.contains('session proof')) return 'scanStartRejected:epoch';
    if (text.contains('root changed')) return 'scanStartRejected:root';
    if (text.contains('proof unavailable')) return 'scanStartRejected:proof';
    if (text.contains('state recovery unavailable')) {
      return 'scanStartRejected:recovery';
    }
    if (text.contains('state unavailable')) return 'scanStartRejected:state';
    if (text.contains('baseline unavailable')) return 'scanStartRejected:baseline';
    if (text.contains('local-only')) return 'scanStartRejected:localOnly';
    if (text.contains('invalid scan mode')) return 'scanStartRejected:mode';
    return 'scanStartRejected:unknown';
  }

  void startCoverRevisionWatch() {
    if (_disposed || _coverSubscription != null) return;
    final stream =
        debugCoverRevisionStream ?? rust_cover.subscribeCoverRevisions();
    _coverSubscription = stream.listen(
      (event) {
        // best-effort：消费失败不得影响 durable state 或其它消费者。
        unawaited(_applyCoverRevision(event.sourceId));
      },
      onError: (Object _) {},
      cancelOnError: false,
    );
  }

  Future<int> _readCoverRevision(String sourceId) async {
    final reader = debugCoverRevisionReader;
    if (reader != null) return reader(sourceId);
    return (await rust_cover.remoteViewRevision(sourceId: sourceId)).toInt();
  }

  /// missed-event recovery：只在**首次观察 / stream 重建 / source 重新 attach /
  /// 上下文切到此前未观察的 source** 时主动读一次 durable revision（无 timer）。
  Future<void> catchUpCoverRevision(String sourceId) =>
      _applyCoverRevision(sourceId);

  /// 收到 wake（或 catch-up）后：读 durable token → 去重 → 推进 notifier → 刷新聚合。
  Future<void> _applyCoverRevision(String sourceId) async {
    if (_disposed || sourceId.isEmpty) return;
    final durable = await _readCoverRevision(sourceId);
    if (_disposed) return;
    final lastSeen = _lastSeenCoverRevision[sourceId] ?? 0;
    if (durable <= lastSeen) return; // duplicate wake：忽略，绝不重复刷新
    _lastSeenCoverRevision[sourceId] = durable;
    _coverRevisions.putIfAbsent(sourceId, () => ValueNotifier(0)).value = durable;
    await _refreshCoverAggregate(sourceId);
  }

  /// 本地只读聚合刷新：绝不触 provider / 不建 session / 不起 timer。
  Future<void> _refreshCoverAggregate(String sourceId) async {
    if (_disposed) return;
    final injected = debugCoverAggregateRefresh;
    if (injected != null) {
      await injected(sourceId);
      return;
    }
    // best-effort：wake 驱动的本地聚合刷新**绝不允许**把异常抛给 stream 消费者
    //（否则一次读取失败会污染整个 wake 处理链路）。失败即静默返回，
    // durable state 与 worker 均不受影响。
    try {
      final status = await _status(sourceId);
      if (_disposed || status == null) return;
      _setStatus(sourceId, status);
    } catch (_) {
      return;
    }
  }

  ValueListenable<RemoteScanStatus?> statusFor(String sourceId) =>
      _statuses.putIfAbsent(sourceId, () => ValueNotifier(null));

  ValueListenable<RemoteScanViewState?> viewStateFor(String sourceId) =>
      _viewStates.putIfAbsent(sourceId, () => ValueNotifier(null));

  /// Hold the automatic session trigger until the browser can hand off the
  /// root page it already fetched. This closes the session-event/list race and
  /// lets the native worker seed its first page without another provider call.
  void deferRootListing(BookSource source) {
    if (source.needsSession) _deferredRootListings.add(source.id);
  }

  void cancelDeferredRootListing(BookSource source) {
    _deferredRootListings.remove(source.id);
  }

  Future<RemoteScanStatus?> noteRootListed(
    BookSource source,
    BigInt session, {
    String? initialListingJson,
  }) {
    _deferredRootListings.remove(source.id);
    if (!_automaticEnabled()) return Future.value(null);
    return ensureForSession(
      source,
      session,
      initialListingJson: initialListingJson,
    );
  }

  Future<void> restoreStatuses(Iterable<BookSource> sources) async {
    for (final source in sources.where((source) => source.needsSession)) {
      final status = await _status(source.id);
      if (status == null) continue;
      _setStatus(source.id, status);
      if (_isSuccessfulTerminalStatus(status.status)) {
        _observedSources.add(source.id);
      } else if (status.status == 'running' || status.status == 'paused') {
        _recoveringSources.add(source.id);
      }
    }
  }

  Future<RemoteScanStatus> ensureForSession(
    BookSource source,
    BigInt session, {
    String? initialListingJson,
  }) {
    final existing = _inflight[source.id];
    if (existing != null) return existing;
    final completedAt = _completedAt[source.id];
    final completed = _lastCompleted[source.id];
    if (completedAt != null &&
        completed != null &&
        _clock().difference(completedAt) < _debounce) {
      return Future.value(completed);
    }
    final recovering = _recoveringSources.remove(source.id);
    final mode = recovering
        ? 'full'
        : (_observedSources.contains(source.id) ? 'incremental' : 'full');
    return _startShared(
      source,
      session,
      mode,
      initialListingJson: initialListingJson,
    );
  }

  Future<RemoteScanStatus> rescan(
    BookSource source,
    BigInt session,
    String mode,
  ) {
    if (mode != 'incremental' && mode != 'full') {
      return Future.error(ArgumentError.value(mode, 'mode'));
    }
    final existing = _inflight[source.id];
    if (existing != null && _inflightModes[source.id] == mode) {
      return existing;
    }
    if (existing != null) {
      return Future.error(RemoteScanAlreadyRunning(source.id, mode));
    }
    return _startShared(
      source,
      session,
      mode,
      forceRecheck: mode == 'incremental',
    );
  }

  Future<RemoteScanStatus> rescanIncremental(
    BookSource source,
    BigInt session,
  ) => rescan(source, session, 'incremental');

  Future<RemoteScanStatus> rescanFull(BookSource source, BigInt session) =>
      rescan(source, session, 'full');

  /// Retry the last scan with the same mode. A failed full scan must remain a
  /// full retry; silently downgrading it to incremental can preserve a
  /// partially indexed Quark/remote tree as if it were a valid baseline.
  Future<RemoteScanStatus> retry(BookSource source, BigInt session) {
    final current = _statuses[source.id]?.value;
    final mode = current?.mode == 'incremental' ? 'incremental' : 'full';
    return rescan(source, session, mode);
  }

  Future<void> pause(String sourceId) async {
    await _pause(sourceId);
    _setControlState(sourceId, 'paused');
    _stopProgressMonitor(sourceId);
  }

  Future<void> resume(String sourceId) async {
    await _resume(sourceId);
    _setControlState(sourceId, 'running');
    _startProgressMonitor(sourceId);
  }

  Future<void> cancel(String sourceId) async {
    await _cancel(sourceId);
    _setControlState(sourceId, 'cancelled');
    _stopProgressMonitor(sourceId);
  }

  Future<RemoteScanStatus> _startShared(
    BookSource source,
    BigInt session,
    String mode, {
    String? initialListingJson,
    bool forceRecheck = false,
  }) {
    // Native start returns a job handle while the status is polled. Publish a
    // state immediately so the status panel is useful during that interval.
    _setStatus(
      source.id,
      RemoteScanStatus(
        sourceId: source.id,
        status: 'queued',
        mode: mode,
        generation: _lastCompleted[source.id]?.generation ?? 0,
      ),
    );
    late final Future<RemoteScanStatus> future;
    // Future.sync also captures a provider/native binding that throws before
    // returning a Future. Without it, the single-flight entry is never
    // installed and no visible failure reaches the status panel.
    final startFuture = Future<RemoteScanStatus>.sync(() {
      if (forceRecheck && _startManual != null) {
        return _startManual(
          source: source,
          session: session,
          rootPath: source.effectiveRootPath,
          mode: mode,
        );
      }
      if (initialListingJson != null && _startWithInitialListing != null) {
        return _startWithInitialListing(
          source: source,
          session: session,
          rootPath: source.effectiveRootPath,
          mode: mode,
          initialListingJson: initialListingJson,
        );
      }
      return _start(
        source: source,
        session: session,
        rootPath: source.effectiveRootPath,
        mode: mode,
      );
    });
    future = startFuture
        .then<RemoteScanStatus>(
          (status) {
            if (_isSuccessfulTerminalStatus(status.status)) {
              _observedSources.add(source.id);
            } else if (!_isActiveScanStatus(status.status)) {
              // A failed/degraded/cancelled generation is not a valid
              // incremental baseline, even when an older generation had
              // completed successfully.  Keep the next automatic trigger
              // on a full scan until a terminal success is observed.
              _observedSources.remove(source.id);
              _completedAt.remove(source.id);
              _lastCompleted.remove(source.id);
            }
            _setStatus(source.id, status);
            if (_isSuccessfulTerminalStatus(status.status)) {
              _completedAt[source.id] = _clock();
              _lastCompleted[source.id] = status;
            }
            if (!_shouldMonitorProgress(status)) {
              _stopProgressMonitor(source.id);
            }
            return status;
          },
          onError: (Object error, StackTrace stackTrace) {
            // Never surface the native exception text here: provider errors
            // may contain credentials or private URLs. Return a stable,
            // safe status so UI callbacks do not create an unhandled error;
            // the flight is released by whenComplete below.
            final generation = _lastCompleted[source.id]?.generation ?? 0;
            _setStatus(
              source.id,
              RemoteScanStatus(
                sourceId: source.id,
                status: 'failed',
                mode: mode,
                generation: generation,
                errorCode: scanStartRejectionCode(error),
              ),
            );
            _observedSources.remove(source.id);
            _completedAt.remove(source.id);
            _lastCompleted.remove(source.id);
            _stopProgressMonitor(source.id);
            return RemoteScanStatus(
              sourceId: source.id,
              status: 'failed',
              mode: mode,
              generation: generation,
              errorCode: scanStartRejectionCode(error),
            );
          },
        )
        .whenComplete(() {
          if (identical(_inflight[source.id], future)) {
            _inflight.remove(source.id);
            _inflightModes.remove(source.id);
          }
        });
    _inflight[source.id] = future;
    _inflightModes[source.id] = mode;
    _startProgressMonitor(source.id);
    return future;
  }

  static bool _isActiveScanStatus(String status) {
    final normalized = status.trim().toLowerCase();
    return normalized == 'queued' || normalized == 'running';
  }

  /// 目录发布和封面处理是两个独立阶段。目录进入终态后，只要还有
  /// 运行中、排队或待重试的封面任务，就继续使用当前来源唯一的轮询器，
  /// 让根目录卡片及时看到封面；封面开关暂停时不保持无意义的轮询。
  static bool _needsProgressMonitor(RemoteScanStatus status) {
    if (_isActiveScanStatus(status.status)) return true;
    return status.activeBooks > 0 ||
        status.pendingBooks > 0 ||
        status.retryBooks > 0;
  }

  bool _shouldMonitorProgress(RemoteScanStatus status) {
    // A disabled cover gate must stop polling a terminal generation whose
    // only remaining work is cover I/O. Listing discovery is independent and
    // remains observable while it is still queued/running.
    if (!_coverFetchEnabled() && !_isActiveScanStatus(status.status)) {
      return false;
    }
    return _needsProgressMonitor(status);
  }

  static bool _isSuccessfulTerminalStatus(String status) {
    final normalized = status.trim().toLowerCase();
    return normalized == 'complete' ||
        normalized == 'completed' ||
        normalized == 'succeeded';
  }

  void _startProgressMonitor(String sourceId) {
    if (_disposed || progressPollInterval <= Duration.zero) return;
    if (_progressTimers.containsKey(sourceId)) return;
    _progressTimers[sourceId] = Timer.periodic(
      progressPollInterval,
      (_) => unawaited(_pollProgress(sourceId)),
    );
  }

  void _stopProgressMonitor(String sourceId) {
    _progressTimers.remove(sourceId)?.cancel();
  }

  Future<void> _pollProgress(String sourceId) async {
    if (_disposed ||
        !_progressTimers.containsKey(sourceId) ||
        !_progressPollInFlight.add(sourceId)) {
      return;
    }
    try {
      final status = await _status(sourceId);
      if (_disposed ||
          !_progressTimers.containsKey(sourceId) ||
          status == null) {
        return;
      }
      _setStatus(sourceId, status);
      if (!_shouldMonitorProgress(status)) {
        _stopProgressMonitor(sourceId);
      }
    } catch (_) {
      // The native start future owns terminal errors. A transient status poll
      // failure must not replace a useful queued/running state.
    } finally {
      _progressPollInFlight.remove(sourceId);
    }
  }

  void _setControlState(String sourceId, String state) {
    if (_disposed) return;
    final notifier = _statuses.putIfAbsent(sourceId, () => ValueNotifier(null));
    final current = notifier.value;
    if (current != null) _setStatus(sourceId, current.copyWith(status: state));
  }

  void _setStatus(String sourceId, RemoteScanStatus status) {
    if (_disposed) return;
    _statuses.putIfAbsent(sourceId, () => ValueNotifier(null)).value = status;
    _viewStates
        .putIfAbsent(sourceId, () => ValueNotifier(null))
        .value = RemoteScanViewState.fromStatus(
      status,
      coverFetchPaused: !_coverFetchEnabled(),
    );
  }

  void _onSettingsChanged() {
    if (_disposed) return;
    final coverEnabled = _coverFetchEnabled();
    for (final entry in _statuses.entries) {
      final status = entry.value.value;
      if (status == null) continue;
      if (coverEnabled && _needsProgressMonitor(status)) {
        _startProgressMonitor(entry.key);
      } else if (!coverEnabled && !_isActiveScanStatus(status.status)) {
        _stopProgressMonitor(entry.key);
      }
      _viewStates
          .putIfAbsent(entry.key, () => ValueNotifier(null))
          .value = RemoteScanViewState.fromStatus(
        status,
        coverFetchPaused: !_coverFetchEnabled(),
      );
    }
  }

  static Future<RemoteScanStatus> _nativeStart({
    required BookSource source,
    required BigInt session,
    required String rootPath,
    required String mode,
  }) => _nativeStartWithInitialListing(
    source: source,
    session: session,
    rootPath: rootPath,
    mode: mode,
    initialListingJson: null,
  );

  static Future<RemoteScanStatus> _nativeStartManual({
    required BookSource source,
    required BigInt session,
    required String rootPath,
    required String mode,
  }) async {
    final job = await rust.remoteScanStartManual(
      sourceType: source.type,
      sourceId: source.id,
      session: session,
      rootPath: rootPath,
      mode: mode,
      initialListingJson: null,
    );
    while (true) {
      final dto = await rust.remoteScanStatus(sourceId: source.id);
      if (dto == null) {
        return RemoteScanStatus(
          sourceId: job.sourceId,
          status: job.status,
          mode: job.mode,
          generation: job.generation,
        );
      }
      final status = _fromStatusDto(dto);
      if (status.status != 'running') return status;
      await Future<void>.delayed(const Duration(milliseconds: 500));
    }
  }

  static Future<RemoteScanStatus> _nativeStartWithInitialListing({
    required BookSource source,
    required BigInt session,
    required String rootPath,
    required String mode,
    required String? initialListingJson,
  }) async {
    final job = await rust.remoteScanStart(
      sourceType: source.type,
      sourceId: source.id,
      session: session,
      rootPath: rootPath,
      mode: mode,
      initialListingJson: initialListingJson,
    );
    while (true) {
      final dto = await rust.remoteScanStatus(sourceId: source.id);
      if (dto == null) {
        return RemoteScanStatus(
          sourceId: job.sourceId,
          status: job.status,
          mode: job.mode,
          generation: job.generation,
        );
      }
      final status = _fromStatusDto(dto);
      if (status.status != 'running') return status;
      await Future<void>.delayed(const Duration(milliseconds: 500));
    }
  }

  static Future<void> _nativePause(String sourceId) =>
      rust.remoteScanPause(sourceId: sourceId);
  static Future<void> _nativeResume(String sourceId) =>
      rust.remoteScanResume(sourceId: sourceId);
  static Future<void> _nativeCancel(String sourceId) =>
      rust.remoteScanCancel(sourceId: sourceId);

  static Future<RemoteScanStatus?> _nativeStatus(String sourceId) async {
    final dto = await rust.remoteScanStatus(sourceId: sourceId);
    return dto == null ? null : _fromStatusDto(dto);
  }

  Future<void> dispose() async {
    if (_disposed) return;
    _disposed = true;
    await _sessionSubscription.cancel();
    await _coverSubscription?.cancel();
    _coverSubscription = null;
    for (final notifier in _coverRevisions.values) {
      notifier.dispose();
    }
    _coverRevisions.clear();
    _lastSeenCoverRevision.clear();
    _settingsListenable.removeListener(_onSettingsChanged);
    for (final timer in _progressTimers.values) {
      timer.cancel();
    }
    _progressTimers.clear();
    _progressPollInFlight.clear();
    for (final notifier in _statuses.values) {
      notifier.dispose();
    }
    for (final notifier in _viewStates.values) {
      notifier.dispose();
    }
  }

  static RemoteScanStatus _fromStatusDto(rust.RemoteScanStatusDto dto) {
    return RemoteScanStatus(
      sourceId: dto.sourceId,
      status: dto.status,
      mode: dto.mode,
      generation: dto.generation,
      checkpoint: dto.checkpoint,
      lastSuccess: dto.lastSuccessAt == null
          ? null
          : DateTime.fromMillisecondsSinceEpoch(dto.lastSuccessAt!),
      errorCode: dto.errorCode,
      processed: dto.processed.toInt(),
      total: dto.total.toInt(),
      listingPhase: dto.listingPhase,
      directoriesChecked: dto.directoriesChecked.toInt(),
      discoveredBooks: dto.discoveredBooks.toInt(),
      discoveryComplete: dto.discoveryComplete,
      readyBooks: dto.readyBooks.toInt(),
      activeBooks: dto.activeBooks.toInt(),
      pendingBooks: dto.pendingBooks.toInt(),
      retryBooks: dto.retryBooks.toInt(),
      blockedBooks: dto.blockedBooks.toInt(),
      unsupportedBooks: dto.unsupportedBooks.toInt(),
      failedBooks: dto.failedBooks.toInt(),
      availableBooks: dto.availableBooks.toInt(),
      waitingBooks: dto.waitingBooks.toInt(),
      otherBooks: dto.otherBooks.toInt(),
      viewRevision: dto.viewRevision.toInt(),
    );
  }
}
