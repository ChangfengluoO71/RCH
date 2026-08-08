import 'package:app/src/rust/api/book.dart';
import 'package:app/src/rust/api/source.dart';
import 'package:app/store/library_store.dart';
import 'package:app/store/models.dart';

/// 115 网盘会话缓存（按书源 id），避免每次打开都重新连接。
/// 分两种模式：官方 APP ID（refresh_token）与网页扫码（Cookie）。
final Map<String, BigInt> _cloud115OpenSessions = {};
final Map<String, BigInt> _cloud115CookieSessions = {};

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
    LibraryStore.instance.updateSource(source.id, refreshToken: s.refreshToken);
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
    LibraryStore.instance.updateSource(source.id, cookie: s.cookie);
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
    return cloud115CookieList(session: session, path: path);
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
    return openCloud115CookieBook(
        session: session, path: path, strategy: strategy);
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
    return cloud115CookieCover(
        session: session,
        path: path,
        page: page,
        width: width,
        height: height,
        crop: crop);
  }
  return cloud115Cover(
      session: session,
      path: path,
      page: page,
      width: width,
      height: height,
      crop: crop);
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
