import 'dart:io';
import 'dart:convert';

import 'package:app/store/eh_daily_coordinator.dart';
import 'package:app/store/eh_subscription_store.dart';
import 'package:app/src/rust/api/eh_subscription.dart' as eh_api;
import 'package:app/src/rust/frb_generated.dart';
import 'package:app/ui/eh_subscription_panel.dart';
import 'package:flutter_rust_bridge/flutter_rust_bridge_for_generated.dart';
import 'package:flutter/material.dart';

/// Isolated Windows GUI entry point for H1 Daily acceptance.
/// It does not call production main() or load library, source, or cache data.
/// The only persistent state is an explicitly supplied test rules file; the
/// collection runner is inert and no network client is created.
Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  final rulesPath = Platform.environment['RCH_H1_TEST_RULES_PATH'];
  if (rulesPath == null || rulesPath.isEmpty) {
    throw StateError(
      'RCH_H1_TEST_RULES_PATH must point to an isolated test file.',
    );
  }
  final dllPath = Platform.environment['RCH_H1_TEST_RUST_DLL'];
  if (dllPath == null || dllPath.isEmpty) {
    throw StateError(
      'RCH_H1_TEST_RUST_DLL must point to the current Debug DLL.',
    );
  }
  await RustLib.init(externalLibrary: ExternalLibrary.open(dllPath));

  // Explicit test-only process modes exercise the real Windows executable and
  // generated FFI while staying outside the production main() startup path.
  // They only touch the isolated rules file and never call ehCollect/network.
  final processMode = Platform.environment['RCH_H1_TEST_PROCESS_MODE'];
  if (processMode != null && processMode.isNotEmpty) {
    await _runProcessMode(processMode, rulesPath);
    return;
  }

  if (!File(
    rulesPath,
  ).absolute.path.toLowerCase().contains('rch-h1-gui-acceptance-')) {
    throw StateError(
      'GUI scenarios require a dedicated RCH-H1-GUI-Acceptance test directory.',
    );
  }
  final scenario = Platform.environment['RCH_H1_TEST_GUI_SCENARIO'] ?? 'normal';
  final rawTextScale = Platform.environment['RCH_H1_TEST_TEXT_SCALE'];
  final textScale = rawTextScale == null ? null : double.tryParse(rawTextScale);
  if (rawTextScale != null &&
      (textScale == null || textScale < 1.0 || textScale > 2.0)) {
    throw StateError('RCH_H1_TEST_TEXT_SCALE must be between 1.0 and 2.0.');
  }

  final existingRulesJson = await eh_api.ehLoadRules(path: rulesPath);
  final rules = Map<String, dynamic>.from(jsonDecode(existingRulesJson) as Map);
  final root = Directory(File(rulesPath).parent.path);
  final output = Directory('${root.path}${Platform.pathSeparator}output');
  await output.create(recursive: true);
  rules.putIfAbsent('search', () => 'H1 GUI fixture');
  rules['out_dir'] = output.path;
  final automation = Map<String, dynamic>.from(
    rules['automation'] as Map? ?? const <String, dynamic>{},
  );
  automation['daily_enabled'] = false;
  rules['automation'] = automation;
  var savedRulesJson = await eh_api.ehSaveRules(
    path: rulesPath,
    baseJson: existingRulesJson,
    rulesJson: jsonEncode(rules),
  );
  if (scenario == 'manual-wait' ||
      scenario == 'read-error' ||
      scenario == 'interrupted') {
    await eh_api.ehSetDailyEnabled(path: rulesPath, enabled: true);
    if (scenario == 'interrupted') {
      final today = ehLocalDateKey(DateTime.now());
      final claim = Map<String, dynamic>.from(
        jsonDecode(await eh_api.ehClaimDaily(path: rulesPath, date: today))
            as Map,
      );
      if (claim['claimed'] != true) {
        throw StateError('Isolated interrupted fixture could not claim today.');
      }
      await eh_api.ehUpdateDailyRun(
        path: rulesPath,
        runJson: jsonEncode({
          'date': today,
          'source': 'daily_auto',
          'status': 'running',
          'saved': 0,
          'error': null,
        }),
      );
    }
    savedRulesJson = await eh_api.ehLoadRules(path: rulesPath);
  }
  final savedRules = Map<String, dynamic>.from(
    jsonDecode(savedRulesJson) as Map,
  );
  stdout.writeln('H1_GUI_PROFILE_ISOLATED=1');
  stdout.writeln('H1_GUI_SCENARIO=$scenario');
  stdout.writeln('H1_GUI_TEXT_SCALE=${textScale ?? 'system'}');
  stdout.writeln('H1_GUI_NETWORK_REQUESTS=0');
  stdout.writeln(
    'H1_GUI_RULES_RESTORED=${savedRules['automation']?['daily_enabled'] == true ? 'enabled' : 'disabled'}',
  );
  final rulesWriter = scenario == 'rules-save-failure'
      ? (String _) async => throw StateError('controlled rules save failure')
      : null;
  final store = EhSubscriptionStore.forTest(
    rules: savedRules,
    rulesPath: rulesPath,
    rulesWriter: rulesWriter,
    manifestReport: const EhManifestReport(
      status: EhManifestReadStatus.missing,
    ),
    collectRunner: (_) async {
      stdout.writeln('H1_GUI_COLLECT_CALLED=1');
      throw StateError(
        'EH collection is disabled in the isolated GUI harness.',
      );
    },
  );
  EhDailyCoordinator.initialize(store);
  final coordinator = EhDailyCoordinator.instance;
  switch (scenario) {
    case 'rules-corrupt':
      await File(rulesPath).writeAsString('{', flush: true);
      try {
        await eh_api.ehLoadRules(path: rulesPath);
      } catch (error) {
        store.loadError = '规则文件损坏：$error';
      }
      break;
    case 'read-error':
      await File(rulesPath).writeAsString('{', flush: true);
      await coordinator.start();
      break;
    case 'manual-wait':
      store.busy = true;
      await coordinator.start();
      break;
    case 'interrupted':
      await coordinator.start();
      break;
    case 'normal':
    case 'rules-save-failure':
    case 'collect-failure':
      break;
    default:
      throw StateError('Unknown GUI scenario: $scenario');
  }
  var previousDailyEnabled = store.dailyAutomation.dailyEnabled;
  store.addListener(() {
    final nowEnabled = store.dailyAutomation.dailyEnabled;
    if (nowEnabled != previousDailyEnabled) {
      previousDailyEnabled = nowEnabled;
      stdout.writeln(
        'H1_GUI_DAILY_SAVED=${nowEnabled ? 'enabled' : 'disabled'}',
      );
    }
  });
  final scrollController = ScrollController();
  runApp(
    MaterialApp(
      title: 'RCH H1 Daily GUI Harness',
      theme: ThemeData(colorSchemeSeed: const Color(0xFF6487A8)),
      builder: (context, child) {
        final content = child ?? const SizedBox.shrink();
        final scale = textScale;
        if (scale == null) return content;
        return MediaQuery(
          data: MediaQuery.of(
            context,
          ).copyWith(textScaler: TextScaler.linear(scale)),
          child: content,
        );
      },
      home: Scaffold(
        appBar: AppBar(
          title: Text('EH Daily Windows 验收 · $scenario'),
          actions: scenario == 'rules-corrupt'
              ? [
                  TextButton(
                    onPressed: () async {
                      await File(
                        rulesPath,
                      ).writeAsString(savedRulesJson, flush: true);
                      store.loadError = null;
                      store.notifyListeners();
                      WidgetsBinding.instance.addPostFrameCallback((_) {
                        if (scrollController.hasClients) {
                          scrollController.jumpTo(
                            scrollController.position.maxScrollExtent,
                          );
                        }
                      });
                      stdout.writeln('H1_GUI_RULES_RESTORED=1');
                    },
                    child: const Text('恢复测试规则'),
                  ),
                ]
              : null,
        ),
        body: SingleChildScrollView(
          controller: scrollController,
          padding: const EdgeInsets.all(16),
          child: EhSubscriptionPanel(storeOverride: store),
        ),
      ),
    ),
  );
  WidgetsBinding.instance.addPostFrameCallback((_) {
    if (scrollController.hasClients) {
      scrollController.jumpTo(scrollController.position.maxScrollExtent);
    }
  });
}

