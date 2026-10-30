import 'package:app/src/rust/api/book.dart';
import 'package:app/src/rust/api/source.dart';
import 'package:app/src/rust/source/cloud115/offline.dart' as cloud115_offline;
import 'package:app/store/ai_upscale_manager.dart';
import 'package:app/store/library_store.dart';
import 'package:app/store/models.dart';
import 'package:app/store/remote_scan_coordinator.dart';
import 'package:app/ui/cloud115_qr_scan.dart';

/// 115 网盘会话缓存（按书源 id），避免每次打开都重新连接。
/// 分两种模式：官方 APP ID（refresh_token）与网页扫码（Cookie）。
final Map<String, BigInt> _cloud115OpenSessions = {};
final Map<String, BigInt> _cloud115CookieSessions = {};
// A poster-wall rebuild and SourceBrowser can request the same provider
// session in the same frame. Keep one connection attempt per source so that
// concurrent callers do not create several Rust clients (and each run its own
// connectivity/list request) before the first result is cached.
final Map<String, Future<BigInt>> _cloud115OpenSessionFlights = {};
final Map<String, Future<BigInt>> _cloud115CookieSessionFlights = {};

/// 正在自动续期（弹扫码框）的 115 书源，避免并发请求重复弹窗。
final Set<String> _refreshing115Cookie = {};

bool _is115ExpiredError(Object e) {
  final s = e.toString();
  return s.contains('登录状态已失效') || s.contains('Cookie 过期') || s.contains('重新扫码');
}

/// Cookie 模式操作自动续期：失败若是「登录失效」类错误，
/// 自动弹扫码框 → 替换 Cookie → 清理会话缓存 → 重试一次。
/// 用户取消扫码则抛出原始错误；已在本源刷新中则直接抛出不重复弹窗。
Future<T> _cookieRetry<T>(BookSource source, Future<T> Function() op) async {
  if ((source.cookie ?? '').trim().isEmpty) return op();
  try {
    return await op();
  } catch (e) {
    if (!_is115ExpiredError(e) || _refreshing115Cookie.contains(source.id)) {
      rethrow;
    }
    _refreshing115Cookie.add(source.id);
    try {
      final refreshed = await _prompt115Rescan(source);
      if (!refreshed) rethrow;
      return await op();
    } finally {
      _refreshing115Cookie.remove(source.id);
    }
  }
}

/// 弹扫码框续期：成功则回写 Cookie 到书源与数据库并清会话。
Future<bool> _prompt115Rescan(BookSource source) async {
  final ctx = AiUpscaleManager.navigatorKey.currentContext;
  if (ctx == null) return false;
  final cookie = await scanCloud115Cookie(ctx);
  if (cookie == null || cookie.trim().isEmpty) return false;
  source.cookie = cookie;
  await LibraryStore.instance.updateSource(source.id, cookie: cookie);
  clearCloud115Session(source.id);
  return true;
}

/// 获取/重连 115 书源会话：Cookie 模式（`source.cookie` 非空）走网页接口，
/// 否则走官方 APP ID 模式。调用方无需感知模式差异。
Future<BigInt> cloud115SessionFor(BookSource source) {
  final cookie = (source.cookie ?? '').trim();
  if (cookie.isNotEmpty) return cloud115CookieSessionFor(source);
  return cloud115OpenSessionFor(source);
}

