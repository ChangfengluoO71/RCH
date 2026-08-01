import 'package:app/src/rust/api/source.dart';
import 'package:app/store/library_store.dart';
import 'package:app/store/models.dart';

/// WebDAV 会话缓存(按书源 id)，避免每次打开都重新连接。
final Map<String, BigInt> _webdavSessions = {};

/// 获取/重连某 WebDAV 书源的会话（带缓存，避免每次打开都重连）。
Future<BigInt> webdavSessionFor(BookSource source) async {
  final cached = _webdavSessions[source.id];
  if (cached != null) return cached;
  final s = await webdavConnect(
    url: source.url ?? '',
    username: source.username ?? '',
    password: source.password ?? '',
  );
  _webdavSessions[source.id] = s.id;
  // 保存探测到的能力标记
  if (s.capabilityLabel.isNotEmpty && source.capabilityLabel.isEmpty) {
    source.capabilityLabel = s.capabilityLabel;
    LibraryStore.instance.updateSourceCapability(source.id, s.capabilityLabel);
  }
  return s.id;
}
