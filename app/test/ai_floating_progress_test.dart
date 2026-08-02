import 'package:app/store/ai_upscale_manager.dart';
import 'package:app/ui/ai_floating_progress.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'dart:async';

AiTask _task(String id, String title, AiTaskStatus status, int sortOrder) =>
    AiTask(
      id: id,
      bookKey: 'bk_$id',
      sourceType: 'local',
      sourceId: 'src',
      path: 'p_$id',
      title: title,
      status: status,
      sortOrder: sortOrder,
    );

/// 带 250ms 周期 setState 的列表，模拟 AiFloatingProgress 的轮询刷新。
class _TimerRebuildList extends StatefulWidget {
  const _TimerRebuildList({required this.order});
  final List<String> order;
  @override
  State<_TimerRebuildList> createState() => _TimerRebuildListState();
}

class _TimerRebuildListState extends State<_TimerRebuildList> {
  Timer? _timer;

  @override
  void initState() {
    super.initState();
    _timer = Timer.periodic(const Duration(milliseconds: 250), (_) {
      if (mounted) setState(() {});
    });
  }

  @override
  void dispose() {
    _timer?.cancel();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return ReorderableListView.builder(
      buildDefaultDragHandles: false,
      itemCount: widget.order.length,
      onReorderItem: (o, n) {
        final moved = widget.order.removeAt(o);
        widget.order.insert(n, moved);
      },
      itemBuilder: (c, i) => Row(
        key: ValueKey(widget.order[i]),
        children: [
          ReorderableDragStartListener(
            index: i,
            child: const Icon(Icons.drag_indicator),
          ),
          Text(widget.order[i]),
        ],
      ),
    );
  }
}

void main() {
  testWidgets('标准 ReorderableListView + 自定义手柄可被测试手势拖拽', (tester) async {
    final order = <String>['a', 'b', 'c'];
    await tester.pumpWidget(MaterialApp(
      home: Scaffold(
        body: SizedBox(
          height: 400,
          child: _TimerRebuildList(order: order),
        ),
      ),
    ));
    await tester.pumpAndSettle();
    final start = tester.getCenter(find.byIcon(Icons.drag_indicator).last);
    final g = await tester.startGesture(start);
    await tester.pump();
    await g.moveTo(start + const Offset(0, -150));
    await tester.pump(const Duration(seconds: 1));
    await g.up();
    await tester.pumpAndSettle();
    expect(order, isNot(['a', 'b', 'c']));
  });

  testWidgets('展开面板渲染：进行中置顶 + 排队可拖拽排序', (tester) async {
    final m = AiUpscaleManager.instance;
    m.debugSetTasks([
      _task('r', '进行中的书', AiTaskStatus.running, 0),
      _task('q1', '排队一', AiTaskStatus.queued, 1),
      _task('q2', '排队二', AiTaskStatus.queued, 2),
    ]);

    // 与 main.dart 一致：悬浮窗挂在 MaterialApp.builder 层（Navigator/Overlay 之上）。
    await tester.pumpWidget(MaterialApp(
      builder: (context, child) => Stack(
        children: [
          child!,
          const Align(
            alignment: Alignment.topRight,
            child: AiFloatingProgress(),
          ),
        ],
      ),
      home: const Scaffold(body: SizedBox()),
    ));
    await tester.pump();

    // 折叠态：显示任务数与展开按钮
    expect(find.text('AI 超分'), findsOneWidget);
    expect(find.byIcon(Icons.unfold_more), findsOneWidget);

    // 展开
    await tester.tap(find.byIcon(Icons.unfold_more));
    await tester.pumpAndSettle();
    expect(find.text('AI 超分任务'), findsOneWidget);
    expect(find.byIcon(Icons.expand_more), findsOneWidget);
    expect(find.byIcon(Icons.drag_indicator), findsNWidgets(2));
    // 进行中任务在最前
    expect(m.activeTasks.first.id, 'r');

    // 拖拽"排队二"上移一行 → 队列顺序变为 q2, q1
    final handle = find.byIcon(Icons.drag_indicator).last;
    final start = tester.getCenter(handle);
    final g = await tester.startGesture(start);
    await tester.pump();
    await g.moveTo(start + const Offset(0, -150));
    await tester.pump(const Duration(seconds: 1));
    await g.up();
    await tester.pumpAndSettle();
    expect(m.activeTasks.map((t) => t.id).toList(), ['r', 'q2', 'q1']);

    // 清理：卸载组件（取消周期 Timer）并清空单例
    await tester.pumpWidget(const SizedBox());
    m.debugSetTasks([]);
  });
}
