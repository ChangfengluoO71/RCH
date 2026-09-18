import 'package:app/src/rust/api/source.dart';
import 'package:app/store/models.dart';
import 'package:app/store/remote_scan_coordinator.dart';

/// SFTP 会话缓存(按书源 id)，避免每次打开都重新连接。
final Map<String, BigInt> _sftpSessions = {};

/// 获取/重连某 SFTP 书源的会话（带缓存，避免每次打开都重连）。
Future<BigInt> sftpSessionFor(BookSource source) async {
  final cached = _sftpSessions[source.id];
  if (cached != null) {
    return cached;
  }
  final (host, port) = _parseHostPort(source);
  final s = await sftpConnect(
    host: host,
    port: port,
    username: source.username ?? '',
    password: source.password ?? '',
  );
  _sftpSessions[source.id] = s.id;
  remoteSessionSuccessHub.emit(source, s.id);
  return s.id;
}

/// P1-D-2：把 host/port 解析暴露给其它调用方（**薄包装**）。
///
/// 内部只调用既有的 `_parseHostPort`：不复制解析逻辑、不新增 normalization、
/// 不改变其行为。目的是让 production writer 与新的 sessionless local-only reader
/// 共享**同一个** Dart parser：
/// same Dart parser → Rust `endpoint_for` → same cache identity。
(String, int) sftpHostPortOf(BookSource source) => _parseHostPort(source);

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
