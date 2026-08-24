import 'package:app/src/rust/api/source.dart';
import 'package:app/store/library_store.dart';
import 'package:app/store/models.dart';

/// 百度网盘会话缓存（按书源 id），避免每次打开都重新连接。
final Map<String, BigInt> _baiduSessions = {};

/// 获取/重连百度网盘书源会话；连接成功后回写刷新后的 refresh_token。
Future<BigInt> baiduSessionFor(BookSource source) async {
  final cached = _baiduSessions[source.id];
  if (cached != null) return cached;
  final s = await baiduConnect(
    refreshToken: source.refreshToken ?? '',
    appKey: source.clientId ?? '',
    clientSecret: source.clientSecret ?? '',
    root: source.path,
  );
  _baiduSessions[source.id] = s.id;
  if (s.refreshToken.isNotEmpty && s.refreshToken != source.refreshToken) {
    source.refreshToken = s.refreshToken;
    await LibraryStore.instance.updateSource(
      source.id,
      refreshToken: s.refreshToken,
    );
  }
  return s.id;
}

/// 强制重新连接百度网盘书源：清掉会话缓存后重连，Rust 侧每次 connect 都会
/// 调用 refresh_token 接口轮换 refresh_token，成功后将最新 token 回写 DB。
/// 当 token 被顶掉/失效（如同 AppKey 多书源互踢、下载报 31045）时手动调用。
Future<BigInt> baiduRefreshTokenFor(BookSource source) async {
  clearBaiduSession(source.id);
  return baiduSessionFor(source);
}

/// 书源被编辑/删除后使会话失效（下次自动重连）。
void clearBaiduSession(String sourceId) {
  _baiduSessions.remove(sourceId);
}
