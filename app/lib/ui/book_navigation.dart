import 'package:app/store/library_store.dart';
import 'package:app/ui/common.dart';
import 'package:app/ui/rch_overlay.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';

/// Applies the tap preference on every platform while preserving each
/// surface's default behavior when the preference is disabled.
VoidCallback comicTapHandler(
  BuildContext context, {
  required bool canRead,
  required VoidCallback onRead,
  required VoidCallback onDetails,
  required VoidCallback onDesktopTap,
}) {
  if (canRead && LibraryStore.instance.settings.tapComicFileWithoutDetails) {
    return onRead;
  }
  if (!isCompact(context)) return onDesktopTap;
  return onDetails;
}

/// Adds the desktop right-click action for opening a comic's detail page.
Widget comicDetailContextMenu(
  BuildContext context, {
  required Widget child,
  required VoidCallback onDetails,
}) {
  if (!_supportsDesktopContextMenu) return child;
  return GestureDetector(
    behavior: HitTestBehavior.deferToChild,
    onSecondaryTapUp: (details) =>
        _showComicDetailContextMenu(context, details.globalPosition, onDetails),
    child: child,
  );
}

bool get _supportsDesktopContextMenu =>
    !kIsWeb &&
    (defaultTargetPlatform == TargetPlatform.windows ||
        defaultTargetPlatform == TargetPlatform.macOS ||
        defaultTargetPlatform == TargetPlatform.linux);

Future<void> _showComicDetailContextMenu(
  BuildContext context,
  Offset globalPosition,
  VoidCallback onDetails,
) async {
  final overlay = Overlay.of(context).context.findRenderObject() as RenderBox;
  final localPosition = overlay.globalToLocal(globalPosition);
  final selected = await showMenu<String>(
    context: context,
    position: RelativeRect.fromRect(
      Rect.fromLTWH(localPosition.dx, localPosition.dy, 0, 0),
      Offset.zero & overlay.size,
    ),
    items: const [PopupMenuItem(value: 'details', child: Text('进入漫画详细页'))],
  );
  if (selected == 'details' && context.mounted) onDetails();
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
