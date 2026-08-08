import 'dart:typed_data';
import 'dart:ui' as ui;

import 'package:app/ui/reader_page.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:photo_view/photo_view.dart';

/// 生成 width x height 的纯色 PNG 字节（测试用）。
Future<Uint8List> _pngBytes(int width, int height) async {
  final recorder = ui.PictureRecorder();
  final canvas = Canvas(recorder);
  canvas.drawRect(
    Rect.fromLTWH(0, 0, width.toDouble(), height.toDouble()),
    Paint()..color = const Color(0xFFCC3333),
  );
  final picture = recorder.endRecording();
  final image = await picture.toImage(width, height);
  final data = await image.toByteData(format: ui.ImageByteFormat.png);
  return data!.buffer.asUint8List();
}

void main() {
  group('ReaderPaging 视口映射', () {
    test('单页模式: 视口与真实页一一对应', () {
      const paging = ReaderPaging(dual: false, skipCover: false, pageCount: 10);
      expect(paging.viewCount, 10);
      for (var p = 0; p < 10; p++) {
        expect(paging.pageOfView(paging.viewOfPage(p)), p);
      }
    });

    test('双页模式(不跳过封面): 每视口两页', () {
      const paging = ReaderPaging(dual: true, skipCover: false, pageCount: 5);
      expect(paging.viewCount, 3); // (0,1) (2,3) (4)
      expect(paging.viewOfPage(0), 0);
      expect(paging.viewOfPage(1), 0);
      expect(paging.viewOfPage(2), 1);
      expect(paging.viewOfPage(4), 2);
      expect(paging.pageOfView(0), 0);
      expect(paging.pageOfView(1), 2);
      expect(paging.pageOfView(2), 4);
    });

    test('双页模式(跳过封面): 首页独占,其余两页一组', () {
      const paging = ReaderPaging(dual: true, skipCover: true, pageCount: 5);
      expect(paging.viewCount, 3); // 0 | (1,2) (3,4)
      expect(paging.viewOfPage(0), 0);
      expect(paging.viewOfPage(1), 1);
      expect(paging.viewOfPage(2), 1);
      expect(paging.viewOfPage(3), 2);
      expect(paging.pageOfView(0), 0);
      expect(paging.pageOfView(1), 1);
      expect(paging.pageOfView(2), 3);
    });

    test('双页模式边界页数', () {
      expect(const ReaderPaging(dual: true, skipCover: true, pageCount: 1).viewCount, 1);
      expect(const ReaderPaging(dual: true, skipCover: true, pageCount: 2).viewCount, 2);
      expect(const ReaderPaging(dual: true, skipCover: true, pageCount: 4).viewCount, 3);
      expect(const ReaderPaging(dual: true, skipCover: false, pageCount: 4).viewCount, 2);
      expect(const ReaderPaging(dual: true, skipCover: false, pageCount: 1).viewCount, 1);
    });
  });

  testWidgets('条漫: 单指滚动与双指缩放共存', (tester) async {
    final ctrl = TransformationController();
    addTearDown(ctrl.dispose);

    await tester.pumpWidget(MaterialApp(
      home: Scaffold(
        body: InteractiveViewer(
          transformationController: ctrl,
          minScale: 1.0,
          maxScale: 4.0,
          scaleEnabled: true,
          panEnabled: false,
          child: ListView.builder(
            itemCount: 40,
            itemBuilder: (_, i) => Container(
              height: 200,
              color: i.isEven ? const Color(0xFF3366CC) : const Color(0xFFCC6633),
            ),
          ),
        ),
      ),
    ));

    // 单指上滑应滚动列表。
    await tester.drag(find.byType(ListView), const Offset(0, -400));
    await tester.pumpAndSettle();
    final scrollable = tester.state<ScrollableState>(find.byType(Scrollable).first);
    expect(scrollable.position.pixels, greaterThan(0),
        reason: '开启缩放手势后不应抢走列表的单指滚动');

    // 双指捏合应放大。
    final center = tester.getCenter(find.byType(InteractiveViewer));
    final g1 = await tester.startGesture(center - const Offset(30, 0), pointer: 1);
    final g2 = await tester.startGesture(center + const Offset(30, 0), pointer: 2);
    await tester.pump(const Duration(milliseconds: 20));
    await g1.moveBy(const Offset(-60, 0));
    await g2.moveBy(const Offset(60, 0));
    await tester.pump();
    await g1.up();
    await g2.up();
    await tester.pumpAndSettle();
    expect(ctrl.value.getMaxScaleOnAxis(), greaterThan(1.0),
        reason: '条漫模式双指捏合应能缩放');
  });

  testWidgets('日漫/美漫: PageView 滑动翻页(photo_view 手势让位)', (tester) async {
    final bytes = <Uint8List>[];
    await tester.runAsync(() async {
      for (var i = 0; i < 3; i++) {
        bytes.add(await _pngBytes(4, 2));
      }
    });

    final ctrl = PageController();
    addTearDown(ctrl.dispose);

    await tester.pumpWidget(MaterialApp(
      home: Scaffold(
        body: PageView.builder(
          controller: ctrl,
          itemCount: 3,
          itemBuilder: (_, i) => PhotoViewGestureDetectorScope(
            axis: Axis.horizontal,
            child: PhotoView(
              imageProvider: MemoryImage(bytes[i]),
              initialScale: PhotoViewComputedScale.contained,
              minScale: PhotoViewComputedScale.contained,
              maxScale: PhotoViewComputedScale.covered * 8,
            ),
          ),
        ),
      ),
    ));
    await tester.runAsync(() => Future<void>.delayed(const Duration(milliseconds: 150)));
    await tester.pump();

    expect(ctrl.page, 0);
    await tester.fling(find.byType(PageView), const Offset(-300, 0), 1200);
    // photo_view 图片加载圈是持续动画,不能用 pumpAndSettle;用有界 pump 等翻页弹道结束。
    await tester.pump(const Duration(milliseconds: 300));
    await tester.pump(const Duration(milliseconds: 300));
    await tester.pump(const Duration(milliseconds: 300));
    expect(ctrl.page ?? 0, greaterThan(0.9),
        reason: '未放大时水平滑动应交给 PageView 翻页');
  });

  testWidgets('双页模式: 未放大时可翻页,放大后手势被 InteractiveViewer 接管', (tester) async {
    final ivCtrl = TransformationController();
    addTearDown(ivCtrl.dispose);
    final pageCtrl = PageController();
    addTearDown(pageCtrl.dispose);

    Widget buildView({required bool panEnabled}) => MaterialApp(
          home: Scaffold(
            body: PageView.builder(
              controller: pageCtrl,
              itemCount: 3,
              itemBuilder: (_, i) => InteractiveViewer(
                transformationController: ivCtrl,
                minScale: 1.0,
                maxScale: 4.0,
                scaleEnabled: false,
                panEnabled: panEnabled,
                child: Container(color: const Color(0xFF3366CC)),
              ),
            ),
          ),
        );

    await tester.pumpWidget(buildView(panEnabled: false));
    await tester.fling(find.byType(PageView), const Offset(-300, 0), 1200);
    await tester.pumpAndSettle();
    expect(pageCtrl.page, closeTo(1, 0.01),
        reason: '未放大时双页视图应可滑动翻页');

    ivCtrl.value = Matrix4.identity()..scaleByDouble(2.0, 2.0, 2.0, 1.0);
    await tester.pumpWidget(buildView(panEnabled: true));
    await tester.pump();
    final before = pageCtrl.page ?? 0;
    await tester.drag(find.byType(PageView), const Offset(-200, 0));
    await tester.pumpAndSettle();
    expect(pageCtrl.page, closeTo(before, 0.01),
        reason: '放大后拖拽应被 InteractiveViewer 接管,不翻页');
  });
}
