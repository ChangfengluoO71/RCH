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

  test('strict bundle decoding rejects corrupt vault payloads', () {
    expect(
      () => decodeCredentialBundleStrict('{not-json'),
      throwsFormatException,
    );
    expect(decodeCredentialBundle('{not-json'), isEmpty);
  });
}
