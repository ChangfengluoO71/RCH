// 回归测试：详情页在安卓横屏矮视口（逻辑 853x480，即 MuMu 1280x720@1.5）
// 下封面列不得 RenderFlex 底部溢出（曾出现黄黑报错条遮挡“开始阅读”按钮）。
import 'package:app/ui/book_detail_page.dart';
import 'package:app/store/models.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  testWidgets('详情页矮屏(853x480)不出现 RenderFlex 溢出', (tester) async {
    tester.view.physicalSize = const Size(1280, 720);
    tester.view.devicePixelRatio = 1.5;
    final source = BookSource(
      id: 'local_test',
      type: 'local',
      name: 't',
      path: '/tmp',
    );
    await tester.pumpWidget(
      MaterialApp(
        home: BookDetailPage(
          source: source,
          path: '/tmp/test.pdf',
          title: 'test.pdf',
        ),
      ),
    );
    await tester.pump(const Duration(seconds: 1));
    expect(
      tester.takeException(),
      isNull,
      reason: '详情页封面列在矮屏下不应 RenderFlex 溢出',
    );
    tester.view.reset();
  });

  testWidgets('详情页显示可复制的原文件名', (tester) async {
    final source = BookSource(
      id: 'local_filename_test',
      type: 'local',
      name: 't',
      path: r'C:\comics',
    );
    const path = r'C:\comics\用户命名标题.cbz';

    await tester.pumpWidget(
      MaterialApp(
        home: BookDetailPage(source: source, path: path, title: '用户命名标题'),
      ),
    );
    await tester.pump();

    expect(find.text('原文件名：'), findsOneWidget);
    expect(find.text('用户命名标题.cbz'), findsOneWidget);
    expect(find.byKey(const Key('copy_original_filename')), findsOneWidget);
  });
}
