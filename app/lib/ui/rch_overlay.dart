import 'dart:ui' show ImageFilter;

import 'package:app/theme/overlay_material_scope.dart';
import 'package:app/theme/window_material_scope.dart';
import 'package:app/store/library_store.dart';
import 'package:app/ui/common.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';

Future<T?> showRchDialog<T>({
  required BuildContext context,
  required WidgetBuilder builder,
  bool barrierDismissible = true,
  bool useRootNavigator = true,
}) {
  if (!_shouldUseAcrylic(context)) {
    return showDialog<T>(
      context: context,
      builder: builder,
      barrierDismissible: barrierDismissible,
      useRootNavigator: useRootNavigator,
    );
  }

  final barrierColor = Theme.of(
    context,
  ).colorScheme.scrim.withValues(alpha: 0.18);
  return showDialog<T>(
    context: context,
    barrierDismissible: barrierDismissible,
    useRootNavigator: useRootNavigator,
    barrierColor: barrierColor,
    builder: (dialogContext) => _FrostedDialog(builder: builder),
  );
}

Future<T?> showRchModalBottomSheet<T>({
  required BuildContext context,
  required WidgetBuilder builder,
  bool isScrollControlled = false,
  bool isDismissible = true,
  bool enableDrag = true,
  bool useRootNavigator = false,
  bool useSafeArea = false,
}) {
  if (!_shouldUseAcrylic(context)) {
    return showModalBottomSheet<T>(
      context: context,
      builder: builder,
      isScrollControlled: isScrollControlled,
      isDismissible: isDismissible,
      enableDrag: enableDrag,
      useRootNavigator: useRootNavigator,
      useSafeArea: useSafeArea,
    );
  }

  const borderRadius = BorderRadius.vertical(top: Radius.circular(28));
  return showModalBottomSheet<T>(
    context: context,
    isScrollControlled: isScrollControlled,
    isDismissible: isDismissible,
    enableDrag: enableDrag,
    useRootNavigator: useRootNavigator,
    useSafeArea: useSafeArea,
    backgroundColor: Colors.transparent,
    barrierColor: Theme.of(context).colorScheme.scrim.withValues(alpha: 0.18),
    elevation: 0,
    shape: const RoundedRectangleBorder(borderRadius: borderRadius),
    clipBehavior: Clip.antiAlias,
    builder: (sheetContext) => _AcrylicSurface(
      borderRadius: borderRadius,
      child: Builder(builder: builder),
    ),
  );
}

bool _shouldUseAcrylic(BuildContext context) {
  final media = MediaQuery.maybeOf(context);
  if (media?.highContrast == true) return false;
  if (isAndroidPlatform) {
    return !appOrientationCoordinator.readerActive &&
        media?.disableAnimations != true &&
        resolveLayoutProfile(context).tablet &&
        LibraryStore.instance.settings.tabletOverlayMaterial == 'frosted';
  }
  final selected =
      OverlayMaterialScope.maybeOf(context)?.acrylicSelected ?? false;
  final supported =
      WindowMaterialScope.maybeOf(context)?.micaSupported ?? false;
  return defaultTargetPlatform == TargetPlatform.windows &&
      selected &&
      supported;
}

/// Dialog widgets occupy the whole route even though their Material surface is
/// much smaller. Locate that surface after layout and clip only its backdrop.
/// Unknown dialog implementations keep their normal, opaque Flutter surface.
class _FrostedDialog extends StatefulWidget {
  const _FrostedDialog({required this.builder});

  final WidgetBuilder builder;

  @override
  State<_FrostedDialog> createState() => _FrostedDialogState();
}

class _FrostedDialogState extends State<_FrostedDialog> {
  final _stackKey = GlobalKey();
  final _contentKey = GlobalKey();
  Rect? _surfaceRect;
  bool _measureScheduled = false;

  void _scheduleMeasure() {
    if (_measureScheduled) return;
    _measureScheduled = true;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      _measureScheduled = false;
      if (!mounted) return;
      final stack = _stackKey.currentContext?.findRenderObject();
      final root = _contentKey.currentContext;
      if (stack is! RenderBox || root == null || !stack.hasSize) return;
      Element? dialog;
      void findDialog(Element element) {
        if (dialog != null) return;
        if (element.widget is Dialog) {
          dialog = element;
          return;
        }
        element.visitChildElements(findDialog);
      }

      root.visitChildElements(findDialog);
      Element? surface;
      void findSurface(Element element) {
        if (surface != null) return;
        if (element.widget is Material) {
          surface = element;
          return;
        }
        element.visitChildElements(findSurface);
      }

      dialog?.visitChildElements(findSurface);
      final box = surface?.findRenderObject();
      Rect? next;
      if (box is RenderBox && box.hasSize && box.attached) {
        next = box.localToGlobal(Offset.zero, ancestor: stack) & box.size;
      }
      if (next != _surfaceRect) setState(() => _surfaceRect = next);
    });
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final rect = _surfaceRect;
    _scheduleMeasure();
    return _DialogLayoutObserver(
      onLayout: _scheduleMeasure,
      child: Stack(
        key: _stackKey,
        children: [
          if (rect != null)
            Positioned.fromRect(
              rect: rect,
              child: _AcrylicSurface(
                borderRadius: BorderRadius.circular(28),
                child: const SizedBox.expand(),
              ),
            ),
          Positioned.fill(
            child: KeyedSubtree(
              key: _contentKey,
              child: Theme(
                data: rect == null
                    ? theme
                    : theme.copyWith(
                        dialogTheme: theme.dialogTheme.copyWith(
                          backgroundColor: Colors.transparent,
                          surfaceTintColor: Colors.transparent,
                          shadowColor: Colors.transparent,
                        ),
                      ),
                child: Builder(builder: widget.builder),
              ),
            ),
          ),
        ],
      ),
    );
  }
}

class _DialogLayoutObserver extends SingleChildRenderObjectWidget {
  const _DialogLayoutObserver({required this.onLayout, required super.child});

  final VoidCallback onLayout;

  @override
  RenderObject createRenderObject(BuildContext context) =>
      _DialogLayoutRenderObject(onLayout);

  @override
  void updateRenderObject(
    BuildContext context,
    _DialogLayoutRenderObject renderObject,
  ) => renderObject.onLayout = onLayout;
}

class _DialogLayoutRenderObject extends RenderProxyBox {
  _DialogLayoutRenderObject(this.onLayout);

  VoidCallback onLayout;

  @override
  void performLayout() {
    super.performLayout();
    onLayout();
  }

  @override
  void paint(PaintingContext context, Offset offset) {
    super.paint(context, offset);
    // A dialog may relayout inside its own boundary without resizing the route.
    onLayout();
  }
}

class _AcrylicSurface extends StatelessWidget {
  const _AcrylicSurface({required this.borderRadius, required this.child});

  final BorderRadius borderRadius;
  final Widget child;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    return ClipRRect(
      borderRadius: borderRadius,
      child: BackdropFilter(
        filter: ImageFilter.blur(sigmaX: 18, sigmaY: 18),
        child: DecoratedBox(
          decoration: BoxDecoration(
            color: scheme.surface.withValues(alpha: 0.78),
            borderRadius: borderRadius,
            border: Border.all(
              color: scheme.outlineVariant.withValues(alpha: 0.55),
            ),
          ),
          child: child,
        ),
      ),
    );
  }
}
