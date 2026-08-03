import 'package:app/src/rust/api/source.dart';
import 'package:app/store/models.dart';

/// SFTP 会话缓存(按书源 id)，避免每次打开都重新连接。
final Map<String, BigInt> _sftpSessions = {};

/// 获取/重连某 SFTP 书源的会话（带缓存，避免每次打开都重连）。
Future<BigInt> sftpSessionFor(BookSource source) async {
  final cached = _sftpSessions[source.id];
  if (cached != null) return cached;
  final (host, port) = _parseHostPort(source);
  final s = await sftpConnect(
    host: host,
    port: port,
    username: source.username ?? '',
    password: source.password ?? '',
  );
  _sftpSessions[source.id] = s.id;
  return s.id;
}

/// 解析服务器地址：`host` / `host:port`，端口缺省取 source.port 或 22。
(String, int) _parseHostPort(BookSource source) {
  final addr = (source.url ?? '').trim();
  if (addr.contains(':')) {
    final idx = addr.lastIndexOf(':');
    final port = int.tryParse(addr.substring(idx + 1));
    if (port != null && port > 0) {
      return (addr.substring(0, idx), port);
    }
  }
  return (addr, source.port ?? 22);
}
