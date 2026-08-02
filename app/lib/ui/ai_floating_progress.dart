import 'dart:async';

import 'package:app/store/ai_upscale_manager.dart';
import 'package:flutter/material.dart';

/// 全局悬浮小窗：显示后台 AI 超分任务进度 / 完成 / 失败提示。
///
/// 用 250ms 周期刷新保证进行中的进度始终可见
/// （不依赖 notify → 重绘的时序，缓存全命中等快速场景也不会跳变）。
class AiFloatingProgress extends StatefulWidget {
  const AiFloatingProgress({super.key});

  @override
  State<AiFloatingProgress> createState() => _AiFloatingProgressState();
}

class _AiFloatingProgressState extends State<AiFloatingProgress> {
  Timer? _timer;
  bool _expanded = false; // true=展开任务列表（可拖拽排序）

  @override
  void initState() {
    super.initState();
    _timer = Timer.periodic(const Duration(milliseconds: 250), (_) {
      if (mounted && _hasContent()) setState(() {});
    });
  }

  bool _hasContent() {
    final m = AiUpscaleManager.instance;
    return m.tasks.any((t) => t.isActive) ||
        m.lastCompletedTitle != null ||
        m.lastFailedMessage != null;
  }

  @override
  void dispose() {
    _timer?.cancel();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return AnimatedBuilder(
      animation: AiUpscaleManager.instance,
      builder: (context, _) {
        final m = AiUpscaleManager.instance;
        final active = m.activeTasks;
        final completed = m.lastCompletedTitle;
        final failed = m.lastFailedMessage;
        if (active.isEmpty && completed == null && failed == null) {
          return const SizedBox.shrink();
        }
        final panel = Material(
          key: const ValueKey('ai_progress'),
          elevation: 6,
          borderRadius: BorderRadius.circular(12),
          color: Colors.black.withValues(alpha: 0.82),
          child: _expanded && active.isNotEmpty
              ? _expandedPanel(m, active)
              : _miniPanel(m, active, completed, failed),
        );
        // 悬浮窗挂在 MaterialApp.builder 层（Navigator/Overlay 之上），而
        // ReorderableListView（拖拽代理）和 Tooltip 都需要 Overlay 祖先，
        // 因此展开形态用局部 Overlay 承载，面板定位右上角，其余区域不挡点击。
        if (_expanded && active.isNotEmpty) {
          return Overlay(
            initialEntries: [
              OverlayEntry(
                builder: (context) =>
                    Positioned(top: 0, right: 0, child: panel),
              ),
            ],
          );
        }
        return panel;
      },
    );
  }

  /// 折叠态：任务数 + 展开按钮 + 精简进度行。
  Widget _miniPanel(
      AiUpscaleManager m, List<AiTask> active, String? completed, String? failed) {
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 8),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          if (active.isNotEmpty)
            Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                const Text('AI 超分',
                    style: TextStyle(color: Colors.white70, fontSize: 11)),
                const SizedBox(width: 4),
                Text('${active.length}',
                    style: const TextStyle(color: Colors.white38, fontSize: 11)),
                const SizedBox(width: 4),
                InkWell(
                  onTap: () => setState(() => _expanded = true),
                  child: const Icon(Icons.unfold_more, size: 16, color: Colors.white54),
                ),
              ],
            ),
          if (failed != null)
            Text(failed,
                maxLines: 2,
                overflow: TextOverflow.ellipsis,
                style: const TextStyle(color: Colors.redAccent, fontSize: 12))
          else if (completed != null)
            Text('已完成《$completed》',
                style: const TextStyle(color: Colors.greenAccent, fontSize: 13))
          else
            for (final t in active) _taskTile(t),
        ],
      ),
    );
  }

  /// 展开态：任务列表；进行中固定在顶部不可拖，排队任务可拖拽排序。
  Widget _expandedPanel(AiUpscaleManager m, List<AiTask> active) {
    return Container(
      width: 340,
      constraints: const BoxConstraints(maxHeight: 380),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Padding(
            padding: const EdgeInsets.fromLTRB(12, 10, 6, 4),
            child: Row(
              children: [
                const Text('AI 超分任务',
                    style: TextStyle(
                        color: Colors.white,
                        fontWeight: FontWeight.w600,
                        fontSize: 13)),
                const SizedBox(width: 8),
                Text('${active.length} 个',
                    style: const TextStyle(color: Colors.white54, fontSize: 11)),
                const Spacer(),
                IconButton(
                  icon: const Icon(Icons.expand_more, size: 18, color: Colors.white70),
                  tooltip: '收起',
                  visualDensity: VisualDensity.compact,
                  onPressed: () => setState(() => _expanded = false),
                ),
              ],
            ),
          ),
          const Divider(height: 1, color: Colors.white12),
          ConstrainedBox(
            constraints: const BoxConstraints(maxHeight: 320),
            child: ReorderableListView.builder(
              shrinkWrap: true,
              physics: const ClampingScrollPhysics(),
              buildDefaultDragHandles: false,
              padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 4),
              itemCount: active.length,
              onReorderItem: (o, n) => m.reorderQueued(o, n),
              itemBuilder: (c, i) {
                final t = active[i];
                final running = t.status == AiTaskStatus.running;
                return Padding(
                  key: ValueKey(t.id),
                  padding: const EdgeInsets.symmetric(vertical: 2),
                  child: Row(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      if (running)
                        const SizedBox(width: 28)
                      else
                        ReorderableDragStartListener(
                          index: i,
                          child: const Padding(
                            padding: EdgeInsets.only(right: 8, top: 6),
                            child: Icon(Icons.drag_indicator,
                                size: 20, color: Colors.white54),
                          ),
                        ),
                      Expanded(child: _taskTile(t)),
                    ],
                  ),
                );
              },
            ),
          ),
        ],
      ),
    );
  }

  Widget _taskTile(AiTask t) {
    final label = t.status == AiTaskStatus.running ? 'AI 超分中' : '排队中';
    final progress = t.total > 0 ? t.done / t.total : 0.0;
    // 排队中（total 未知）不显示误导性的 0/0。
    final countText = t.total > 0 ? '${t.done}/${t.total}' : '';
    return Padding(
      padding: const EdgeInsets.only(bottom: 4),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              Text(countText.isEmpty ? label : '$label $countText',
                  style: const TextStyle(color: Colors.white, fontSize: 12)),
              const SizedBox(width: 10),
              InkWell(
                onTap: () => AiUpscaleManager.instance.cancel(t.id),
                child: const Text('取消',
                    style: TextStyle(color: Colors.redAccent, fontSize: 12)),
              ),
            ],
          ),
          const SizedBox(height: 4),
          SizedBox(
            width: 190,
            child: LinearProgressIndicator(
              value: progress,
              minHeight: 3,
              backgroundColor: Colors.white12,
            ),
          ),
          const SizedBox(height: 4),
          Text(t.title,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: const TextStyle(color: Colors.white54, fontSize: 11)),
        ],
      ),
    );
  }
}
