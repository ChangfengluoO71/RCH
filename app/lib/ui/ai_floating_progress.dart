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
        final active = m.tasks.where((t) => t.isActive).toList();
        final completed = m.lastCompletedTitle;
        final failed = m.lastFailedMessage;
        if (active.isEmpty && completed == null && failed == null) {
          return const SizedBox.shrink();
        }
        return Material(
          key: const ValueKey('ai_progress'),
          elevation: 6,
          borderRadius: BorderRadius.circular(12),
          color: Colors.black.withValues(alpha: 0.82),
          child: Padding(
            padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 10),
            child: Column(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
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
          ),
        );
      },
    );
  }

  Widget _taskTile(AiTask t) {
    final label = t.status == AiTaskStatus.running ? 'AI 超分中' : '排队中';
    final progress = t.total > 0 ? t.done / t.total : 0.0;
    return Padding(
      padding: const EdgeInsets.only(bottom: 4),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              Text('$label ${t.done}/${t.total}',
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