Future<void> _runProcessMode(String mode, String rulesPath) async {
  final date = Platform.environment['RCH_H1_TEST_DATE'] ?? '2026-10-12';
  switch (mode) {
    case 'claim-hold':
      final result = await _claim(date, rulesPath);
      if (result['claimed'] != true) {
        throw StateError('Isolated process could not claim the test date.');
      }
      await _writeReadyMarker('claimed');
      await _holdForParent();
      return;
    case 'running-hold':
      final result = await _claim(date, rulesPath);
      if (result['claimed'] != true) {
        throw StateError('Isolated process could not claim the test date.');
      }
      await eh_api.ehUpdateDailyRun(
        path: rulesPath,
        runJson: jsonEncode({
          'date': date,
          'source': 'daily_auto',
          'status': 'running',
          'saved': 0,
          'error': null,
        }),
      );
      await _writeReadyMarker('running');
      await _holdForParent();
      return;
    case 'claim-race':
      await _writeReadyMarker('ready');
      await _waitForParentGate();
      final result = await _claim(date, rulesPath);
      await _writeResultMarker(jsonEncode({'claimed': result['claimed']}));
      return;
    case 'recover-check':
      final nextDate = Platform.environment['RCH_H1_TEST_NEXT_DATE'];
      if (nextDate == null || nextDate.isEmpty) {
        throw StateError('RCH_H1_TEST_NEXT_DATE is required.');
      }
      final recoveredJson = await eh_api.ehRecoverDailyRun(path: rulesPath);
      final recoveredRules = Map<String, dynamic>.from(
        jsonDecode(recoveredJson) as Map,
      );
      final recoveredRun = Map<String, dynamic>.from(
        (recoveredRules['automation'] as Map)['last_run'] as Map,
      );
      final sameDateClaim = await _claim(date, rulesPath);
      final nextDateClaim = await _claim(nextDate, rulesPath);
      await _writeResultMarker(
        jsonEncode({
          'recovered_status': recoveredRun['status'],
          'same_date_claimed': sameDateClaim['claimed'],
          'next_date_claimed': nextDateClaim['claimed'],
        }),
      );
      return;
    default:
      throw StateError('Unknown isolated process mode.');
  }
}

