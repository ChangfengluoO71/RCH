import 'package:app/ui/webtoon_navigation.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

/// 用一个**真实 ListView** 复现"占位高度 → 真实高度"造成的可见内容后跳。
///
/// 完整 `ReaderPage` 需要 Rust FFI 才能挂载，所以这里用与阅读器**同一套**测量/补偿回调
/// （`WebtoonAnchorKeeper` + `ScrollPosition.correctBy`）搭最小骨架，专门钉住布局机制本身。
/// `compensate: false` 就是修复前的形状（用于证明这条回归确实能抓到问题）。
class _ListHarness extends StatefulWidget {
  const _ListHarness({required this.compensate, required this.heights});

  final bool compensate;
  final Map<int, double> heights;

  @override
  State<_ListHarness> createState() => _ListHarnessState();
}

class _ListHarnessState extends State<_ListHarness> {
  final ScrollController ctrl = ScrollController();
  final WebtoonAnchorKeeper anchor = WebtoonAnchorKeeper();
  final GlobalKey listKey = GlobalKey();

  @override
  void didUpdateWidget(covariant _ListHarness oldWidget) {
    super.didUpdateWidget(oldWidget);
    // 等价于阅读器里"某页字节到达"：告知 keeper 该页增长前的高度（占位 200）
    for (var i = 0; i < 6; i++) {
      final before = oldWidget.heights[i] ?? 200;
      final after = widget.heights[i] ?? 200;
      if (before != after) anchor.announceGrowth(i, before);
    }
  }

  @override
  void dispose() {
    ctrl.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      home: Scaffold(
        body: ListView.builder(
          key: listKey,
          controller: ctrl,
          itemCount: 6,
          itemBuilder: (context, i) => Builder(
            builder: (itemCtx) {
              WidgetsBinding.instance.addPostFrameCallback((_) {
                // 条目可能在测量回调之前已离开树（缓存区回收）——必须查条目自身的 mounted，
                // 否则 findRenderObject() 会打到 DEFUNCT element（阅读器侧同理，已同步加守卫）。
                if (!mounted || !itemCtx.mounted) return;
                final listCtx = listKey.currentContext;
                if (listCtx == null || !listCtx.mounted) return;
                final ro = itemCtx.findRenderObject();
                final listBox = listCtx.findRenderObject();
                if (ro is! RenderBox || listBox is! RenderBox) return;
                final top = listBox.globalToLocal(ro.localToGlobal(Offset.zero)).dy;
                final correction = anchor.record(
                  index: i,
                  newHeight: ro.size.height,
                  itemTopInViewport: top,
                );
                if (!widget.compensate || correction == 0 || !ctrl.hasClients) {
                  return;
                }
                final pos = ctrl.position;
                final target = (pos.pixels + correction)
                    .clamp(pos.minScrollExtent, pos.maxScrollExtent);
                pos.correctBy(target - pos.pixels);
                // correctBy 是静默纠偏（Flutter 内部在 layout 中用），帧后调用需要显式要求重排
                setState(() {});
              });
              return SizedBox(
                key: ValueKey<int>(i),
                height: widget.heights[i] ?? 200,
                child: ColoredBox(
                  color: i.isEven ? Colors.blue : Colors.green,
                  child: Center(child: Text('page $i')),
                ),
              );
            },
          ),
        ),
      ),
    );
  }
}

