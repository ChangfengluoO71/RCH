import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('Android Keystore credential vault round-trip', (tester) async {
    if (defaultTargetPlatform != TargetPlatform.android) return;

    const channel = MethodChannel('rch/credentials');
    const key = 'test:credential-vault-runtime';
    const sentinel = 'rch-android-vault-sentinel';
    await channel.invokeMethod<void>('put', <String, Object?>{
      'key': key,
      'value': sentinel,
    });
    expect(
      await channel.invokeMethod<String>('get', <String, Object?>{'key': key}),
      sentinel,
    );
    await channel.invokeMethod<void>('delete', <String, Object?>{'key': key});
    expect(
      await channel.invokeMethod<String>('get', <String, Object?>{'key': key}),
      isNull,
    );
    debugPrint('ANDROID_KEYSTORE_REGRESSION=PASS;sentinel_only=true');
  });
}
