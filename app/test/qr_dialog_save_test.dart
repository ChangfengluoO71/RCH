import 'dart:async';

import 'package:app/ui/cloud115_qr_scan.dart';
import 'package:flutter/material.dart';
import 'package:flutter_rust_bridge/flutter_rust_bridge_for_generated.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  testWidgets('保存二维码图片 exports a PNG and reports success', (tester) async {
    final savedBytes = Completer<Uint8List>();
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: Center(
            child: Cloud115CookieQrScanDialog(
              uid: 'u1',
              time: PlatformInt64Util.from(0),
              sign: 's1',
              qrcode: 'https://115.com/scan/test',
              app: 'wechatmini',
              imageSaver: (bytes) async {
                if (!savedBytes.isCompleted) savedBytes.complete(bytes);
              },
            ),
          ),
        ),
      ),
    );
    await tester.pump(const Duration(milliseconds: 100));

    expect(find.text('保存二维码图片'), findsOneWidget);
    await tester.tap(find.text('保存二维码图片'));
    await tester.pump();
    await tester.runAsync(
      () => Future<void>.delayed(const Duration(milliseconds: 200)),
    );
    await tester.pump();

    expect(savedBytes.isCompleted, isTrue);
    final exportedBytes = await savedBytes.future;
    expect(exportedBytes, isNotNull);
    expect(exportedBytes.length, greaterThan(8));
    expect(
      exportedBytes.sublist(0, 8),
      orderedEquals(<int>[137, 80, 78, 71, 13, 10, 26, 10]),
    );
    expect(find.textContaining('二维码已保存'), findsOneWidget);

    await tester.pumpWidget(const SizedBox());
  });

  testWidgets('save failure reports a retryable status', (tester) async {
    final saveAttempted = Completer<void>();
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: Center(
            child: Cloud115CookieQrScanDialog(
              uid: 'u1',
              time: PlatformInt64Util.from(0),
              sign: 's1',
              qrcode: 'https://115.com/scan/test',
              app: 'wechatmini',
              imageSaver: (_) async {
                if (!saveAttempted.isCompleted) saveAttempted.complete();
                throw StateError('permission denied');
              },
            ),
          ),
        ),
      ),
    );
    await tester.pump(const Duration(milliseconds: 100));

    await tester.tap(find.text('保存二维码图片'));
    await tester.pump();
    await tester.runAsync(
      () => Future<void>.delayed(const Duration(milliseconds: 200)),
    );
    await tester.pump();

    expect(saveAttempted.isCompleted, isTrue);
    expect(find.textContaining('保存二维码失败'), findsOneWidget);
    await tester.pumpWidget(const SizedBox());
  });

  testWidgets('save action remains usable on a phone-width dialog', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(1080, 1920);
    tester.view.devicePixelRatio = 3;
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: Center(
            child: Cloud115CookieQrScanDialog(
              uid: 'u1',
              time: PlatformInt64Util.from(0),
              sign: 's1',
              qrcode: 'https://115.com/scan/test',
              app: 'wechatmini',
              imageSaver: (_) async {},
            ),
          ),
        ),
      ),
    );
    await tester.pump(const Duration(milliseconds: 100));

    expect(tester.takeException(), isNull);
    expect(find.text('保存二维码图片'), findsOneWidget);

    await tester.pumpWidget(const SizedBox());
    tester.view.reset();
  });
}
