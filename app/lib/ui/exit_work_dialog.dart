import 'package:app/store/cloud115_bt_monitor.dart';
import 'package:app/store/long_task_registry.dart';
import 'package:flutter/material.dart';

class ExitWorkDialog extends StatefulWidget {
  const ExitWorkDialog({
    super.key,
    required this.tasks,
    required this.monitorSummary,
    this.monitorStatusUnknown = false,
    required this.prepareForExit,
  });

  final List<LongTaskSnapshot> tasks;
  final Cloud115MonitorSummary monitorSummary;
  final bool monitorStatusUnknown;
  final Future<ExitPreparationResult> Function() prepareForExit;

  @override
  State<ExitWorkDialog> createState() => _ExitWorkDialogState();
}

class _ExitWorkDialogState extends State<ExitWorkDialog> {
  bool _busy = false;
  String? _error;

  Future<void> _exit() async {
    if (_busy) return;
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      final result = await widget.prepareForExit();
      if (!mounted) return;
      if (!result.completed) {
        final reasons = <String>[
          if (result.timedOut) '等待本地任务保存进度超时',
          if (result.failedLabels.isNotEmpty)
            '无法暂停：${result.failedLabels.join('、')}',
        ];
        setState(() {
          _error = reasons.isEmpty ? '部分本地任务尚未完成退出准备' : reasons.join('；');
          _busy = false;
        });
        return;
      }
      Navigator.of(context).pop(true);
    } catch (_) {
      if (!mounted) return;
      setState(() {
        _error = '本地任务退出准备失败。请继续使用 RCH，或稍后重试。';
        _busy = false;
      });
    }
  }

  @override
  Widget build(BuildContext context) {
    final unpreparable = widget.tasks
        .where((task) => !task.canPrepareForExit && !task.resumable)
        .map((task) => task.label)
        .toSet()
        .toList(growable: false);
    final resumable = widget.tasks
        .where((task) => task.resumable && !task.canPrepareForExit)
        .map((task) => task.label)
        .toSet()
        .toList(growable: false);
    final monitorCount = widget.monitorSummary.trackedCount;
    return AlertDialog(
      title: const Text('RCH 仍有任务'),
      content: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 520, maxHeight: 480),
        child: SingleChildScrollView(
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              for (final task in widget.tasks)
                ListTile(
                  contentPadding: EdgeInsets.zero,
                  dense: true,
                  leading: const Icon(Icons.sync),
                  title: Text(
                    task.label,
                    maxLines: 2,
                    overflow: TextOverflow.ellipsis,
                  ),
                  subtitle: Text(task.phase),
                ),
              if (monitorCount > 0) ...[
                const Divider(),
                Text(
                  '另有 $monitorCount 项 RCH 关联的 115 任务待只读核对。退出后本地轮询暂停；115 云端下载继续，下次打开 RCH 后恢复核对。',
                ),
              ],
              if (widget.monitorStatusUnknown) ...[
                if (monitorCount == 0) const Divider(),
                const Text(
                  '无法读取本地 115 监控状态，因此不能确认是否有待核对任务。退出后本地轮询会暂停；已接受的 115 云端下载不受影响，下次打开 RCH 后可继续核对。',
                ),
              ],
              if (resumable.isNotEmpty) ...[
                const SizedBox(height: 12),
                Text('以下任务已有本地检查点，退出后会在下次启动时按各自规则恢复：${resumable.join('、')}。'),
              ],
              if (unpreparable.isNotEmpty) ...[
                const SizedBox(height: 12),
                Text(
                  '以下任务没有安全的暂停检查点，退出时可能中断：${unpreparable.join('、')}。',
                  style: TextStyle(color: Theme.of(context).colorScheme.error),
                ),
              ],
              if (_error != null) ...[
                const SizedBox(height: 12),
                Text(
                  _error!,
                  style: TextStyle(color: Theme.of(context).colorScheme.error),
                ),
              ],
            ],
          ),
        ),
      ),
      actions: [
        TextButton(
          onPressed: _busy ? null : () => Navigator.of(context).pop(false),
          child: const Text('继续使用 RCH'),
        ),
        FilledButton(
          onPressed: _busy ? null : _exit,
          child: _busy
              ? const SizedBox(
                  width: 18,
                  height: 18,
                  child: CircularProgressIndicator(strokeWidth: 2),
                )
              : const Text('退出并暂停可暂停任务'),
        ),
      ],
    );
  }
}
