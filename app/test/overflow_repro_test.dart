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

  /// 2026-09-21（用户报告 + 真机截图）：夸克等网盘的"容器文件夹"路径末段是 provider 的
  /// 不透明 id（32/64 位 hex），原来会被当成"原文件名"显示成一串哈希
  /// （`c680a216916d4ef88e1526e45ea49861`）。现在回退到目录项名称（调用点传入的 `title`）。
  testWidgets('末段是 provider id 时显示目录名而不是哈希', (tester) async {
    final source = BookSource(
      id: 'container_id_test',
      type: 'local',
      name: 't',
      path: r'C:\comics',
    );
    const hash = 'c680a216916d4ef88e1526e45ea49861';
    const name = 'W-舞冰的祈愿-金牌得主';

    await tester.pumpWidget(
      MaterialApp(
        home: BookDetailPage(
          source: source,
          path: '/39954038cd1c43a09eb2fd254668a1bb/$hash',
          title: name,
        ),
      ),
    );
    await tester.pump();

    expect(find.text('原文件名：'), findsOneWidget);
    expect(
      find.byWidgetPredicate((w) => w is SelectableText && w.data == name),
      findsOneWidget,
      reason: '原文件名应回退到目录项名称',
    );
    expect(
      find.text(hash),
      findsNothing,
      reason: '不得把 provider id 当原文件名显示',
    );
  });

  /// 2026-09-21（用户报告）：**115 的目录/文件 id 是 19 位纯数字**
  /// （真机 DB 实测 `3491122006131214136`、`3502050240473597592`）。
  /// 与夸克（32 位 hex）同类：末段是 id 时不得直接当"原文件名"显示。
  testWidgets('115 的 19 位数字 id 末段回退到目录名', (tester) async {
    final source = BookSource(
      id: '115_id_test',
      type: '115',
      name: 't',
      path: '0',
    );

    // ① 单段 id 路径（115 的目录/文件路径就是这个形状）
    await tester.pumpWidget(
      MaterialApp(
        home: BookDetailPage(
          source: source,
          path: '/3491122006131214136',
          title: '日漫',
        ),
      ),
    );
    await tester.pump();
    expect(
      find.byWidgetPredicate((w) => w is SelectableText && w.data == '日漫'),
      findsOneWidget,
      reason: '115 的 19 位 id 必须回退到目录名',
    );
    expect(find.text('3491122006131214136'), findsNothing);

    // ② 多段全 id：向前找不到正常名字 ⇒ 退回目录项名称
    await tester.pumpWidget(
      MaterialApp(
        home: BookDetailPage(
          source: source,
          path: '/3491082995119425384/3502050240473597592',
          title: '画集',
        ),
      ),
    );
    await tester.pump();
    expect(
      find.byWidgetPredicate((w) => w is SelectableText && w.data == '画集'),
      findsOneWidget,
    );
    expect(find.text('3502050240473597592'), findsNothing);

    // ③ 混合路径：向前走找到第一个正常名字就用它（比目录项名称更贴近真实路径）
    await tester.pumpWidget(
      MaterialApp(
        home: BookDetailPage(
          source: source,
          path: '/3491082995119425384/金牌得主',
          title: '不一致的标题',
        ),
      ),
    );
    await tester.pump();
    expect(
      find.byWidgetPredicate((w) => w is SelectableText && w.data == '金牌得主'),
      findsOneWidget,
    );
  });
}