/// 官方 APP ID 模式：连接成功后回写刷新后的 refresh_token。
Future<BigInt> cloud115OpenSessionFor(BookSource source) {
  final cached = _cloud115OpenSessions[source.id];
  if (cached != null) {
    return Future.value(cached);
  }
  final existing = _cloud115OpenSessionFlights[source.id];
  if (existing != null) return existing;
  final requestedRefreshToken = source.refreshToken ?? '';
  final requestedClientId = source.clientId ?? '';
  final requestedRootId = source.rootId ?? '0';
  late final Future<BigInt> flight;
  flight =
      () async {
        final s = await cloud115Connect(
          refreshToken: requestedRefreshToken,
          appId: requestedClientId,
          rootId: requestedRootId,
        );
        // A source edit may invalidate this flight while the native connect
        // is still in progress. Do not repopulate the cache or overwrite the
        // newer credentials with that stale result.
        if ((source.refreshToken ?? '') == requestedRefreshToken &&
            (source.clientId ?? '') == requestedClientId &&
            (source.rootId ?? '0') == requestedRootId) {
          _cloud115OpenSessions[source.id] = s.id;
          if (s.refreshToken.isNotEmpty &&
              s.refreshToken != source.refreshToken) {
            source.refreshToken = s.refreshToken;
            await LibraryStore.instance.updateSource(
              source.id,
              refreshToken: s.refreshToken,
            );
          }
          remoteSessionSuccessHub.emit(source, s.id);
        }
        return s.id;
      }().whenComplete(() {
        if (identical(_cloud115OpenSessionFlights[source.id], flight)) {
          _cloud115OpenSessionFlights.remove(source.id);
        }
      });
  _cloud115OpenSessionFlights[source.id] = flight;
  return flight;
}

/// 网页扫码 Cookie 模式（无需 APP ID）：连接成功后回写 Cookie（如已变化）。
Future<BigInt> cloud115CookieSessionFor(BookSource source) {
  final cached = _cloud115CookieSessions[source.id];
  if (cached != null) {
    return Future.value(cached);
  }
  final existing = _cloud115CookieSessionFlights[source.id];
  if (existing != null) return existing;
  final requestedCookie = source.cookie ?? '';
  final requestedRootId = source.rootId ?? '0';
  late final Future<BigInt> flight;
  flight =
      () async {
        final s = await cloud115CookieConnect(
          cookie: requestedCookie,
          rootId: requestedRootId,
        );
        if ((source.cookie ?? '') == requestedCookie &&
            (source.rootId ?? '0') == requestedRootId) {
          _cloud115CookieSessions[source.id] = s.id;
          if (s.cookie.isNotEmpty && s.cookie != source.cookie) {
            source.cookie = s.cookie;
            await LibraryStore.instance.updateSource(
              source.id,
              cookie: s.cookie,
            );
          }
          remoteSessionSuccessHub.emit(source, s.id);
        }
        return s.id;
      }().whenComplete(() {
        if (identical(_cloud115CookieSessionFlights[source.id], flight)) {
          _cloud115CookieSessionFlights.remove(source.id);
        }
      });
  _cloud115CookieSessionFlights[source.id] = flight;
  return flight;
}

/// Run the one-item, read-only BT preflight on an already configured Cookie
/// source. No session is opened until the caller explicitly invokes this.
Future<cloud115_offline.Cloud115BtPreflight>
cloud115CookieBtReadonlyPreflightFor(
  BookSource source, {
  required String outputDir,
  required String relativeFile,
  required String expectedInfoHash,
  required cloud115_offline.Cloud115BtFolderSelection target,
}) async {
  if (!source.is115 || (source.cookie ?? '').trim().isEmpty) {
    throw StateError('请选择已有的 115 Cookie 书源');
  }
  final session = _cachedCookieSessionForBt(source);
  return cloud115CookieBtReadonlyPreflight(
    session: session,
    outputDir: outputDir,
    relativeFile: relativeFile,
    expectedInfoHash: expectedInfoHash,
    target: target,
  );
}

Future<void> cloud115CookieBtDeleteLocalIfRemoteTaskFoundFor(
  BookSource source, {
  required String outputDir,
  required String relativeFile,
  required String expectedInfoHash,
  required String expectedAccountId,
  required cloud115_offline.Cloud115BtFolderSelection target,
}) => cloud115CookieBtDeleteLocalIfRemoteTaskFound(
  session: _cachedCookieSessionForBt(source),
  outputDir: outputDir,
  relativeFile: relativeFile,
  expectedInfoHash: expectedInfoHash,
  expectedAccountId: expectedAccountId,
  target: target,
);

Future<List<cloud115_offline.Cloud115BtFolder>>
cloud115CookieBtListChildFoldersFor(
  BookSource source, {
  required String parentId,
}) => cloud115CookieBtListChildFolders(
  session: _cachedCookieSessionForBt(source),
  parentId: parentId,
);

