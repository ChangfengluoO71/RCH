import 'dart:async';

import 'package:flutter/foundation.dart';

import 'package:app/src/rust/api/source.dart';
import 'package:app/store/models.dart';
import 'package:app/store/remote_scan_coordinator.dart';

/// A host-key mismatch is surfaced before password or private-key auth runs.
class SftpHostKeyTrustRequired implements Exception {
  const SftpHostKeyTrustRequired({
    required this.host,
    required this.port,
    required this.expectedFingerprint,
    required this.actualFingerprint,
    required this.expectedEndpoint,
    required this.actualEndpoint,
  });

  final String host;
  final int port;
  final String expectedFingerprint;
  final String actualFingerprint;
  final String expectedEndpoint;
  final String actualEndpoint;

  bool get isChanged =>
      expectedFingerprint.trim().isNotEmpty ||
      expectedEndpoint.trim().isNotEmpty;

  @override
  String toString() => isChanged
      ? 'SFTP host trust changed for $host:$port'
      : 'SFTP server host key needs explicit trust for $host:$port';
}

/// Require a stored pin to exactly match the key observed in the unauthenticated
/// probe. This helper keeps the trust boundary directly testable without FFI.
@visibleForTesting
String requireTrustedSftpHostKey({
  required String host,
  required int port,
  required String expectedFingerprint,
  required String actualFingerprint,
  required String expectedEndpoint,
  required String actualEndpoint,
}) {
  final expected = expectedFingerprint.trim();
  final actual = actualFingerprint.trim();
  final pinnedEndpoint = expectedEndpoint.trim();
  final observedEndpoint = actualEndpoint.trim();
  if (expected.isEmpty ||
      actual.isEmpty ||
      expected != actual ||
      pinnedEndpoint.isEmpty ||
      pinnedEndpoint != observedEndpoint) {
    throw SftpHostKeyTrustRequired(
      host: host,
      port: port,
      expectedFingerprint: expected,
      actualFingerprint: actual,
      expectedEndpoint: pinnedEndpoint,
      actualEndpoint: observedEndpoint,
    );
  }
  return actual;
}

String sftpEndpointKey(String host, int port) {
  final normalizedHost = host.trim().toLowerCase();
  final authority = normalizedHost.contains(':')
      ? '[$normalizedHost]'
      : normalizedHost;
  return '$authority:$port';
}

/// SFTP session cache keyed by book source id.
final Map<String, BigInt> _sftpSessions = {};

/// Discard a cached SFTP session and release its network client.
void clearSftpSession(String sourceId) {
  final id = _sftpSessions.remove(sourceId);
  if (id != null) unawaited(sftpDisconnect(id: id).catchError((_) {}));
}

Future<BigInt> sftpSessionFor(BookSource source) async {
  final cached = _sftpSessions[source.id];
  if (cached != null) return cached;

  final (host, port) = _parseHostPort(source);
  final actualEndpoint = sftpEndpointKey(host, port);
  // This first SSH handshake only observes the server key. It carries no
  // username, password, private key, or passphrase.
  final actualFingerprint = await sftpProbeHostKey(host: host, port: port);
  final trustedFingerprint = requireTrustedSftpHostKey(
    host: host,
    port: port,
    expectedFingerprint: source.hostKeyFingerprint ?? '',
    actualFingerprint: actualFingerprint,
    expectedEndpoint: source.hostKeyEndpoint ?? '',
    actualEndpoint: actualEndpoint,
  );
  final authMethod = source.effectiveSftpAuthMethod;
  if (authMethod != 'password' && authMethod != 'privateKey') {
    throw FormatException(
      'Unsupported SFTP authentication method: $authMethod',
    );
  }
  final session = await sftpConnect(
    host: host,
    port: port,
    username: source.username ?? '',
    authMethod: authMethod,
    password: authMethod == 'password' ? source.password ?? '' : '',
    privateKey: authMethod == 'privateKey' ? source.privateKey ?? '' : '',
    keyPassphrase: authMethod == 'privateKey'
        ? source.privateKeyPassphrase ?? ''
        : '',
    expectedFingerprint: trustedFingerprint,
  );
  // The key could change between probe and authenticated handshake; the Rust
  // handler pins it again and the returned value is checked defensively.
  try {
    requireTrustedSftpHostKey(
      host: host,
      port: port,
      expectedFingerprint: trustedFingerprint,
      actualFingerprint: session.hostFingerprint,
      expectedEndpoint: actualEndpoint,
      actualEndpoint: actualEndpoint,
    );
  } catch (_) {
    await sftpDisconnect(id: session.id);
    rethrow;
  }
  _sftpSessions[source.id] = session.id;
  remoteSessionSuccessHub.emit(source, session.id);
  return session.id;
}

String sftpHostOf(BookSource source) => _parseHostPort(source).$1;

(String, int) sftpHostPortOf(BookSource source) => _parseHostPort(source);

(String, int) _parseHostPort(BookSource source) {
  final addr = (source.url ?? '').trim();
  if (addr.startsWith('[')) {
    final close = addr.indexOf(']');
    if (close > 0) {
      final host = addr.substring(1, close);
      final tail = addr.substring(close + 1);
      final embeddedPort = tail.startsWith(':')
          ? int.tryParse(tail.substring(1))
          : null;
      if (embeddedPort != null && embeddedPort > 0 && embeddedPort <= 65535) {
        return (host, embeddedPort);
      }
      return (host, source.port ?? 22);
    }
  }
  if (addr.contains(':')) {
    final idx = addr.lastIndexOf(':');
    final port = int.tryParse(addr.substring(idx + 1));
    if (port != null && port > 0 && port <= 65535) {
      return (addr.substring(0, idx), port);
    }
  }
  return (addr, source.port ?? 22);
}
