import 'package:app/src/rust/api/book.dart';
import 'package:app/src/rust/api/remote_cover.dart' as rust;
import 'package:app/store/models.dart';

/// Unified Flutter entry point for the local remote-catalog projection and
/// cover queue.  The repository intentionally keeps the generated FRB calls
/// behind a small injectable boundary so widget tests never need a provider
/// session or network service.
typedef RemoteDirectoryViewLoader =
    Future<rust.RemoteDirectoryViewDto> Function({
      required String sourceId,
      required String logicalPath,
      required int offset,
      required int limit,
    });

typedef RemoteCoverReadLoader =
    Future<PageImage?> Function({
      required String sourceId,
      required String assetId,
      required rust.CoverSelectionDto selection,
      required rust.CoverProfileDto profile,
    });

/// Reads a cover that is already materialized locally (memory, cover-disk, or
/// raw-local cache) without creating a session or touching the network.  The
/// disk-first caller uses it to settle the local question before consulting
/// the remote network gate, so a disabled gate never hides a local cover.
typedef RemoteCoverLocalReadLoader =
    Future<PageImage?> Function({
      required String sourceId,
      required String assetId,
      required rust.CoverSelectionDto selection,
      required rust.CoverProfileDto profile,
    });

typedef RemoteCoverRequestLoader =
    Future<rust.RemoteCoverStateDto> Function({
      required String sourceId,
      required BigInt session,
      required String assetId,
      required String consumerId,
      required rust.CoverSelectionDto selection,
      required rust.CoverProfileDto profile,
    });

/// P1-E：只读读取 durable cover state（返回 null = 该 asset 无任何记录）。
typedef RemoteCoverStateLoader =
    Future<rust.RemoteCoverStateDto?> Function({
      required String sourceId,
      required String assetId,
      required rust.CoverSelectionDto selection,
      required rust.CoverProfileDto profile,
    });

/// 主动重试失败封面：返回本次重新排队的任务数（0 = 没有可重排的失败）。
typedef RemoteCoverRetryFailedLoader =
    Future<int> Function({required String sourceId, required int limit});

typedef RemoteCoverReleaseLoader =
    Future<void> Function({required String consumerId});

/// All new cloud-cover UI code should use this class.  Existing provider
/// cover functions remain available as compatibility wrappers, but they are
/// not called by this repository.
class RemoteCoverRepository {
  RemoteCoverRepository({
    RemoteDirectoryViewLoader? directoryLoader,
    RemoteCoverReadLoader? readLoader,
    RemoteCoverLocalReadLoader? localReadLoader,
    RemoteCoverRequestLoader? requestLoader,
    RemoteCoverReleaseLoader? releaseLoader,
    RemoteCoverStateLoader? stateLoader,
    RemoteCoverRetryFailedLoader? retryFailedLoader,
  }) : _retryFailedLoader = retryFailedLoader ?? _defaultRetryFailedLoader,
       _directoryLoader = directoryLoader ?? _defaultDirectoryLoader,
       _readLoader = readLoader ?? _defaultReadLoader,
       _localReadLoader = localReadLoader ?? readLoader ?? _defaultReadLoader,
       _requestLoader = requestLoader ?? _defaultRequestLoader,
       _releaseLoader = releaseLoader ?? _defaultReleaseLoader,
       _stateLoader = stateLoader ?? _defaultStateLoader;

  static final instance = RemoteCoverRepository();

  final RemoteDirectoryViewLoader _directoryLoader;
  final RemoteCoverReadLoader _readLoader;
  final RemoteCoverLocalReadLoader _localReadLoader;
  final RemoteCoverRequestLoader _requestLoader;
  final RemoteCoverReleaseLoader _releaseLoader;
  final RemoteCoverStateLoader _stateLoader;
  final RemoteCoverRetryFailedLoader _retryFailedLoader;

  /// 用户主动"重试失败封面"（2026-09-21 真机：墙上大片"获取失败"，而失败原因早已修好，
  /// 终态 failed 却是粘性的）。轻量：只重排该源**当前档**的失败，不动其它档、不清缓存。
  Future<int> retryFailed({required String sourceId, int limit = 200}) =>
      _retryFailedLoader(sourceId: sourceId, limit: limit < 1 ? 1 : (limit > 500 ? 500 : limit));

  Future<rust.RemoteDirectoryViewDto> directoryView({
    required BookSource source,
    required String logicalPath,
    int offset = 0,
    int limit = 200,
  }) => _directoryLoader(
    sourceId: source.id,
    logicalPath: logicalPath,
    offset: offset,
    limit: limit < 0 ? 0 : (limit > 200 ? 200 : limit),
  );

