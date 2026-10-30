import 'package:app/src/rust/api/db.dart';
import 'package:app/store/credential_vault.dart';
import 'package:app/store/sync_manager.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  test(
    'sync legacy password is migrated and redacted after vault verification',
    () async {
      final settings = <String, String>{
        'sync_webdav_password': 'legacy-password',
      };
      final vault = MemoryCredentialVault();
      final manager = _manager(settings, vault);
      addTearDown(manager.dispose);

      await manager.load();

      expect(vault.values['sync:webdav-password'], 'legacy-password');
      expect(settings['sync_webdav_password'], isEmpty);
      expect(settings['sync_webdav_password_ref'], 'sync:webdav-password');
    },
  );

  test(
    'sync password is not written to settings when secure storage fails',
    () async {
      final settings = <String, String>{
        'sync_webdav_password': 'legacy-password',
      };
      final manager = _manager(settings, _FailingCredentialVault());
      addTearDown(manager.dispose);

      await manager.load();
      await expectLater(
        manager.setWebdavConfig(
          url: 'https://example.test',
          username: 'reader',
          password: 'replacement-secret',
          dir: 'RCH/sync',
        ),
        throwsStateError,
      );

      expect(settings['sync_webdav_password'], 'legacy-password');
    },
  );
}

SyncManager _manager(Map<String, String> values, CredentialVault vault) =>
    SyncManager.forTesting(
      credentialVault: vault,
      loadSettings: () async => [
        for (final entry in values.entries)
          SettingEntryDto(key: entry.key, value: entry.value),
      ],
      saveSetting: ({required key, required value}) async {
        values[key] = value;
      },
      listDevices: () async => const [],
    );

class _FailingCredentialVault implements CredentialVault {
  @override
  Future<void> delete(String key) async {
    throw StateError('secure storage unavailable');
  }

  @override
  Future<String?> get(String key) async {
    throw StateError('secure storage unavailable');
  }

  @override
  Future<void> put(String key, String value) async {
    throw StateError('secure storage unavailable');
  }
}