BigInt _cachedCookieSessionForBt(BookSource source) {
  if (!source.is115 || (source.cookie ?? '').trim().isEmpty) {
    throw StateError('请选择已有的 115 Cookie 书源');
  }
  final session = _cloud115CookieSessions[source.id];
  if (session == null) {
    throw StateError(
      '当前没有已连接的 115 Cookie 会话。请先通过现有书源连接；此实验入口不会重新登录或刷新 Cookie。',
    );
  }
  return session;
}

/// Returns whether BT operations can reuse the source's already cached Cookie
/// session. This is an in-memory check only and never reconnects or refreshes.
bool hasCachedCloud115CookieSession(BookSource source) =>
    source.is115 &&
    (source.cookie ?? '').trim().isNotEmpty &&
    _cloud115CookieSessions.containsKey(source.id);

/// Restore a saved Cookie for a headless Daily run. This intentionally does
/// not invoke the interactive retry path, mutate a source, or persist a
/// server-returned Cookie: another engine may have updated the source since
/// this engine loaded its snapshot.
Future<void> restoreCloud115CookieSessionWithoutInteraction(
  BookSource source,
) async {
  if (hasCachedCloud115CookieSession(source)) return;
  final cookie = (source.cookie ?? '').trim();
  final rootId = source.rootId ?? '0';
  if (!source.is115 || cookie.isEmpty) {
    throw StateError('已保存的 115 Cookie 不可用');
  }
  final session = await cloud115CookieConnect(cookie: cookie, rootId: rootId);
  if ((source.cookie ?? '').trim() != cookie ||
      (source.rootId ?? '0') != rootId) {
    await cloud115CookieDisconnect(id: session.id);
    throw StateError('115 书源已更改，请重新检查配置');
  }
  if (hasCachedCloud115CookieSession(source)) {
    await cloud115CookieDisconnect(id: session.id);
    return;
  }
  _cloud115CookieSessions[source.id] = session.id;
}

/// Called only when a headless engine has finished all its operations.
Future<void> closeHeadlessCloud115CookieSessions() async {
  final sessions = _cloud115CookieSessions.values.toSet();
  _cloud115CookieSessions.clear();
  for (final id in sessions) {
    await cloud115CookieDisconnect(id: id);
  }
}

/// Reads the stable account number encoded in the configured Cookie without
/// opening a client, refreshing the Cookie, or issuing a request. Used only to
/// ensure monitoring rows are never queried under a different account.
String? cloud115CookieAccountIdHint(BookSource source) {
  if (!source.is115) return null;
  for (final part in (source.cookie ?? '').split(';')) {
    final value = part.trim();
    if (!value.startsWith('UID=')) continue;
    final id = value.substring(4).split('_').first;
    if (RegExp(r'^\d+$').hasMatch(id)) return id;
  }
  return null;
}

Future<cloud115_offline.Cloud115BtUploadParseResult>
cloud115CookieBtUploadAndParseFor(
  BookSource source, {
  required String outputDir,
  required String relativeFile,
  required String expectedInfoHash,
  required cloud115_offline.Cloud115BtFolderSelection target,
}) => cloud115CookieBtUploadAndParse(
  session: _cachedCookieSessionForBt(source),
  outputDir: outputDir,
  relativeFile: relativeFile,
  expectedInfoHash: expectedInfoHash,
  target: target,
);

Future<cloud115_offline.Cloud115BtUploadParseResult>
cloud115CookieBtConfirmUploadParseFor(
  BookSource source, {
  required String outputDir,
  required String relativeFile,
  required String expectedInfoHash,
  required cloud115_offline.Cloud115BtFolderSelection target,
  String? pickCode,
}) => cloud115CookieBtConfirmUploadParse(
  session: _cachedCookieSessionForBt(source),
  outputDir: outputDir,
  relativeFile: relativeFile,
  expectedInfoHash: expectedInfoHash,
  target: target,
  pickCode: pickCode,
);

