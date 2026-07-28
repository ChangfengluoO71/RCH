import 'package:flutter_test/flutter_test.dart';
import 'package:app/main.dart';
import 'package:app/src/rust/frb_generated.dart';
import 'package:integration_test/integration_test.dart';

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  setUpAll(() async => await RustLib.init());
  testWidgets('App 启动显示书架', (WidgetTester tester) async {
    await tester.pumpWidget(const RchApp());
    await tester.pump();
    expect(find.text('RCH 书架'), findsOneWidget);
  });
}
