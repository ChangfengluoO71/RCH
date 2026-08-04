import 'package:app/ui/home_page.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  Future<void> pumpDialog(WidgetTester tester) async {
    await tester.pumpWidget(const MaterialApp(home: Scaffold(body: Center(child: AddSourceDialog()))));
    await tester.pumpAndSettle();
  }

  testWidgets('添加书源对话框：七种类型可切换且字段随类型变化', (tester) async {
    await pumpDialog(tester);

    Future<void> selectType(String label) async {
      await tester.tap(find.byType(DropdownMenu<String>));
      await tester.pumpAndSettle();
      await tester.tap(find.text(label).last);
      await tester.pumpAndSettle();
    }

    // 默认 WebDAV：显示服务器地址/用户名/密码/初始路径
    expect(find.text('服务器地址'), findsOneWidget);
    expect(find.text('用户名'), findsOneWidget);
    expect(find.text('密码'), findsOneWidget);

    // 切到本地目录：只显示目录路径
    await selectType('本地目录');
    expect(find.text('目录路径'), findsOneWidget);
    expect(find.text('服务器地址'), findsNothing);

    // 切到 SMB：显示 UNC 共享路径
    await selectType('SMB');
    expect(find.text('共享目录路径(UNC)'), findsOneWidget);

    // 切到 SFTP：显示服务器地址/端口/用户名/密码/初始路径
    await selectType('SFTP');
    expect(find.text('服务器地址'), findsOneWidget);
    expect(find.text('端口(默认22)'), findsOneWidget);
    expect(find.text('用户名'), findsOneWidget);
    expect(find.text('密码'), findsOneWidget);
    expect(find.text('初始路径(可选,默认/)'), findsOneWidget);

    // 切到百度网盘：根目录 + 授权按钮 + refresh_token
    await selectType('百度网盘');
    expect(find.text('根目录(默认/)'), findsOneWidget);
    expect(find.text('授权登录'), findsOneWidget);
    expect(find.text('refresh_token(授权后自动填入，也可直接粘贴)'), findsOneWidget);

    // 切到 115 网盘：根文件夹 ID + 扫码授权
    await selectType('115 网盘');
    expect(find.text('根文件夹 ID(默认 0)'), findsOneWidget);
    expect(find.text('扫码授权'), findsOneWidget);

    // 切到夸克网盘：根文件夹 ID + Cookie 字段
    await selectType('夸克网盘');
    expect(find.text('根文件夹 ID(默认 0)'), findsOneWidget);
    expect(find.text('Cookie(pan.quark.cn 登录后 F12 复制)'), findsOneWidget);
  });
}
