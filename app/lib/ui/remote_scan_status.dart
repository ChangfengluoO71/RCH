import 'dart:async';

import 'package:app/store/remote_scan_models.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';

class RemoteScanStatusPanel extends StatelessWidget {
  const RemoteScanStatusPanel({
    super.key,
    required this.sourceName,
    required this.stateListenable,
    this.idleMessage = '等待扫描',
    this.onRetry,
    this.compact = false,
  });

  final String sourceName;
  final ValueListenable<RemoteScanViewState?> stateListenable;
  final String idleMessage;
  final Future<void> Function()? onRetry;
  final bool compact;

  @override
  Widget build(BuildContext context) {
    return ValueListenableBuilder<RemoteScanViewState?>(
      valueListenable: stateListenable,
      builder: (context, state, _) {
        if (state == null) return _idlePanel();
        final normalizedStatus = state.status.trim().toLowerCase();
        final discovering = state.total <= 0 && _canPause(state.status);
        final hasDetailedProgress =
            state.directoriesChecked > 0 ||
            state.discoveredBooks > 0 ||
            state.readyBooks > 0 ||
            state.activeBooks > 0 ||
            state.pendingBooks > 0 ||
            state.retryBooks > 0 ||
            state.blockedBooks > 0 ||
            state.unsupportedBooks > 0 ||
            state.failedBooks > 0;
        final progress = hasDetailedProgress
            ? _detailedProgress(state)
            : state.total > 0
            ? '漫画文件扫描：已完成 ${state.processed} 本 / 已发现 ${state.total} 本'
            : discovering
            ? '漫画文件扫描：已完成 ${state.processed} 本，正在发现更多漫画'
            : normalizedStatus == 'complete' || normalizedStatus == 'completed'
            ? '漫画文件扫描：已完成 ${state.processed} 本，未发现更多漫画'
            : '漫画文件扫描：已完成 ${state.processed} 本，全量扫描尚未完成';
        final message = _messageFor(state);
        final statusLabel = _statusLabel(state.status);
        final modeLabel = _modeLabel(state.mode);
        return Material(
          color: Colors.blueGrey.withAlpha(32),
          child: Padding(
            padding: compact
                ? const EdgeInsets.symmetric(horizontal: 8, vertical: 4)
                : const EdgeInsets.all(8),
            child: Row(
              children: [
                Icon(_iconFor(state.status), size: compact ? 16 : 20),
                const SizedBox(width: 8),
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(
                        '$sourceName：$statusLabel（$modeLabel）',
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(fontSize: compact ? 11 : 12),
                      ),
                      Text(
                        '$progress${message == null ? '' : ' · $message'}',
                        maxLines: 2,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(fontSize: compact ? 11 : 12),
                      ),
                    ],
                  ),
                ),
                // 2026-09-21（用户要求）：扫描是自动运行的 ⇒ 移除"暂停/继续"按钮。
                if (_canRetryState(state) && onRetry != null)
                  _iconButton(
                    context: context,
                    icon: Icons.refresh,
                    tooltip: '重试远程扫描',
                    onPressed: onRetry!,
                    failureMessage: '重新扫描失败，请稍后重试',
                  ),
                // 2026-09-21（用户要求）：移除"增量/全量重新扫描"按钮（扫描自动运行）。
              ],
            ),
          ),
        );
      },
    );
  }

  static String _detailedProgress(RemoteScanViewState state) {
    final listing = state.discoveryComplete
        ? '目录：已检查 ${state.directoriesChecked} 个，已发现 ${state.discoveredBooks} 本'
        : '目录：已检查 ${state.directoriesChecked} 个，已发现 ${state.discoveredBooks} 本，仍在发现';
    // P1-F：主文案以 **available / discovered** 为准（source scope，不新增 folder progress）。
    final cover =
        '封面：可用 ${state.availableBooks} / 共 ${state.discoveredBooks} 本，'
        '等待 ${state.waitingBooks}，进行中 ${state.activeBooks}，'
        '失败 ${state.failedBooks}，暂不支持 ${state.unsupportedBooks}，'
        '暂不可用 ${state.blockedBooks}';
    return '$listing\n$cover';
  }

  Widget _idlePanel() {
    return Material(
      color: Colors.blueGrey.withAlpha(24),
      child: Padding(
        padding: compact
            ? const EdgeInsets.symmetric(horizontal: 8, vertical: 4)
            : const EdgeInsets.all(8),
        child: Row(
          children: [
            Icon(Icons.schedule, size: compact ? 16 : 20),
            const SizedBox(width: 8),
            Expanded(
              child: Row(
                children: [
                  Flexible(
                    child: Text(
                      '$sourceName：',
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(fontSize: compact ? 11 : 12),
                    ),
                  ),
                  Flexible(
                    child: Text(
                      idleMessage,
                      maxLines: compact ? 1 : 2,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(fontSize: compact ? 11 : 12),
                    ),
                  ),
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }

  static Widget _iconButton({
    required BuildContext context,
    required IconData icon,
    required String tooltip,
    required Future<void> Function() onPressed,
    required String failureMessage,
  }) {
    return IconButton(
      icon: Icon(icon, size: 18),
      tooltip: tooltip,
      visualDensity: VisualDensity.compact,
      onPressed: () =>
          unawaited(_runAction(context, onPressed, failureMessage)),
    );
  }

  static Future<void> _runAction(
    BuildContext context,
    Future<void> Function() action,
    String failureMessage,
  ) async {
    try {
      await action();
    } catch (_) {
      if (!context.mounted) return;
      final messenger = ScaffoldMessenger.maybeOf(context);
      messenger
        ?..hideCurrentSnackBar()
        ..showSnackBar(SnackBar(content: Text(failureMessage)));
    }
  }

  static IconData _iconFor(String status) {
    final normalized = status.trim().toLowerCase().replaceAll(
      RegExp(r'[^a-z0-9]+'),
      '',
    );
    return switch (normalized) {
      'complete' || 'completed' || 'succeeded' => Icons.check_circle_outline,
      'paused' => Icons.pause_circle_outline,
      'degraded' || 'rangeunavailable' => Icons.warning_amber_outlined,
      'failed' || 'cancelled' || 'canceled' => Icons.error_outline,
      'queued' => Icons.schedule,
      _ => Icons.sync,
    };
  }

  static bool _canPause(String status) {
    final normalized = status.trim().toLowerCase();
    return normalized == 'queued' || normalized == 'running';
  }

  static String _statusLabel(String status) {
    final normalized = status.trim().toLowerCase();
    return const {
          'queued': '排队中',
          'running': '扫描中',
          'complete': '已完成',
          'completed': '已完成',
          'succeeded': '已完成',
          'paused': '已暂停',
          'degraded': '部分完成',
          'rangeunavailable': 'Range 不可用',
          'range_unavailable': 'Range 不可用',
          'failed': '失败',
          'cancelled': '已取消',
          'canceled': '已取消',
        }[normalized] ??
        '未知状态';
  }

  static String _modeLabel(String mode) {
    final normalized = mode.trim().toLowerCase();
    return const {
          'full': '全量扫描',
          'snapshot': '全量扫描',
          'incremental': '增量扫描',
        }[normalized] ??
        '未知模式';
  }

  static bool _canRetry(String status) {
    final normalized = status.trim().toLowerCase().replaceAll(
      RegExp(r'[^a-z0-9]+'),
      '',
    );
    return normalized == 'degraded' ||
        normalized == 'rangeunavailable' ||
        normalized == 'failed' ||
        normalized == 'cancelled' ||
        normalized == 'canceled';
  }

  /// A listing may complete while one or more cover tasks are unavailable.
  /// Keep a visible one-tap retry for that terminal generation as well; a
  /// plain `complete` without a cover error does not need the extra button.
  static bool _canRetryState(RemoteScanViewState state) {
    if (_canRetry(state.status)) return true;
    final normalizedStatus = state.status.trim().toLowerCase();
    if (normalizedStatus != 'complete' &&
        normalizedStatus != 'completed' &&
        normalizedStatus != 'succeeded') {
      return false;
    }
    final code = state.errorCode?.toLowerCase() ?? '';
    return code.contains('cover') || code.contains('rangeunavailable');
  }

  static String? _messageFor(RemoteScanViewState state) {
    if (state.coverFetchPaused) return '远程封面联网获取已暂停';
    final code = state.errorCode;
    if (code == null || code.isEmpty) return null;
    final lower = code.toLowerCase().replaceAll(RegExp(r'[^a-z0-9]+'), '');
    const known = <String, String>{
      'rangeunavailable': 'Range 不可用',
      'authexpired': '授权已过期',
      'unauthorized': '需要重新授权',
      'forbidden': '没有访问权限',
      'notfound': '远程资源不存在',
      'ratelimited': '请求过于频繁，请稍后重试',
      'transient': '网络暂时不可用',
      'malformed': '远程响应格式错误',
      'cancelled': '已取消',
      'canceled': '已取消',
      'unsupported': '暂不支持此远程操作',
      'storage': '本地存储失败',
      'coverstorage': '封面缓存写入失败',
      'coverunavailable': '封面暂时无法获取，已使用占位图',
      'coverfetchpaused': '远程封面联网获取已暂停',
      'fullscanrequired': '尚未完成全量扫描，将先执行全量扫描',
      'provider': '远程服务返回错误',
      'nativestartfailed': '扫描启动失败',
      // RG-B：启动被原生拒绝时的**可操作**文案（枚举码由协调器给出，不含原生文本）。
      'scanstartrejected:epoch': '上次扫描未正常结束，已自动重置，请重新扫描',
      'scanstartrejected:session': '远程会话已失效，请重新连接该源后重试',
      'scanstartrejected:root': '书源根目录已变更，请检查书源配置',
      'scanstartrejected:proof': '书源身份信息缺失，请重新添加书源',
      'scanstartrejected:recovery': '扫描状态恢复失败，请稍后重试',
      'scanstartrejected:state': '扫描状态读取失败，请稍后重试',
      'scanstartrejected:baseline': '扫描基线读取失败，请稍后重试',
      'scanstartrejected:mode': '扫描模式不被支持',
      'scanstartrejected:localonly': '该类型书源不支持远程扫描',
      'scanstartrejected:unknown': '扫描启动失败，请重试',
      'interrupted': '上次扫描被中断（应用未正常退出），请重新扫描',
      'sourcechanged': '书源配置已变更',
      'truncated': '远程响应不完整',
      'paused': '扫描已暂停',
    };
    // The setting listener updates `coverFetchPaused` immediately. Once the
    // user turns networking back on, do not keep displaying the old terminal
    // generation's paused hint until the next scan runs.
    if (lower.contains('coverfetchpaused')) return null;
    for (final entry in known.entries) {
      if (lower.contains(entry.key)) return entry.value;
    }
    return '远程扫描失败';
  }
}