Future<cloud115_offline.Cloud115BtSubmitResult> cloud115CookieBtSubmitOnceFor(
  BookSource source, {
  required String outputDir,
  required String relativeFile,
  required String expectedInfoHash,
  required cloud115_offline.Cloud115BtFolderSelection target,
  required List<int> wantedIndices,
}) => cloud115CookieBtSubmitOnce(
  session: _cachedCookieSessionForBt(source),
  outputDir: outputDir,
  relativeFile: relativeFile,
  expectedInfoHash: expectedInfoHash,
  target: target,
  wantedIndices: wantedIndices,
);

Future<cloud115_offline.Cloud115BtTaskStatus> cloud115CookieBtTaskStatusFor(
  BookSource source, {
  required String infoHash,
  required cloud115_offline.Cloud115BtFolderSelection target,
}) => cloud115CookieBtTaskStatus(
  session: _cachedCookieSessionForBt(source),
  infoHash: infoHash,
  target: target,
);

Future<cloud115_offline.Cloud115BtTaskSnapshot> cloud115CookieBtTaskSnapshotFor(
  BookSource source,
) => cloud115CookieBtTaskSnapshot(session: _cachedCookieSessionForBt(source));

/// 书源被编辑/删除后使会话失效（下次自动重连）。
void clearCloud115Session(String sourceId) {
  _cloud115OpenSessions.remove(sourceId);
  _cloud115CookieSessions.remove(sourceId);
  _cloud115OpenSessionFlights.remove(sourceId);
  _cloud115CookieSessionFlights.remove(sourceId);
}

// ------------------------------------------------------------
// 下游调用封装：按书源模式（Cookie / 官方 APP ID）自动选择对应函数，
// 调用方只需传入 BookSource，无需关心模式。
// ------------------------------------------------------------

/// 列出 115 目录。
Future<List<DirEntry>> cloud115ListFor(
  BookSource source, {
  required BigInt session,
  required String path,
}) {
  if ((source.cookie ?? '').isNotEmpty) {
    return _cookieRetry(source, () async {
      final s = await cloud115CookieSessionFor(source);
      return cloud115CookieList(session: s, path: path);
    });
  }
  return cloud115List(session: session, path: path);
}

/// 打开 115 上的书籍。
Future<BookInfo> openCloud115BookFor(
  BookSource source, {
  required BigInt session,
  required String path,
  required String strategy,
}) {
  if ((source.cookie ?? '').isNotEmpty) {
    return _cookieRetry(source, () async {
      final s = await cloud115CookieSessionFor(source);
      return openCloud115CookieBook(session: s, path: path, strategy: strategy);
    });
  }
  return openCloud115Book(session: session, path: path, strategy: strategy);
}

/// 115 书籍是否已有 raw/ 本地缓存。
Future<bool> cloud115HasRawCacheFor(
  BookSource source, {
  required BigInt session,
  required String path,
}) {
  if ((source.cookie ?? '').isNotEmpty) {
    return cloud115CookieHasRawCache(session: session, path: path);
  }
  return cloud115HasRawCache(session: session, path: path);
}

/// 取 115 书籍封面。
Future<PageImage> cloud115CoverFor(
  BookSource source, {
  required BigInt session,
  required String path,
  required int page,
  required int width,
  required int height,
  CropRect? crop,
}) {
  if ((source.cookie ?? '').isNotEmpty) {
    return _cookieRetry(source, () async {
      final s = await cloud115CookieSessionFor(source);
      return cloud115CookieCover(
        session: s,
        path: path,
        page: page,
        width: width,
        height: height,
        crop: crop,
      );
    });
  }
  return cloud115Cover(
    session: session,
    path: path,
    page: page,
    width: width,
    height: height,
    crop: crop,
  );
}

/// 115 下载进度（0.0~1.0，按书源模式选择）。
Future<double> cloud115DownloadProgressFor(
  BookSource source, {
  required BigInt session,
}) {
  if ((source.cookie ?? '').isNotEmpty) {
    return cloud115CookieDownloadProgress(session: session);
  }
  return cloud115DownloadProgress(session: session);
}
