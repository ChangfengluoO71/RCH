class RemoteScanStatus {
  final String sourceId;
  final String status;
  final String mode;
  final int generation;
  final String? checkpoint;
  final DateTime? lastSuccess;
  final String? errorCode;
  final int processed;
  final int total;

  const RemoteScanStatus({
    required this.sourceId,
    required this.status,
    required this.mode,
    required this.generation,
    this.checkpoint,
    this.lastSuccess,
    this.errorCode,
    this.processed = 0,
    this.total = 0,
  });

  RemoteScanStatus copyWith({String? status, String? errorCode}) {
    return RemoteScanStatus(
      sourceId: sourceId,
      status: status ?? this.status,
      mode: mode,
      generation: generation,
      checkpoint: checkpoint,
      lastSuccess: lastSuccess,
      errorCode: errorCode ?? this.errorCode,
      processed: processed,
      total: total,
    );
  }
}

class RemoteScanViewState {
  final String sourceId;
  final String status;
  final String mode;
  final int processed;
  final int total;
  final DateTime? lastSuccess;
  final String? errorCode;
  final bool coverFetchPaused;

  const RemoteScanViewState({
    required this.sourceId,
    required this.status,
    required this.mode,
    this.processed = 0,
    this.total = 0,
    this.lastSuccess,
    this.errorCode,
    this.coverFetchPaused = false,
  });

  factory RemoteScanViewState.fromStatus(
    RemoteScanStatus status, {
    bool coverFetchPaused = false,
  }) {
    return RemoteScanViewState(
      sourceId: status.sourceId,
      status: status.status,
      mode: status.mode,
      processed: status.processed,
      total: status.total,
      lastSuccess: status.lastSuccess,
      errorCode: status.errorCode,
      coverFetchPaused: coverFetchPaused,
    );
  }
}
