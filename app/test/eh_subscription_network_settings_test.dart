import 'package:app/store/eh_subscription_store.dart';
import 'package:app/ui/eh_subscription_panel.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  testWidgets('EH network settings expose proxy mode and manual proxy input', (
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

    expect(find.text('网络与自动化'), findsOneWidget);
    final section = find.byKey(const ValueKey('eh-network-automation'));
    await tester.ensureVisible(section);
    await tester.tap(section);
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('eh-proxy-mode')), findsOneWidget);

    await tester.tap(find.byKey(const ValueKey('eh-proxy-mode')));
    await tester.pumpAndSettle();
    await tester.tap(find.text('manual（手动代理）').last);
    await tester.pumpAndSettle();

    expect(find.text('代理地址'), findsOneWidget);
    expect(find.textContaining('socks5h://'), findsOneWidget);
  });
}
