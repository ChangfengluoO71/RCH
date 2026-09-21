import 'package:app/store/remote_scan_models.dart';
import 'package:app/ui/remote_scan_status.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

/// 扫描状态面板契约（2026-09-21 重写）。
///
/// **背景**：用户决定"扫描自动运行"，本轮移除了面板上的
/// 「暂停/继续」「增量重新扫描」「全量重新扫描」三个按钮，只保留「重试远程扫描」。
/// 旧测试仍在构造 `onPause` / `onResume` / `onRescanIncremental` / `onRescanFull`
/// ⇒ CI 的 `flutter analyze` 有 12 处 error（发布门禁红线）。
/// 本文件改为**锁定删除结果**（回归测试）+ 重试动作契约，只使用真实存在的 API。
void main() {
  Widget panel({
    required RemoteScanViewState? state,
    Future<void> Function()? onRetry,
    String idleMessage = '等待扫描',
  }) {
    return MaterialApp(
      home: Scaffold(
        body: RemoteScanStatusPanel(
          sourceName: 'Cloud',
          stateListenable: ValueNotifier<RemoteScanViewState?>(state),
          idleMessage: idleMessage,
          onRetry: onRetry,
        ),
      ),
    );
  }

  const running = RemoteScanViewState(
    sourceId: 'source-a',
    status: 'running',
    mode: 'full',
  );
  const retryable = RemoteScanViewState(
    sourceId: 'source-a',
    status: 'failed',
    mode: 'full',
  );

  testWidgets('运行中不再提供暂停/继续/增量重扫/全量重扫（2026-09-21 移除）', (tester) async {
    await tester.pumpWidget(panel(state: running, onRetry: () async {}));
    for (final label in ['暂停', '继续', '增量重新扫描', '全量重新扫描']) {
      expect(find.text(label), findsNothing, reason: '不应再出现「$label」按钮');
      expect(find.byTooltip(label), findsNothing, reason: '不应再出现「$label」提示');
      expect(find.widgetWithText(TextButton, label), findsNothing);
    }
  });

  testWidgets('可重试状态渲染「重试远程扫描」，点击触发回调', (tester) async {
    var calls = 0;
    await tester.pumpWidget(
      panel(
        state: retryable,
        onRetry: () async {
          calls++;
        },
      ),
    );
    expect(find.byTooltip('重试远程扫描'), findsOneWidget);
    await tester.tap(find.byTooltip('重试远程扫描'));
    await tester.pump();
    expect(calls, 1);
  });

  testWidgets('未注入 onRetry 时不渲染重试按钮（不猜、不静默兜底）', (tester) async {
    await tester.pumpWidget(panel(state: retryable));
    expect(find.byTooltip('重试远程扫描'), findsNothing);
  });

  testWidgets('无状态时显示 idleMessage（默认「等待扫描」）', (tester) async {
    await tester.pumpWidget(panel(state: null));
    expect(find.textContaining('等待扫描'), findsWidgets);
  });

  testWidgets('自定义 idleMessage 会被使用', (tester) async {
    await tester.pumpWidget(panel(state: null, idleMessage: '尚未扫描本来源'));
    expect(find.textContaining('尚未扫描本来源'), findsWidgets);
  });
}
