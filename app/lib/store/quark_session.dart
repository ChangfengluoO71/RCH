import 'package:app/src/rust/api/source.dart';
import 'package:app/store/library_store.dart';
import 'package:app/store/models.dart';

/// 夸克网盘会话缓存（按书源 id），避免每次打开都重新连接。
final Map<String, BigInt> _quarkSessions = {};

/// 获取/重连夸克网盘书源会话；连接成功后把会话内续期后的 cookie 回写 DB。
Future<BigInt> quarkSessionFor(BookSource source) async {
  final cached = _quarkSessions[source.id];
  if (cached != null) return cached;
  final s = await quarkConnect(
    cookie: source.cookie ?? '',
    rootId: source.rootId ?? '0',
  );
  _quarkSessions[source.id] = s.id;
  if (s.cookie.isNotEmpty && s.cookie != source.cookie) {
    source.cookie = s.cookie;
    await LibraryStore.instance.updateSource(source.id, cookie: s.cookie);
  }
  return s.id;
}

/// 书源被编辑（Cookie 变更）/删除后使会话失效（下次自动重连）。
void clearQuarkSession(String sourceId) {
  _quarkSessions.remove(sourceId);
}
