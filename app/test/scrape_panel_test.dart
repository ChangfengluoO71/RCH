import 'package:app/ui/scrape_panel.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  testWidgets('settings scrape panel exposes only the rerun action', (
    tester,
  ) async {
    await tester.pumpWidget(
      const MaterialApp(home: Scaffold(body: ScrapePanel())),
    );
    await tester.pump();

    expect(find.text('重新刮削'), findsOneWidget);
    expect(find.text('仅生成刮削 proposal'), findsNothing);
    expect(find.byType(DropdownButton<String>), findsNothing);
    expect(find.byType(FilledButton), findsOneWidget);
  });
}
