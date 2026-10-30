import 'package:app/ui/home_page.dart';
import 'package:file_selector_platform_interface/file_selector_platform_interface.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

class _FakeFileSelectorPlatform extends FileSelectorPlatform {
  FileDialogOptions? directoryOptions;

  @override
  Future<String?> getDirectoryPathWithOptions(FileDialogOptions options) async {
    directoryOptions = options;
    return r'F:\comic\测试漫画';
  }
}

void main() {
  Future<void> pumpDialog(WidgetTester tester) async {
    await tester.pumpWidget(
      const MaterialApp(
        home: Scaffold(body: Center(child: AddSourceDialog())),
      ),
    );
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

    // 切到 SMB：显示服务器、共享路径和可选账号密码
    await selectType('SMB');
    expect(find.text('服务器地址'), findsOneWidget);
    expect(find.text('SMB端口(默认445)'), findsOneWidget);
    expect(find.text('共享名/初始目录'), findsOneWidget);
    expect(find.text('用户名（可选）'), findsOneWidget);
    expect(find.text('密码（可选）'), findsOneWidget);

    // SFTP offers explicit password and private-key authentication.
    await selectType('SFTP');
    expect(find.text('服务器地址'), findsOneWidget);
    expect(find.text('端口(默认22)'), findsOneWidget);
    expect(find.text('用户名'), findsOneWidget);
    expect(find.text('Authentication method'), findsOneWidget);
    expect(find.widgetWithText(TextField, 'Password'), findsOneWidget);
    await tester.tap(find.byType(DropdownButtonFormField<String>));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Private key').last);
    await tester.pumpAndSettle();
    expect(find.text('Import OpenSSH private key'), findsOneWidget);
    expect(find.text('Private key passphrase (optional)'), findsOneWidget);
    expect(find.widgetWithText(TextField, 'Password'), findsNothing);
    await tester.tap(find.byType(DropdownButtonFormField<String>));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Password').last);
    await tester.pumpAndSettle();
    expect(find.widgetWithText(TextField, 'Password'), findsOneWidget);
    expect(find.text('Private key passphrase (optional)'), findsNothing);
    expect(find.text('初始路径(可选,默认/)'), findsOneWidget);

    // 切到百度网盘：根目录 + 授权按钮 + refresh_token
    await selectType('百度网盘');
    expect(find.text('根目录(默认/)'), findsOneWidget);
    expect(find.text('授权登录'), findsOneWidget);
    expect(find.text('refresh_token(授权后自动填入，也可直接粘贴)'), findsOneWidget);

    // 切到 115 网盘：根文件夹 ID + 扫码获取 Cookie（无需 APP ID）
    await selectType('115 网盘');
    expect(find.text('根文件夹 ID(留空=网盘根目录)'), findsOneWidget);
    expect(find.text('扫码获取 Cookie（无需 APP ID）'), findsOneWidget);
    expect(find.text('Cookie（扫码后自动填入，也可浏览器 F12 复制）'), findsOneWidget);

    // 切到夸克网盘：根文件夹 ID + Cookie 字段
    await selectType('夸克网盘');
    expect(find.text('根文件夹 ID(默认 0)'), findsOneWidget);
    expect(find.text('扫码获取 Cookie（无需 F12）'), findsOneWidget);
  });

  testWidgets('添加书源对话框：提交校验失败时错误信息自动滚动可见', (tester) async {
    await pumpDialog(tester);

    // 默认 WebDAV，服务器地址留空直接提交 → 触发校验错误
    await tester.tap(find.widgetWithText(FilledButton, '添加'));
    await tester.pumpAndSettle();

    final errFinder = find.text('请填写服务器地址');
    expect(errFinder, findsOneWidget);

    // 错误信息必须落在对话框可视区域内（防止窄屏/横屏下被滚动区折叠，
    // 导致用户以为“点了添加没反应”）
    final errRect = tester.getRect(errFinder);
    final dialogRect = tester.getRect(find.byType(AlertDialog));
    expect(
      errRect.bottom <= dialogRect.bottom + 1,
      isTrue,
      reason: '错误信息应自动滚动到可见位置',
    );
  });

  testWidgets('本地书源可以通过系统文件夹选择器填写目录', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.windows;
    final previousPlatform = FileSelectorPlatform.instance;
    final fakePlatform = _FakeFileSelectorPlatform();
    FileSelectorPlatform.instance = fakePlatform;
    try {
      await pumpDialog(tester);
      await tester.tap(find.byType(DropdownMenu<String>));
      await tester.pumpAndSettle();
      await tester.tap(find.text('本地目录').last);
      await tester.pumpAndSettle();

      await tester.tap(find.text('选择文件夹'));
      await tester.pumpAndSettle();

      expect(fakePlatform.directoryOptions?.confirmButtonText, '选择此目录');
      expect(
        tester.widget<TextField>(find.byType(TextField).last).controller!.text,
        r'F:\comic\测试漫画',
      );
    } finally {
      FileSelectorPlatform.instance = previousPlatform;
      debugDefaultTargetPlatformOverride = null;
    }
  });
}
