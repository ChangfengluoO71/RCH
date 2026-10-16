import 'package:app/store/eh_subscription_store.dart';
import 'package:app/ui/eh_subscription_panel.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  testWidgets('EH proxy controls live in the separate Network panel', (
    tester,
  ) async {
    final store = EhSubscriptionStore.forTest(
      rules: {
        'search': r'language:chinese$',
        'network': {'mode': 'system', 'proxy_url': ''},
      },
    );
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: SingleChildScrollView(
            child: EhNetworkSettingsPanel(storeOverride: store),
          ),
        ),
      ),
    );

    expect(find.byKey(const ValueKey('eh-proxy-mode')), findsOneWidget);

    await tester.tap(find.byKey(const ValueKey('eh-proxy-mode')));
    await tester.pumpAndSettle();
    await tester.tap(find.text('manual（手动代理）').last);
    await tester.pumpAndSettle();

    expect(find.text('代理地址'), findsOneWidget);
    expect(find.textContaining('socks5h://'), findsOneWidget);
  });

  testWidgets('EH subscription panel keeps network controls out of Advanced', (
    tester,
  ) async {
    final store = EhSubscriptionStore.forTest(
      rules: {
        'search': r'language:chinese$',
        'network': {'mode': 'system', 'proxy_url': ''},
      },
    );
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: SingleChildScrollView(
            child: EhSubscriptionPanel(storeOverride: store),
          ),
        ),
      ),
    );

    expect(find.byKey(const ValueKey('eh-proxy-mode')), findsNothing);
    expect(find.byKey(const ValueKey('eh-advanced-options')), findsOneWidget);
  });
}
