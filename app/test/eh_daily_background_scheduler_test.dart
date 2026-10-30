import 'package:app/store/eh_daily_background.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  const channel = MethodChannel('rch/eh_daily_background_test');
  final calls = <MethodCall>[];
  setUp(() {
    calls.clear();
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(channel, (call) async {
          calls.add(call);
          if (call.method == 'status') {
            return {
              'scheduled': true,
              'running': false,
              'networkConnected': false,
              'lastStatus': 'waitingForOutputDirectory',
              'lastFinishedAt': 1000,
            };
          }
          return null;
        });
  });
  tearDown(() {
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(channel, null);
  });

  test('the opt-in state is sent to the native persistent scheduler', () async {
    const scheduler = AndroidEhDailyBackgroundScheduler(channel: channel);
    await scheduler.sync(true);
    await scheduler.sync(false);
    expect(calls.map((call) => call.method), ['setEnabled', 'setEnabled']);
    expect(calls.map((call) => call.arguments), [
      {'enabled': true},
      {'enabled': false},
    ]);
  });

  test('status preserves waiting-network and last-run information', () async {
    const scheduler = AndroidEhDailyBackgroundScheduler(channel: channel);
    final status = await scheduler.status();
    expect(status.scheduled, isTrue);
    expect(status.running, isFalse);
    expect(status.networkConnected, isFalse);
    expect(status.lastStatus, 'waitingForOutputDirectory');
    expect(status.lastFinishedAt?.millisecondsSinceEpoch, 1000);
  });
}
