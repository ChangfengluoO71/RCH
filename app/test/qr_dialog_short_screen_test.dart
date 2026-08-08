// 回归护栏：115 扫码对话框在 MuMu 矮屏（逻辑 853x480，物理 1280x720@1.5）
// 下不得出现 RenderFlex 溢出黄条（用户反馈二维码上方曾被黄黑条遮挡）。
import 'package:app/ui/cloud115_qr_scan.dart';
import 'package:flutter/material.dart';
import 'package:flutter_rust_bridge/flutter_rust_bridge_for_generated.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  testWidgets('115 扫码对话框在 MuMu 矮屏(853x480)不出现溢出', (tester) async {
    tester.view.physicalSize = const Size(1280, 720);
    tester.view.devicePixelRatio = 1.5;
    await tester.pumpWidget(MaterialApp(
      home: Scaffold(
        body: Center(
          child: Cloud115CookieQrScanDialog(
            uid: 'u1',
            time: PlatformInt64Util.from(0),
            sign: 's1',
            qrcode: 'https://115.com/scan/dg-test-1234567890abcdef',
            app: 'wechatmini',
          ),
        ),
      ),
    ));
    await tester.pump(const Duration(milliseconds: 100));
    expect(tester.takeException(), isNull,
        reason: '115 扫码对话框在矮屏下不应 RenderFlex 溢出');
    // 卸载对话框，取消轮询定时器，避免测试结束时仍有 pending timer。
    await tester.pumpWidget(const SizedBox());
    tester.view.reset();
  });
}
