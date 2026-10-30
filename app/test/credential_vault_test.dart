import 'package:app/store/credential_vault.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  test('platform vault uses the rch/credentials method contract', () async {
    final calls = <String>[];
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(const MethodChannel('rch/credentials'), (
          call,
        ) async {
          calls.add(call.method);
          if (call.method == 'get') return 'round-trip';
          return null;
        });
    final vault = MethodChannelCredentialVault();
    await vault.put('source:s1', 'secret');
    expect(await vault.get('source:s1'), 'round-trip');
    await vault.delete('source:s1');
    expect(calls, ['put', 'get', 'delete']);
  });

  test('native vault uses the platform secure store outside Android', () {
    expect(
      platformCredentialVault(platform: TargetPlatform.android),
      isA<MethodChannelCredentialVault>(),
    );
    expect(
      platformCredentialVault(platform: TargetPlatform.windows),
      isA<FlutterSecureStorageCredentialVault>(),
    );
    expect(usesNativeCredentialVault(isWeb: false), isTrue);
    expect(usesNativeCredentialVault(isWeb: true), isFalse);
    expect(includeSensitiveCredentialsInBackup(isWeb: false), isFalse);
    expect(includeSensitiveCredentialsInBackup(isWeb: true), isTrue);
  });

  test(
    'secure storage credential adapter supports read, write and delete',
    () async {
      final storage = _MemoryCredentialKeyValueStore();
      final vault = FlutterSecureStorageCredentialVault(storage: storage);

      await vault.put('source:test', 'secret');
      expect(await vault.get('source:test'), 'secret');
      await vault.delete('source:test');
      expect(await vault.get('source:test'), isNull);
    },
  );

  test('strict bundle decoding rejects corrupt vault payloads', () {
    expect(
      () => decodeCredentialBundleStrict('{not-json'),
      throwsFormatException,
    );
    expect(decodeCredentialBundle('{not-json'), isEmpty);
  });
}

class _MemoryCredentialKeyValueStore implements CredentialKeyValueStore {
  final values = <String, String>{};

  @override
  Future<void> write({required String key, required String value}) async {
    values[key] = value;
  }

  @override
  Future<String?> read({required String key}) async => values[key];

  @override
  Future<void> delete({required String key}) async {
    values.remove(key);
  }
}
