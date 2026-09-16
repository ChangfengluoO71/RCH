import 'dart:async';

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

class RemoteSessionSuccessHub {
  final _events = StreamController<RemoteSessionSuccess>.broadcast(sync: true);
  Stream<RemoteSessionSuccess> get events => _events.stream;
  void emit(BookSource source, BigInt session) =>
      _events.add(RemoteSessionSuccess(source, session));
  Future<void> dispose() => _events.close();
}

final remoteSessionSuccessHub = RemoteSessionSuccessHub();

class RemoteScanCoordinator {
  RemoteScanCoordinator({
    RemoteScanStart? start,
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
    DateTime Function()? clock,
  }) : _start = start ?? _nativeStart,
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
  final RemoteScanStartWithInitialListing? _startWithInitialListing;
  final RemoteScanControl _pause;
  final RemoteScanControl _resume;
  final RemoteScanControl _cancel;
  final RemoteScanStatusLoader _status;
  final bool Function() _automaticEnabled;
  final bool Function() _coverFetchEnabled;
  final Listenable _settingsListenable;
  final Duration _debounce;
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
  final Set<String> _deferredRootListings = {};

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
      _observedSources.add(source.id);
      if (status.status == 'running' || status.status == 'paused') {
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
    return _startShared(source, session, mode);
  }

  Future<RemoteScanStatus> rescanIncremental(
    BookSource source,
    BigInt session,
  ) => rescan(source, session, 'incremental');

  Future<RemoteScanStatus> rescanFull(BookSource source, BigInt session) =>
      rescan(source, session, 'full');

  Future<void> pause(String sourceId) async {
    await _pause(sourceId);
    _setControlState(sourceId, 'paused');
  }

  Future<void> resume(String sourceId) async {
    await _resume(sourceId);
    _setControlState(sourceId, 'running');
  }

  Future<void> cancel(String sourceId) async {
    await _cancel(sourceId);
    _setControlState(sourceId, 'cancelled');
  }

  Future<RemoteScanStatus> _startShared(
    BookSource source,
    BigInt session,
    String mode, {
    String? initialListingJson,
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
    final startFuture =
        initialListingJson != null && _startWithInitialListing != null
        ? _startWithInitialListing(
            source: source,
            session: session,
            rootPath: source.effectiveRootPath,
            mode: mode,
            initialListingJson: initialListingJson,
          )
        : _start(
            source: source,
            session: session,
            rootPath: source.effectiveRootPath,
            mode: mode,
          );
    future = startFuture
        .then((status) {
          _observedSources.add(source.id);
          _setStatus(source.id, status);
          if (status.status == 'complete') {
            _completedAt[source.id] = _clock();
            _lastCompleted[source.id] = status;
          }
          return status;
        })
        .whenComplete(() {
          if (identical(_inflight[source.id], future)) {
            _inflight.remove(source.id);
            _inflightModes.remove(source.id);
          }
        });
    _inflight[source.id] = future;
    _inflightModes[source.id] = mode;
    return future;
  }

  void _setControlState(String sourceId, String state) {
    final notifier = _statuses.putIfAbsent(sourceId, () => ValueNotifier(null));
    final current = notifier.value;
    if (current != null) _setStatus(sourceId, current.copyWith(status: state));
  }

  void _setStatus(String sourceId, RemoteScanStatus status) {
    _statuses.putIfAbsent(sourceId, () => ValueNotifier(null)).value = status;
    _viewStates
        .putIfAbsent(sourceId, () => ValueNotifier(null))
        .value = RemoteScanViewState.fromStatus(
      status,
      coverFetchPaused: !_coverFetchEnabled(),
    );
  }

  void _onSettingsChanged() {
    for (final entry in _statuses.entries) {
      final status = entry.value.value;
      if (status == null) continue;
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
    await _sessionSubscription.cancel();
    _settingsListenable.removeListener(_onSettingsChanged);
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
    );
  }
}
