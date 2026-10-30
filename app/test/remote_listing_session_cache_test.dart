import 'package:app/store/models.dart';
import 'package:app/store/remote_listing.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test(
    'editing an SMB source evicts the shared browser session cache',
    () async {
      const sourceId = 'remote-cache-smb-edit-test';
      final source = BookSource(
        id: sourceId,
        type: 'smb',
        name: 'Test share',
        url: 'nas.example',
        path: 'comics',
      );
      cacheRemoteSession(sourceId, BigInt.from(987654));

      expect(await cachedRemoteSession(sourceId), BigInt.from(987654));

      clearRemoteSessionCache(source);

      expect(await cachedRemoteSession(sourceId), isNull);
    },
  );
}
