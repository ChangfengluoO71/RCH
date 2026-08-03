import 'package:app/ui/home_page.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  Future<void> pumpDialog(WidgetTester tester) async {
    await tester.pumpWidget(const MaterialApp(home: Scaffold(body: Center(child: AddSourceDialog()))));
    await tester.pumpAndSettle();
  }

  testWidgets('添加书源对话框：四种类型可切换且字段随类型变化', (tester) async {
    await pumpDialog(tester);

    // 四个类型段都存在
    expect(find.text('本地目录'), findsOneWidget);
    expect(find.text('WebDAV'), findsOneWidget);
    expect(find.text('SMB'), findsOneWidget);
    expect(find.text('SFTP'), findsOneWidget);

    // 默认 WebDAV：显示服务器地址/用户名/密码/初始路径
    expect(find.text('服务器地址'), findsOneWidget);
    expect(find.text('用户名'), findsOneWidget);
    expect(find.text('密码'), findsOneWidget);

    // 切到本地目录：只显示目录路径
    await tester.tap(find.text('本地目录'));
    await tester.pumpAndSettle();
    expect(find.text('目录路径'), findsOneWidget);
    expect(find.text('服务器地址'), findsNothing);

    // 切到 SMB：显示 UNC 共享路径
    await tester.tap(find.text('SMB'));
    await tester.pumpAndSettle();
    expect(find.text('共享目录路径(UNC)'), findsOneWidget);

    // 切到 SFTP：显示服务器地址/端口/用户名/密码/初始路径
    await tester.tap(find.text('SFTP'));
    await tester.pumpAndSettle();
    expect(find.text('服务器地址'), findsOneWidget);
    expect(find.text('端口(默认22)'), findsOneWidget);
    expect(find.text('用户名'), findsOneWidget);
    expect(find.text('密码'), findsOneWidget);
    expect(find.text('初始路径(可选,默认/)'), findsOneWidget);
  });
}