Future<Map<String, dynamic>> _claim(String date, String rulesPath) async =>
    Map<String, dynamic>.from(
      jsonDecode(await eh_api.ehClaimDaily(path: rulesPath, date: date)) as Map,
    );

Future<void> _writeReadyMarker(String value) async {
  final path = Platform.environment['RCH_H1_TEST_READY_PATH'];
  if (path == null || path.isEmpty) {
    throw StateError('RCH_H1_TEST_READY_PATH is required.');
  }
  await File(path).writeAsString(value, flush: true);
  stdout.writeln('H1_PROCESS_READY=$value');
}

Future<void> _writeResultMarker(String value) async {
  final path = Platform.environment['RCH_H1_TEST_RESULT_PATH'];
  if (path == null || path.isEmpty) {
    throw StateError('RCH_H1_TEST_RESULT_PATH is required.');
  }
  await File(path).writeAsString(value, flush: true);
  stdout.writeln('H1_PROCESS_RESULT_WRITTEN=1');
}

Future<void> _waitForParentGate() async {
  final path = Platform.environment['RCH_H1_TEST_GATE_PATH'];
  if (path == null || path.isEmpty) {
    throw StateError('RCH_H1_TEST_GATE_PATH is required.');
  }
  final deadline = DateTime.now().add(const Duration(seconds: 30));
  while (!File(path).existsSync()) {
    if (DateTime.now().isAfter(deadline)) {
      throw StateError('Timed out waiting for the test process barrier.');
    }
    await Future<void>.delayed(const Duration(milliseconds: 20));
  }
}

Future<void> _holdForParent() async {
  while (true) {
    await Future<void>.delayed(const Duration(seconds: 1));
  }
}
