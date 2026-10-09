import 'package:app/store/library_store.dart';
import 'package:app/ui/common.dart';
import 'package:app/ui/rch_overlay.dart';
import 'package:flutter/material.dart';

/// Applies the phone tap preference while letting each desktop surface retain
/// its original tap callback.
VoidCallback comicTapHandler(
  BuildContext context, {
  required bool canRead,
  required VoidCallback onRead,
  required VoidCallback onDetails,
  required VoidCallback onDesktopTap,
}) {
  if (!isCompact(context)) return onDesktopTap;
  if (canRead && LibraryStore.instance.settings.tapComicFileWithoutDetails) {
    return onRead;
  }
  return onDetails;
}

/// Long-pressing a phone comic asks before opening its detail page.
Future<void> showComicDetailPrompt(
  BuildContext context, {
  required VoidCallback onDetails,
}) async {
  if (!isCompact(context)) return;
  final enterDetails = await showRchDialog<bool>(
    context: context,
    builder: (dialogContext) => AlertDialog(
      title: const Text('是否进入漫画详细页？'),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(dialogContext).pop(false),
          child: const Text('取消'),
        ),
        FilledButton(
          onPressed: () => Navigator.of(dialogContext).pop(true),
          child: const Text('进入详情'),
        ),
      ],
    ),
  );
  if (enterDetails == true && context.mounted) onDetails();
}
