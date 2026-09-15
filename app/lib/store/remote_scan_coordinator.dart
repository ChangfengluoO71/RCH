import 'dart:async';

import 'package:app/src/rust/api/remote_scan.dart' as rust;
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
typedef RemoteScanControl = Future<void> Function(String sourceId);

class RemoteScanCoordinator {
  RemoteScanCoordinator({
    RemoteScanStart? start,
    RemoteScanControl? pauseCall,
    RemoteScanControl? resumeCall,
    RemoteScanControl? cancelCall,
  }) : _start = start ?? _nativeStart,
       _pause = pauseCall ?? _nativePause,
       _resume = resumeCall ?? _nativeResume,
       _cancel = cancelCall ?? _nativeCancel;

  static final instance = RemoteScanCoordinator();

  final RemoteScanStart _start;
  final RemoteScanControl _pause;
  final RemoteScanControl _resume;
  final RemoteScanControl _cancel;
  final Map<String, Future<RemoteScanStatus>> _inflight = {};
  final Set<String> _observedSources = {};
  final Map<String, ValueNotifier<RemoteScanStatus?>> _statuses = {};

  ValueListenable<RemoteScanStatus?> statusFor(String sourceId) =>
      _statuses.putIfAbsent(sourceId, () => ValueNotifier(null));

  Future<void> restoreStatuses(Iterable<BookSource> sources) async {
    for (final source in sources.where((source) => source.needsSession)) {
      final dto = await rust.remoteScanStatus(sourceId: source.id);
      if (dto == null) continue;
      final status = _fromStatusDto(dto);
      _statuses.putIfAbsent(source.id, () => ValueNotifier(null)).value =
          status;
      _observedSources.add(source.id);
    }
  }

  Future<RemoteScanStatus> ensureForSession(BookSource source, BigInt session) {
    final existing = _inflight[source.id];
    if (existing != null) return existing;
    final mode = _observedSources.contains(source.id) ? 'incremental' : 'full';
    return _startShared(source, session, mode);
  }

  Future<RemoteScanStatus> rescan(
    BookSource source,
    BigInt session,
    String mode,
  ) {
    if (mode != 'incremental' && mode != 'full') {
      return Future.error(ArgumentError.value(mode, 'mode'));
    }
    return _inflight[source.id] ?? _startShared(source, session, mode);
  }

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
    String mode,
  ) {
    late final Future<RemoteScanStatus> future;
    future =
        _start(
              source: source,
              session: session,
              rootPath: source.effectiveRootPath,
              mode: mode,
            )
            .then((status) {
              _observedSources.add(source.id);
              _statuses
                      .putIfAbsent(source.id, () => ValueNotifier(null))
                      .value =
                  status;
              return status;
            })
            .whenComplete(() {
              if (identical(_inflight[source.id], future)) {
                _inflight.remove(source.id);
              }
            });
    _inflight[source.id] = future;
    return future;
  }

  void _setControlState(String sourceId, String state) {
    final notifier = _statuses.putIfAbsent(sourceId, () => ValueNotifier(null));
    final current = notifier.value;
    if (current != null) notifier.value = current.copyWith(status: state);
  }

  static Future<RemoteScanStatus> _nativeStart({
    required BookSource source,
    required BigInt session,
    required String rootPath,
    required String mode,
  }) async {
    final job = await rust.remoteScanStart(
      sourceType: source.type,
      sourceId: source.id,
      session: session,
      rootPath: rootPath,
      mode: mode,
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
