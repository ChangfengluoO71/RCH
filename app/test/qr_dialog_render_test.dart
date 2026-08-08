// 回归护栏：扫码对话框必须能正常渲染。
// 背景：QrImageView 内部使用 LayoutBuilder，与 AlertDialog 的 IntrinsicWidth
// 冲突，会在 performLayout 抛 "LayoutBuilder does not support returning
// intrinsic dimensions"，导致对话框只剩遮罩、内容渲染不出来。
// 因此扫码对话框统一改用 QrPainter + CustomPaint，本测试防止回退。
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:qr_flutter/qr_flutter.dart';

void main() {
  testWidgets('AlertDialog with QrPainter custom paint renders', (tester) async {
    await tester.pumpWidget(MaterialApp(
      home: Scaffold(body: Center(child: const _QrDialog())),
    ));
    await tester.pumpAndSettle();
    expect(tester.takeException(), isNull);
    expect(find.byType(AlertDialog), findsOneWidget);
    expect(find.byType(CustomPaint), findsWidgets);
  });
}

class _QrDialog extends StatelessWidget {
  const _QrDialog();
  @override
  Widget build(BuildContext context) {
    return AlertDialog(
      title: const Text('115 扫码获取 Cookie'),
      content: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          const Text('用 115 手机 App 扫码', style: TextStyle(fontSize: 12)),
          const SizedBox(height: 10),
          SizedBox(
            width: 220,
            height: 220,
            child: CustomPaint(
              painter: QrPainter(
                data: 'https://115.com/scan/dg-test-1234567890abcdef',
                version: QrVersions.auto,
              ),
            ),
          ),
          const SizedBox(height: 10),
          const Text('请用 115 APP 扫码', style: TextStyle(fontSize: 12)),
        ],
      ),
      actions: [TextButton(onPressed: null, child: const Text('关闭'))],
    );
  }
}