void main() {
  group('WebtoonAnchorKeeper', () {
    test('视口上方条目变高 ⇒ 等量补偿（正值）', () {
      final keeper = WebtoonAnchorKeeper();
      expect(
        keeper.record(index: 0, newHeight: 200, itemTopInViewport: -800),
        0,
        reason: '首次测量只记录，不补偿',
      );
      expect(
        keeper.record(index: 0, newHeight: 3000, itemTopInViewport: -800),
        2800,
        reason: '占位 200 → 真实 3000，视口上方整段增长必须补回',
      );
    });

    test('条目跨视口顶边或仍在视口内 ⇒ 不补偿', () {
      final keeper = WebtoonAnchorKeeper();
      keeper.record(index: 1, newHeight: 200, itemTopInViewport: -50);
      expect(keeper.record(index: 1, newHeight: 3000, itemTopInViewport: -50), 0);
      keeper.record(index: 2, newHeight: 200, itemTopInViewport: 300);
      expect(keeper.record(index: 2, newHeight: 900, itemTopInViewport: 300), 0);
    });

    test('变矮同样补偿（负值）；高度不变不补偿', () {
      final keeper = WebtoonAnchorKeeper();
      keeper.record(index: 0, newHeight: 900, itemTopInViewport: -5000);
      expect(keeper.record(index: 0, newHeight: 400, itemTopInViewport: -5000), -500);
      expect(keeper.record(index: 0, newHeight: 400, itemTopInViewport: -5000), 0);
    });

    test('未构建过的页（只有增长预警）也能算出补偿', () {
      final keeper = WebtoonAnchorKeeper();
      keeper.announceGrowth(3, 200); // 该页字节到达，前值是 200 占位
      expect(
        keeper.record(index: 3, newHeight: 3000, itemTopInViewport: -1200),
        2800,
        reason: '预警必须让 keeper 知道前值，否则这次位移会被漏掉',
      );
      expect(
        keeper.record(index: 3, newHeight: 3000, itemTopInViewport: -1200),
        0,
        reason: '预警只消费一次（同一高度重复测量不再补偿）',
      );
    });

    test('reset 后重新按首次测量处理', () {
      final keeper = WebtoonAnchorKeeper();
      keeper.record(index: 0, newHeight: 200, itemTopInViewport: -900);
      keeper.announceGrowth(1, 200);
      keeper.reset();
      expect(keeper.trackedCount, 0);
      expect(keeper.record(index: 0, newHeight: 3000, itemTopInViewport: -900), 0);
      expect(
        keeper.record(index: 1, newHeight: 3000, itemTopInViewport: -900),
        0,
        reason: 'reset 连预警一起作废',
      );
    });
  });

  group('条漫滚动锚点（真实 ListView）', () {
    Future<void> resolveAbove(WidgetTester tester, bool compensate) async {
      // 传**新实例**才会真的重建（同实例 pumpWidget 会被框架跳过重建）
      await tester.pumpWidget(
        _ListHarness(
          compensate: compensate,
          heights: const <int, double>{0: 3000, 1: 3000},
        ),
      );
      await tester.pump(); // 新高度布局 + 帧后回调（补偿在此施加）
      await tester.pump();
    }

    /// 定向复现真触发路径：整段轨迹 =「拖到 item1 贴顶」→（可选）上方页占位收敛
    /// →「继续拖 40px」。最后那段继续拖拽会持续产生布局帧，正是真实快速下拉的形态
    /// （手指还在动 ⇒ 纠偏后的偏移下一帧就生效，而不是停在静默纠偏上）。
    ///
    /// 返回最终 item1 的 dy；被推走后不再构建则返回 null。
    Future<double?> run({
      required WidgetTester tester,
      required bool compensate,
      required bool resolveMidway,
    }) async {
      await tester.pumpWidget(
        _ListHarness(compensate: compensate, heights: const <int, double>{}),
      );
      await tester.pump();
      await tester.drag(find.byType(ListView), const Offset(0, -220));
      await tester.pumpAndSettle();
      if (resolveMidway) {
        await resolveAbove(tester, compensate);
      }
      // 手指继续动：这一小段拖拽保证每帧都重排（真实快速下拉就是这个节奏）
      await tester.drag(find.byType(ListView), const Offset(0, -40));
      await tester.pumpAndSettle();
      final found = find.byKey(const ValueKey<int>(1)).evaluate();
      return found.isEmpty
          ? null
          : tester.getTopLeft(find.byKey(const ValueKey<int>(1))).dy;
    }

    testWidgets('对照实验：上方占位收敛 + 持续拖拽 ⇒ 锚点不动（修复目标）', (tester) async {
      final control = await run(tester: tester, compensate: true, resolveMidway: false);
      expect(control, isNotNull, reason: '对照跑：无增长时 item1 应当可见');

      // 换新 tester 场景重跑（每个 testWidgets 只有一棵树，这里在同一个 test 里顺序跑两次）
      await tester.pumpWidget(const SizedBox.shrink());
      final fixed = await run(tester: tester, compensate: true, resolveMidway: true);

      expect(
        fixed,
        isNotNull,
        reason: '有补偿时 item1 必须仍在视口内（不补偿会被推到 5600px 之外）',
      );
      expect(
        fixed,
        moreOrLessEquals(control!, epsilon: 1.0),
        reason: '同样的拖拽轨迹下，上方占位收敛不得额外推动可见内容：'
            '对照 dy=$control，收敛后 dy=$fixed',
      );
    });

    testWidgets('对照实验：不补偿时锚点被推走（钉住根因，防止修复被误删）', (tester) async {
      final control = await run(tester: tester, compensate: false, resolveMidway: false);
      await tester.pumpWidget(const SizedBox.shrink());
      final broken = await run(tester: tester, compensate: false, resolveMidway: true);

      expect(
        broken == null || (control != null && (broken - control).abs() > 100),
        isTrue,
        reason: '没有锚点补偿时可见内容会被推走（用户报的"跳回好几页前"）：'
            '对照 dy=$control，收敛后 dy=$broken',
      );
    });
  });

  group('WebtoonNavigationModel', () {
    test('unknown extents use a non-zero estimate and keep offsets monotonic', () {
      final model = WebtoonNavigationModel(pageCount: 100, estimatedHeight: 240);

      expect(model.offsetFor(0), 0);
      expect(model.offsetFor(1), 240);
      expect(model.offsetFor(50), 50 * 240);
      expect(model.offsetFor(100), 100 * 240);
    });

    test('latest programmatic target wins and stale generations are ignored', () {
      final model = WebtoonNavigationModel(pageCount: 20);
      final first = model.requestTarget(10);
      final second = model.requestTarget(3);

      expect(first.generation, lessThan(second.generation));
      expect(model.pendingTarget, 3);
      expect(model.accepts(first.generation), isFalse);
      expect(model.accepts(second.generation), isTrue);
      expect(model.offsetForIntent(first), model.offsetFor(10));
    });

    test('programmatic completion clears only the matching pending target', () {
      final model = WebtoonNavigationModel(pageCount: 20);
      final first = model.requestTarget(10);
      final second = model.requestTarget(3);

      expect(model.completeProgrammatic(first), isFalse);
      expect(model.pendingTarget, 3);
      expect(model.completeProgrammatic(second), isTrue);
      expect(model.pendingTarget, isNull);
      expect(model.stablePage, 3);
    });

    test('fast scroll observes viewport without overwriting stable page until settle', () {
      final model = WebtoonNavigationModel(pageCount: 20, estimatedHeight: 200);
      expect(model.stablePage, 0);

      model.observe(offset: 1700, viewportExtent: 200, isScrolling: true);
      expect(model.viewportPage, 9);
      expect(model.stablePage, 0);

      model.observe(offset: 1700, viewportExtent: 200, isScrolling: false);
      model.settle();
      expect(model.stablePage, 9);
    });

    test('user gesture cancels a pending target without jumping to stale target', () {
      final model = WebtoonNavigationModel(pageCount: 20, estimatedHeight: 200);
      final intent = model.requestTarget(12);
      model.cancelPendingForUserGesture();
      model.observe(offset: 400, viewportExtent: 200, isScrolling: false);
      model.settle();

      expect(model.accepts(intent.generation), isFalse);
      expect(model.pendingTarget, isNull);
      expect(model.stablePage, 2);
    });

    test('measured heights replace estimates while preserving monotonic offsets', () {
      final model = WebtoonNavigationModel(pageCount: 3, estimatedHeight: 200);
      model.measure(0, 500);
      model.measure(1, 100);

      expect(model.offsetFor(1), 500);
      expect(model.offsetFor(2), 600);
      expect(model.offsetFor(3), 800);
    });
  });
}
