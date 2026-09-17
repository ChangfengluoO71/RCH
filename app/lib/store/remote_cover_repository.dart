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

typedef RemoteCoverRequestLoader =
    Future<rust.RemoteCoverStateDto> Function({
      required String sourceId,
      required BigInt session,
      required String assetId,
      required String consumerId,
      required rust.CoverSelectionDto selection,
      required rust.CoverProfileDto profile,
    });

typedef RemoteCoverReleaseLoader =
    Future<void> Function({required String consumerId});

/// All new cloud-cover UI code should use this class.  Existing provider
/// cover functions remain available as compatibility wrappers, but they are
/// not called by this repository.
class RemoteCoverRepository {
  RemoteCoverRepository({
    RemoteDirectoryViewLoader? directoryLoader,
    RemoteCoverReadLoader? readLoader,
    RemoteCoverRequestLoader? requestLoader,
    RemoteCoverReleaseLoader? releaseLoader,
  }) : _directoryLoader = directoryLoader ?? _defaultDirectoryLoader,
       _readLoader = readLoader ?? _defaultReadLoader,
       _requestLoader = requestLoader ?? _defaultRequestLoader,
       _releaseLoader = releaseLoader ?? _defaultReleaseLoader;

  static final instance = RemoteCoverRepository();

  final RemoteDirectoryViewLoader _directoryLoader;
  final RemoteCoverReadLoader _readLoader;
  final RemoteCoverRequestLoader _requestLoader;
  final RemoteCoverReleaseLoader _releaseLoader;

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
