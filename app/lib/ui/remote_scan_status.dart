import 'package:app/store/remote_scan_models.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';

class RemoteScanStatusPanel extends StatelessWidget {
  const RemoteScanStatusPanel({
    super.key,
    required this.sourceName,
    required this.stateListenable,
    this.onPause,
    this.onResume,
    this.onRetry,
    this.onRescanIncremental,
    this.onRescanFull,
    this.compact = false,
  });

  final String sourceName;
  final ValueListenable<RemoteScanViewState?> stateListenable;
  final Future<void> Function()? onPause;
  final Future<void> Function()? onResume;
  final Future<void> Function()? onRetry;
  final Future<void> Function()? onRescanIncremental;
  final Future<void> Function()? onRescanFull;
  final bool compact;

  @override
  Widget build(BuildContext context) {
    return ValueListenableBuilder<RemoteScanViewState?>(
      valueListenable: stateListenable,
      builder: (context, state, _) {
        if (state == null) return const SizedBox.shrink();
        final paused = state.status == 'paused';
        final progress = state.total <= 0
            ? '${state.processed}'
            : '${state.processed}/${state.total}';
        final message = _messageFor(state);
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
                  child: Text(
                    '$sourceName scan: ${state.status} ${state.mode} $progress'
                    '${message == null ? '' : ' - $message'}',
                    maxLines: compact ? 1 : 2,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(fontSize: compact ? 11 : 12),
                  ),
                ),
                if (paused && onResume != null)
                  _iconButton(
                    icon: Icons.play_arrow,
                    tooltip: 'Resume remote scan',
                    onPressed: onResume!,
                  )
                else if (!paused && onPause != null)
                  _iconButton(
                    icon: Icons.pause,
                    tooltip: 'Pause remote scan',
                    onPressed: onPause!,
                  ),
                if (_canRetry(state.status) && onRetry != null)
                  _iconButton(
                    icon: Icons.refresh,
                    tooltip: 'Retry remote scan',
                    onPressed: onRetry!,
                  ),
                if (onRescanIncremental != null)
                  _iconButton(
                    icon: Icons.update,
                    tooltip: 'Incremental rescan',
                    onPressed: onRescanIncremental!,
                  ),
                if (onRescanFull != null)
                  _iconButton(
                    icon: Icons.restart_alt,
                    tooltip: 'Full rescan',
                    onPressed: onRescanFull!,
                  ),
              ],
            ),
          ),
        );
      },
    );
  }

  static Widget _iconButton({
    required IconData icon,
    required String tooltip,
    required Future<void> Function() onPressed,
  }) {
    return IconButton(
      icon: Icon(icon, size: 18),
      tooltip: tooltip,
      visualDensity: VisualDensity.compact,
      onPressed: () => onPressed(),
    );
  }

  static IconData _iconFor(String status) => switch (status) {
    'complete' => Icons.check_circle_outline,
    'paused' => Icons.pause_circle_outline,
    'degraded' || 'rangeUnavailable' => Icons.warning_amber_outlined,
    'queued' => Icons.schedule,
    _ => Icons.sync,
  };

  static bool _canRetry(String status) =>
      status == 'degraded' ||
      status == 'rangeUnavailable' ||
      status == 'failed' ||
      status == 'cancelled';

  static String? _messageFor(RemoteScanViewState state) {
    if (state.coverFetchPaused) return 'cover fetch paused';
    final code = state.errorCode;
    if (code == null || code.isEmpty) return null;
    final lower = code.toLowerCase();
    if (lower.contains('rangeunavailable')) return 'Range unavailable';
    if (lower.contains('authexpired')) return 'Authorization expired';
    if (lower.contains('forbidden')) return 'Permission denied';
    return _redact(code);
  }

  static String _redact(String value) {
    final withoutUrls = value.replaceAll(
      RegExp(r'https?://\S+', caseSensitive: false),
      '[redacted-url]',
    );
    return withoutUrls
        .replaceAll(
          RegExp(
            r'(authorization|cookie|token|password|secret)\s*[:=]\s*[^\s,;]+',
            caseSensitive: false,
          ),
          r'$1=[redacted]',
        )
        .replaceAll(
          RegExp(r'Bearer\s+[^\s,;]+', caseSensitive: false),
          'Bearer [redacted]',
        );
  }
}
