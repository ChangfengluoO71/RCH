import 'dart:typed_data';
import 'dart:ui' as ui;

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
  // 回归测试：reader_page.dart 的“缩放后移动区域只在第一页生效”Bug。
  // 根因：_go() 只调用 _photoCtrl.reset()，而 PhotoView 内部的
  // PhotoViewScaleStateController 仍停留在 zoomedIn。翻页时图片更换导致
  // scale boundaries 变化，但 PhotoViewCore 因 scaleState 处于 zooming 状态
  // 跳过缩放重算（photo_view_core.dart 的 scale getter），新页沿用上一页的
  // 缩放值 —— 第一页状态是全新的（initial），所以缩放/拖动只在第一页正常。
  // 修复：翻页/跳转/0 键复位/版本切换时 scaleState 与 photoCtrl 一起重置。
  testWidgets('翻页换图后缩放回到新页 contained，而非沿用上一页缩放',
      (tester) async {
    // 4x2 图在 200x200 视口 contained=50；8x2 图 contained=25。
    late Uint8List bytesA;
    late Uint8List bytesB;
    await tester.runAsync(() async {
      bytesA = await _pngBytes(4, 2);
      bytesB = await _pngBytes(8, 2);
    });

    final ctrl = PhotoViewController();
    final scaleState = PhotoViewScaleStateController();
    addTearDown(ctrl.dispose);
    addTearDown(scaleState.dispose);

    // 第 1 页：图片 A 加载完成 → contained(50)。
    await _pumpPhotoView(tester, ctrl, scaleState, bytesA);
    expect(ctrl.scale, closeTo(50, 1));

    // 用户放大（等效 +/- 键或捏合手势）。
    ctrl.scale = 120;
    await tester.pump();
    expect(ctrl.scale, closeTo(120, 1));

    // 模拟翻页（旧行为）：只重置 photoCtrl，然后换到图片 B。
    // PhotoView 元素存活、scaleState 残留 zoomedIn → 缩放沿用 A 的 50，
    // 而不是 B 的 contained(25)——即“翻页后页面卡在上一页的放大状态”。
    ctrl.reset();
    await _pumpPhotoView(tester, ctrl, scaleState, bytesB);
    expect(ctrl.scale, closeTo(50, 1),
        reason: '旧 Bug：只重置 photoCtrl 时，翻页后沿用上一页缩放');

    // 模拟翻页（修复行为）：scaleState 与 photoCtrl 一并重置后换图，
    // 新页正确回到 contained(25)，缩放/拖动在新页恢复正常。
    ctrl.reset();
    scaleState.reset();
    await _pumpPhotoView(tester, ctrl, scaleState, bytesB);
    expect(ctrl.scale, closeTo(25, 1),
        reason: '修复后：翻页回到新页 contained，缩放/拖动可再次使用');
  });

  // 双页模式回归：_buildPair 的 InteractiveViewer 原实现 panEnabled=false，
  // 键盘 +/- 放大后无法拖动查看细节（单页模式修复后双页仍存在该问题）。
  testWidgets('双页模式缩放后可拖动（InteractiveViewer panEnabled）',
      (tester) async {
    final ctrl = TransformationController();
    addTearDown(ctrl.dispose);

    await tester.pumpWidget(MaterialApp(
      home: Center(
        child: SizedBox(
          width: 200,
          height: 200,
          child: InteractiveViewer(
            transformationController: ctrl,
            minScale: 1.0,
            maxScale: 4.0,
            scaleEnabled: false,
            panEnabled: true, // 修复：缩放后可拖动
            child: Container(
              width: 200,
              height: 200,
              color: const Color(0xFF3366CC),
            ),
          ),
        ),
      ),
    ));

    // 放大 2x（等效双页模式的 +/- 键缩放）。
    ctrl.value = Matrix4.identity()..scaleByDouble(2.0, 2.0, 2.0, 1.0);
    await tester.pump();

    final before = ctrl.value.getTranslation();
    // 初始矩阵是原点锚定缩放（等效 _zoomIV），内容锚在左上角，
    // 只能向左/上拖动（向右/下会被边界钳制回 0）。
    await tester.drag(find.byType(InteractiveViewer), const Offset(-30, -20));
    await tester.pump();
    final after = ctrl.value.getTranslation();

    expect((after - before).length, greaterThan(1.0),
        reason: '双页模式缩放后应能拖动（旧实现 panEnabled=false 无法拖动）');
  });
}

/// 构建带指定图片的 PhotoView（PhotoView 元素跨调用保持存活，模拟翻页），
/// 并等待真实图片解码完成（PhotoViewCore 会把推导出的 scale 写回 controller）。
Future<void> _pumpPhotoView(
  WidgetTester tester,
  PhotoViewController ctrl,
  PhotoViewScaleStateController scaleState,
  Uint8List bytes,
) async {
  await tester.runAsync(() async {
    await tester.pumpWidget(MaterialApp(
      home: Center(
        child: SizedBox(
          width: 200,
          height: 200,
          child: PhotoView(
            imageProvider: MemoryImage(bytes),
            controller: ctrl,
            scaleStateController: scaleState,
            initialScale: PhotoViewComputedScale.contained,
            minScale: PhotoViewComputedScale.contained,
            maxScale: PhotoViewComputedScale.covered * 8,
          ),
        ),
      ),
    ));
    // 图片解码是真实异步：等待若干帧让 ImageWrapper 完成加载并触发重算。
    await Future<void>.delayed(const Duration(milliseconds: 150));
    await tester.pump();
  });
  await tester.pump();
}
