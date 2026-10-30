import 'dart:async';

import 'package:app/src/rust/api/source.dart';
import 'package:app/store/models.dart';
import 'package:app/store/remote_scan_coordinator.dart';

/// S3 session cache keyed by book source id.
final Map<String, BigInt> _s3Sessions = {};

/// Discard a cached S3 session and release its network client.
void clearS3Session(String sourceId) {
  final id = _s3Sessions.remove(sourceId);
  if (id != null) unawaited(s3Disconnect(id: id).catchError((_) {}));
}

Future<BigInt> s3SessionFor(BookSource source) async {
  final cached = _s3Sessions[source.id];
  if (cached != null) return cached;

  final endpoint = (source.url ?? '').trim();
  final bucket = (source.rootId ?? '').trim();
  final session = await s3Connect(
    endpoint: endpoint,
    bucket: bucket,
    accessKey: source.username ?? '',
    secretKey: source.password ?? '',
    region: (source.s3Region ?? '').trim().isEmpty
        ? 'us-east-1'
        : source.s3Region!.trim(),
    rootPath: source.effectiveRootPath,
  );
  _s3Sessions[source.id] = session.id;
  remoteSessionSuccessHub.emit(source, session.id);
  return session.id;
}

String s3EndpointOf(BookSource source) => (source.url ?? '').trim();
