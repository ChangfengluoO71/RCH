import 'package:app/src/rust/api/source.dart';
import 'package:app/store/library_store.dart';
import 'package:app/store/models.dart';
import 'package:app/store/netdisk_credentials.dart';

/// 115 网盘会话缓存（按书源 id），避免每次打开都重新连接。
final Map<String, BigInt> _cloud115Sessions = {};

/// 获取/重连 115 网盘书源会话；连接成功后回写刷新后的 refresh_token。
Future<BigInt> cloud115SessionFor(BookSource source) async {
  final cached = _cloud115Sessions[source.id];
  if (cached != null) return cached;
  final s = await cloud115Connect(
    refreshToken: source.refreshToken ?? '',
    appId: (source.clientId?.isNotEmpty ?? false) ? source.clientId! : kCloud115DefaultAppId,
    rootId: source.rootId ?? '0',
  );
  _cloud115Sessions[source.id] = s.id;
  if (s.refreshToken.isNotEmpty && s.refreshToken != source.refreshToken) {
    source.refreshToken = s.refreshToken;
    LibraryStore.instance.updateSource(source.id, refreshToken: s.refreshToken);
  }
  return s.id;
}

/// 书源被编辑/删除后使会话失效（下次自动重连）。
void clearCloud115Session(String sourceId) {
  _cloud115Sessions.remove(sourceId);
}
