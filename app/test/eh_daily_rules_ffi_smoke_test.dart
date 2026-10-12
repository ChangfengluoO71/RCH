import 'dart:convert';
import 'dart:io';

import 'package:app/src/rust/api/eh_subscription.dart';
import 'package:app/src/rust/frb_generated.dart';
import 'package:app/store/eh_daily_coordinator.dart';
import 'package:flutter_rust_bridge/flutter_rust_bridge_for_generated.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  test(
    'current Rust FFI preserves Daily state in the existing rules file',
    () async {
      final dll = File('rust/target/debug/rust_lib_app.dll').absolute;
      expect(
        dll.existsSync(),
        isTrue,
        reason: 'Build the current workspace DLL first.',
      );
      final root = await Directory.systemTemp.createTemp(
        'rch-daily-rules-ffi-',
      );
      final rulesPath =
          '${root.path}${Platform.pathSeparator}eh_subscription_rules.json';

      try {
        await RustLib.init(externalLibrary: ExternalLibrary.open(dll.path));
        final defaults =
            jsonDecode(await ehLoadRules(path: rulesPath))
                as Map<String, dynamic>;
        expect(defaults['automation']['daily_enabled'], isFalse);

        await ehSetDailyEnabled(path: rulesPath, enabled: true);
        final today = ehLocalDateKey(DateTime.now());
        final firstClaim =
            jsonDecode(await ehClaimDaily(path: rulesPath, date: today))
                as Map<String, dynamic>;
        expect(firstClaim['claimed'], isTrue);
        final duplicateClaim =
            jsonDecode(await ehClaimDaily(path: rulesPath, date: today))
                as Map<String, dynamic>;
        expect(duplicateClaim['claimed'], isFalse);

        final afterRuleEdit =
            jsonDecode(firstClaim['rules_json'] as String)
                as Map<String, dynamic>;
        afterRuleEdit['search'] = 'updated query';
        final afterSave = await ehSaveRules(
          path: rulesPath,
          baseJson: jsonEncode(defaults),
          rulesJson: jsonEncode(afterRuleEdit),
        );
        final restored = jsonDecode(afterSave) as Map<String, dynamic>;

        expect(restored['search'], 'updated query');
        expect(restored['automation']['daily_enabled'], isTrue);
        expect(restored['automation']['last_auto_attempt_date'], today);
        expect(restored['automation']['last_run']['status'], 'claimed');

        final interrupted =
            jsonDecode(await ehRecoverDailyRun(path: rulesPath))
                as Map<String, dynamic>;
        expect(
          interrupted['automation']['last_run']['status'],
          'interrupted_unknown',
        );
        expect(
          (jsonDecode(await ehClaimDaily(path: rulesPath, date: today))
              as Map<String, dynamic>)['claimed'],
          isFalse,
        );
        stdout.writeln('H1_DAILY_RULES_FFI=PASS');
        stdout.writeln('H1_DAILY_RULES_PATH=${dll.path}');
      } finally {
        if (RustLib.instance.initialized) RustLib.dispose();
        await root.delete(recursive: true);
      }
    },
  );
}