  Future<PageImage?> readCover({
    required String sourceId,
    required String assetId,
    required rust.CoverSelectionDto selection,
    required rust.CoverProfileDto profile,
  }) => _readLoader(
    sourceId: sourceId,
    assetId: assetId,
    selection: selection,
    profile: profile,
  );

  /// Returns a cover that already exists locally, or null on a local miss.
  ///
  /// This is the disk-first read: it never creates a session, never requests a
  /// downlink, and never consults the remote network gate, so an existing
  /// local/disk cover stays displayable while remote fetching is disabled.
  Future<PageImage?> readLocalCover({
    required String sourceId,
    required String assetId,
    required rust.CoverSelectionDto selection,
    required rust.CoverProfileDto profile,
  }) => _localReadLoader(
    sourceId: sourceId,
    assetId: assetId,
    selection: selection,
    profile: profile,
  );

  /// P1-E：**只读**读取某 asset 的 durable cover state。
  ///
  /// 不 enqueue、不 request、不建 session、不访问 provider、不改 durable state ——
  /// 这是"后续 wake 只重读、绝不重复 request"所依赖的读入口。
  ///
  /// 第 79 轮续（真机 bug）：`selection` + `profile` 必须与卡片**实际请求/读取图片**
  /// 用的那组键一致。此前 Rust 侧写死读 `default`/`340x480@1`，而卡片按
  /// `coverQuality`（low=170x240）取图 ⇒ 图已 ready 但墙面仍按另一 profile 的旧
  /// `failed` 显示"获取失败"。
  Future<rust.RemoteCoverStateDto?> readState({
    required String sourceId,
    required String assetId,
    required rust.CoverSelectionDto selection,
    required rust.CoverProfileDto profile,
  }) => _stateLoader(
    sourceId: sourceId,
    assetId: assetId,
    selection: selection,
    profile: profile,
  );

  static Future<int> _defaultRetryFailedLoader({
    required String sourceId,
    required int limit,
  }) => rust.remoteCoverRetryFailed(sourceId: sourceId, limit: limit);

  static Future<rust.RemoteCoverStateDto?> _defaultStateLoader({
    required String sourceId,
    required String assetId,
    required rust.CoverSelectionDto selection,
    required rust.CoverProfileDto profile,
  }) => rust.remoteCoverState(
    sourceId: sourceId,
    assetId: assetId,
    selection: selection,
    profile: profile,
  );

  Future<rust.RemoteCoverStateDto> requestCover({
    required BookSource source,
    required BigInt session,
    required String assetId,
    required String consumerId,
    required rust.CoverSelectionDto selection,
    required rust.CoverProfileDto profile,
  }) => _requestLoader(
    sourceId: source.id,
    session: session,
    assetId: assetId,
    consumerId: consumerId,
    selection: selection,
    profile: profile,
  );

  Future<void> release({required String consumerId}) =>
      _releaseLoader(consumerId: consumerId);

  static Future<rust.RemoteDirectoryViewDto> _defaultDirectoryLoader({
    required String sourceId,
    required String logicalPath,
    required int offset,
    required int limit,
  }) => rust.remoteDirectoryView(
    sourceId: sourceId,
    logicalPath: logicalPath,
    offset: offset,
    limit: limit,
  );

  static Future<PageImage?> _defaultReadLoader({
    required String sourceId,
    required String assetId,
    required rust.CoverSelectionDto selection,
    required rust.CoverProfileDto profile,
  }) => rust.remoteCoverRead(
    sourceId: sourceId,
    assetId: assetId,
    selection: selection,
    profile: profile,
  );

  static Future<rust.RemoteCoverStateDto> _defaultRequestLoader({
    required String sourceId,
    required BigInt session,
    required String assetId,
    required String consumerId,
    required rust.CoverSelectionDto selection,
    required rust.CoverProfileDto profile,
  }) => rust.remoteCoverRequest(
    sourceId: sourceId,
    session: session,
    assetId: assetId,
    consumerId: consumerId,
    selection: selection,
    profile: profile,
  );

  static Future<void> _defaultReleaseLoader({required String consumerId}) =>
      rust.remoteCoverRelease(consumerId: consumerId);
}

String remoteCoverStateLabel(String state) {
  switch (state.trim().toLowerCase()) {
    case 'pending':
      return '等待处理';
    case 'running':
      return '封面获取中';
    case 'ready':
      return '已就绪';
    case 'retry_wait':
      return '稍后重试';
    case 'blocked':
      return '等待授权';
    case 'unsupported':
      return '暂不支持局部读取';
    case 'cancelled':
      return '已取消';
    case 'failed':
      return '封面获取失败';
    default:
      return '等待处理';
  }
}
