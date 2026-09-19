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
  final String listingPhase;
  final int directoriesChecked;
  final int discoveredBooks;
  final bool discoveryComplete;
  final int readyBooks;
  final int activeBooks;
  final int pendingBooks;
  final int retryBooks;
  final int blockedBooks;
  final int unsupportedBooks;
  final int failedBooks;
  /// P1-F：`ready` 且字节真的可用（缓存/文件系统校验）。
  final int availableBooks;
  /// P1-F：等待中 = pending + retry + stale-ready + no-job。
  final int waitingBooks;
  /// P1-F：真实未知 durable state（默认 0）。
  final int otherBooks;
  final int viewRevision;

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
    this.listingPhase = 'pending',
    this.directoriesChecked = 0,
    this.discoveredBooks = 0,
    this.discoveryComplete = false,
    this.readyBooks = 0,
    this.activeBooks = 0,
    this.pendingBooks = 0,
    this.retryBooks = 0,
    this.blockedBooks = 0,
    this.unsupportedBooks = 0,
    this.failedBooks = 0,
    this.availableBooks = 0,
    this.waitingBooks = 0,
    this.otherBooks = 0,
    this.viewRevision = 0,
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
      listingPhase: listingPhase,
      directoriesChecked: directoriesChecked,
      discoveredBooks: discoveredBooks,
      discoveryComplete: discoveryComplete,
      readyBooks: readyBooks,
      activeBooks: activeBooks,
      pendingBooks: pendingBooks,
      retryBooks: retryBooks,
      blockedBooks: blockedBooks,
      unsupportedBooks: unsupportedBooks,
      failedBooks: failedBooks,
      availableBooks: availableBooks,
      waitingBooks: waitingBooks,
      otherBooks: otherBooks,
      viewRevision: viewRevision,
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
  final String listingPhase;
  final int directoriesChecked;
  final int discoveredBooks;
  final bool discoveryComplete;
  final int readyBooks;
  final int activeBooks;
  final int pendingBooks;
  final int retryBooks;
  final int blockedBooks;
  final int unsupportedBooks;
  final int failedBooks;
  /// P1-F：`ready` 且字节真的可用（缓存/文件系统校验）。
  final int availableBooks;
  /// P1-F：等待中 = pending + retry + stale-ready + no-job。
  final int waitingBooks;
  /// P1-F：真实未知 durable state（默认 0）。
  final int otherBooks;

  const RemoteScanViewState({
    required this.sourceId,
    required this.status,
    required this.mode,
    this.processed = 0,
    this.total = 0,
    this.lastSuccess,
    this.errorCode,
    this.coverFetchPaused = false,
    this.listingPhase = 'pending',
    this.directoriesChecked = 0,
    this.discoveredBooks = 0,
    this.discoveryComplete = false,
    this.readyBooks = 0,
    this.activeBooks = 0,
    this.pendingBooks = 0,
    this.retryBooks = 0,
    this.blockedBooks = 0,
    this.unsupportedBooks = 0,
    this.failedBooks = 0,
    this.availableBooks = 0,
    this.waitingBooks = 0,
    this.otherBooks = 0,
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
      listingPhase: status.listingPhase,
      directoriesChecked: status.directoriesChecked,
      discoveredBooks: status.discoveredBooks,
      discoveryComplete: status.discoveryComplete,
      readyBooks: status.readyBooks,
      activeBooks: status.activeBooks,
      pendingBooks: status.pendingBooks,
      retryBooks: status.retryBooks,
      blockedBooks: status.blockedBooks,
      unsupportedBooks: status.unsupportedBooks,
      failedBooks: status.failedBooks,
      availableBooks: status.availableBooks,
      waitingBooks: status.waitingBooks,
      otherBooks: status.otherBooks,
    );
  }
}

/// Convert provider/native errors into short Chinese UI messages. The raw
/// exception may contain an endpoint, HTML response, cookie or token, so it
/// must never be rendered directly in a browser/snackbar.
String remoteErrorMessage(Object error, {String fallback = '远程请求失败，请稍后重试'}) {
  final raw = error.toString().toLowerCase();
  // 正在扫描时再次点"重新扫描"不是错误，但过去会落到 fallback 显示成"远程请求失败"
  // （用户实测："点重新扫描显示失败"）。这里给出真实原因。
  if (raw.contains('remotescanalreadyrunning') ||
      raw.contains('already running')) {
    return '该源正在扫描中，请等本次扫描完成';
  }
  if (raw.contains('登录状态') ||
      raw.contains('auth') ||
      raw.contains('unauthorized') ||
      raw.contains('cookie 过期') ||
      raw.contains('重新扫码')) {
    return '登录状态已失效，请重新授权';
  }
  if (raw.contains('403') ||
      raw.contains('forbidden') ||
      raw.contains('拒绝访问')) {
    return '远程服务拒绝访问，请检查授权或权限';
  }
  if (raw.contains('404') || raw.contains('notfound') || raw.contains('不存在')) {
    return '远程文件不存在或已被删除';
  }
  if (raw.contains('405') || raw.contains('method not allowed')) {
    return '远程服务暂不支持此请求';
  }
  if (raw.contains('rangeunavailable') ||
      raw.contains('range 不可用') ||
      raw.contains('range_probe')) {
    return '该文件不支持分段读取，已使用占位封面';
  }
  if (raw.contains('timeout') ||
      raw.contains('timed out') ||
      raw.contains('network') ||
      raw.contains('connection') ||
      raw.contains('断网') ||
      raw.contains('网络')) {
    return '网络连接失败，请稍后重试';
  }
  return fallback;
}
