import 'package:app/ui/common.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  testWidgets(
    'reader exit restores the latest home policy and ignores stale readers',
    (tester) async {
      final calls = <List<dynamic>>[];
      final messenger =
          TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
      messenger.setMockMethodCallHandler(SystemChannels.platform, (call) async {
        if (call.method == 'SystemChrome.setPreferredOrientations') {
          calls.add(call.arguments as List<dynamic>);
        }
        return null;
      });
      try {
        debugDefaultTargetPlatformOverride = TargetPlatform.android;
        final coordinator = AppOrientationCoordinator();
        const tablet = LayoutProfile(
          tablet: true,
          compact: true,
          lockPortrait: false,
        );
        const phone = LayoutProfile(
          tablet: false,
          compact: true,
          lockPortrait: true,
        );
        final older = Object();
        final current = Object();
        coordinator.updateHome(tablet);
        coordinator.enterReader(older);
        coordinator.enterReader(current);
        coordinator.updateHome(phone);
        coordinator.leaveReader(older);
        await tester.pump();
        expect(
          calls.last,
          hasLength(4),
          reason: 'an older reader must not lock a newer reader',
        );
        coordinator.leaveReader(current);
        await tester.pump();
        expect(calls.last, [
          'DeviceOrientation.portraitUp',
        ], reason: 'exit must use the most recent home setting');
      } finally {
        debugDefaultTargetPlatformOverride = null;
        messenger.setMockMethodCallHandler(SystemChannels.platform, null);
      }
    },
  );
}
