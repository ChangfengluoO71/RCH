import 'package:app/store/ai_upscale_manager.dart';
import 'package:flutter/material.dart';

/// 全局悬浮小窗：显示后台 AI 超分任务进度与完成提示。
class AiFloatingProgress extends StatelessWidget {
  const AiFloatingProgress({super.key});

  @override
  Widget build(BuildContext context) {
    return AnimatedBuilder(
      animation: AiUpscaleManager.instance,
      builder: (context, _) {
        final m = AiUpscaleManager.instance;
        final active = m.tasks.where((t) => t.isActive).toList();
        final completed = m.lastCompletedTitle;
        if (active.isEmpty && completed == null) return const SizedBox.shrink();
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
                if (completed != null)
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
