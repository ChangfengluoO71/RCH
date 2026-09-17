import 'package:app/store/storage_layout.dart';
import 'package:app/store/models.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('Android durable data root is below application support, not cache', () {
    expect(
      StorageLayout.androidDataRoot(r'C:\app\support'),
      r'C:\app\support\RCH\data',
    );
    expect(
      StorageLayout.androidDataRoot('/data/user/0/rch/files'),
      '/data/user/0/rch/files/RCH/data',
    );
  });

  test('legacy database candidates have deterministic precedence', () {
    final candidates = StorageLayout.legacyDatabaseCandidates(
      dataRoot: '/files/RCH/data',
      markerRoot: '/custom/cache',
      defaultCacheRoot: '/cache/RCH',
      supportRoot: '/files/support',
    );
    expect(candidates, [
      '/files/RCH/data/database.db',
      '/custom/cache/database.db',
      '/cache/RCH/database.db',
      '/files/support/database.db',
      '/files/support/RCH/database.db',
    ]);
  });

  test('redacted source JSON keeps a vault reference but no secret values', () {
    final source = BookSource(
      id: 's1',
      type: 'webdav',
      name: 'NAS',
      url: 'https://example.invalid/dav',
      password: 'secret',
      cookie: 'cookie',
      clientId: 'public-app-id',
      credentialRef: 'vault:s1',
    );
    final json = source.toJson(includeSensitive: false);
    expect(json['credentialRef'], 'vault:s1');
    expect(json.containsKey('password'), isFalse);
    expect(json.containsKey('cookie'), isFalse);
    expect(json['clientId'], 'public-app-id');
  });
}
