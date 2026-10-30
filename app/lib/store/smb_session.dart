import 'dart:async';

import 'package:app/src/rust/api/source.dart';
import 'package:app/store/models.dart';
import 'package:app/store/remote_scan_coordinator.dart';
import 'package:app/store/smb_source_config.dart';

/// Established SMB sessions are cached by source id for browser, reader,
/// catalog scans, and remote cover work.
final Map<String, BigInt> _smbSessions = <String, BigInt>{};

void clearSmbSession(String sourceId) {
  final id = _smbSessions.remove(sourceId);
  if (id != null) unawaited(smbDisconnect(id: id).catchError((Object _) {}));
}

Future<BigInt> smbSessionFor(BookSource source) async {
  final cached = _smbSessions[source.id];
  if (cached != null) return cached;

  final config = SmbSourceConfig.fromSource(source);
  final session = await smbConnect(
    host: config.server,
    port: config.port,
    share: config.share,
    rootPath: config.rootPath,
    username: source.username ?? '',
    password: source.password ?? '',
  );
  _smbSessions[source.id] = session.id;
  remoteSessionSuccessHub.emit(source, session.id);
  return session.id;
}

/// Validate the share and configured root before saving a new source. The
/// temporary session is always closed and is not published to background scan
/// coordination.
Future<void> verifySmbSource(BookSource source) async {
  final config = SmbSourceConfig.fromSource(source);
  final session = await smbConnect(
    host: config.server,
    port: config.port,
    share: config.share,
    rootPath: config.rootPath,
    username: source.username ?? '',
    password: source.password ?? '',
  );
  try {
    await smbList(session: session.id, path: config.canonicalPath);
  } finally {
    await smbDisconnect(id: session.id);
  }
}
