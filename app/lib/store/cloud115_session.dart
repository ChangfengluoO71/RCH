import 'package:app/src/rust/api/book.dart';
import 'package:app/src/rust/api/source.dart';
import 'package:app/store/ai_upscale_manager.dart';
import 'package:app/store/library_store.dart';
import 'package:app/store/models.dart';
import 'package:app/ui/cloud115_qr_scan.dart';

/// 115 网盘会话缓存（按书源 id），避免每次打开都重新连接。
/// 分两种模式：官方 APP ID（refresh_token）与网页扫码（Cookie）。
final Map<String, BigInt> _cloud115OpenSessions = {};
final Map<String, BigInt> _cloud115CookieSessions = {};

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
Future<BigInt> cloud115OpenSessionFor(BookSource source) async {
  final cached = _cloud115OpenSessions[source.id];
  if (cached != null) return cached;
  final s = await cloud115Connect(
    refreshToken: source.refreshToken ?? '',
    appId: source.clientId ?? '',
    rootId: source.rootId ?? '0',
  );
  _cloud115OpenSessions[source.id] = s.id;
  if (s.refreshToken.isNotEmpty && s.refreshToken != source.refreshToken) {
    source.refreshToken = s.refreshToken;
    await LibraryStore.instance.updateSource(
      source.id,
      refreshToken: s.refreshToken,
    );
  }
  return s.id;
}

/// 网页扫码 Cookie 模式（无需 APP ID）：连接成功后回写 Cookie（如已变化）。
Future<BigInt> cloud115CookieSessionFor(BookSource source) async {
  final cached = _cloud115CookieSessions[source.id];
  if (cached != null) return cached;
  final s = await cloud115CookieConnect(
    cookie: source.cookie ?? '',
    rootId: source.rootId ?? '0',
  );
  _cloud115CookieSessions[source.id] = s.id;
  if (s.cookie.isNotEmpty && s.cookie != source.cookie) {
    source.cookie = s.cookie;
    await LibraryStore.instance.updateSource(source.id, cookie: s.cookie);
  }
  return s.id;
}

/// 书源被编辑/删除后使会话失效（下次自动重连）。
void clearCloud115Session(String sourceId) {
  _cloud115OpenSessions.remove(sourceId);
  _cloud115CookieSessions.remove(sourceId);
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
