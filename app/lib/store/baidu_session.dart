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
    LibraryStore.instance.updateSource(source.id, refreshToken: s.refreshToken);
  }
  return s.id;
}

/// 书源被编辑/删除后使会话失效（下次自动重连）。
void clearBaiduSession(String sourceId) {
  _baiduSessions.remove(sourceId);
}
