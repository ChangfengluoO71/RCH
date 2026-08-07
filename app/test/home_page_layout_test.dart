// 回归测试：HomePage 在宽屏（桌面侧栏）与窄屏（手机底部导航）下都不得出现
// "RenderFlex ... unbounded" 布局异常（曾因侧栏非 flex 子节点内嵌 Expanded 导致黑屏）。
import 'package:app/ui/home_page.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  testWidgets('HomePage 宽屏与窄屏布局无异常', (tester) async {
    tester.view.physicalSize = const Size(1200, 800);
    tester.view.devicePixelRatio = 1.0;
    await tester.pumpWidget(const MaterialApp(home: HomePage()));
    await tester.pump();
    await tester.pump(const Duration(seconds: 1));
    expect(tester.takeException(), isNull, reason: '宽屏（桌面侧栏）布局不应抛 RenderFlex 异常');

    tester.view.physicalSize = const Size(400, 800);
    await tester.pumpWidget(const MaterialApp(home: HomePage()));
    await tester.pump();
    await tester.pump(const Duration(seconds: 1));
    expect(tester.takeException(), isNull, reason: '窄屏（手机底部导航）布局不应抛 RenderFlex 异常');
    tester.view.reset();
  });
}
