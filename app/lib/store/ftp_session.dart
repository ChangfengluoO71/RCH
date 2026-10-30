import 'dart:async';

import 'package:app/src/rust/api/source.dart';
import 'package:app/store/models.dart';
import 'package:app/store/remote_scan_coordinator.dart';

/// FTP session cache keyed by book source id.
final Map<String, BigInt> _ftpSessions = {};

/// Discard a cached FTP session and release its network client.
void clearFtpSession(String sourceId) {
  final id = _ftpSessions.remove(sourceId);
  if (id != null) unawaited(ftpDisconnect(id: id).catchError((_) {}));
}

Future<BigInt> ftpSessionFor(BookSource source) async {
  final cached = _ftpSessions[source.id];
  if (cached != null) return cached;

  final host = (source.url ?? '').trim().isEmpty ? '127.0.0.1' : (source.url ?? '').trim();
  final port = source.port ?? 21;
  final session = await ftpConnect(
    host: host,
    port: port,
    username: source.username ?? '',
    password: source.password ?? '',
    rootPath: source.effectiveRootPath,
  );
  _ftpSessions[source.id] = session.id;
  remoteSessionSuccessHub.emit(source, session.id);
  return session.id;
}

String ftpHostOf(BookSource source) =>
    (source.url ?? '').trim().isEmpty ? '127.0.0.1' : (source.url ?? '').trim();

int ftpPortOf(BookSource source) => source.port ?? 21;
