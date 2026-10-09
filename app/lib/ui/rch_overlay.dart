import 'dart:ui' show ImageFilter;

import 'package:app/theme/overlay_material_scope.dart';
import 'package:app/theme/window_material_scope.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';

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

  return showDialog<T>(
    context: context,
    barrierDismissible: barrierDismissible,
    useRootNavigator: useRootNavigator,
    builder: (dialogContext) {
      final theme = Theme.of(dialogContext);
      return Theme(
        data: theme.copyWith(
          dialogTheme: theme.dialogTheme.copyWith(
            backgroundColor: Colors.transparent,
            surfaceTintColor: Colors.transparent,
            shadowColor: Colors.transparent,
          ),
        ),
        child: _AcrylicSurface(
          borderRadius: BorderRadius.circular(28),
          child: Builder(builder: builder),
        ),
      );
    },
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
  final selected =
      OverlayMaterialScope.maybeOf(context)?.acrylicSelected ?? false;
  final supported =
      WindowMaterialScope.maybeOf(context)?.micaSupported ?? false;
  final highContrast = MediaQuery.maybeOf(context)?.highContrast ?? false;
  return defaultTargetPlatform == TargetPlatform.windows &&
      selected &&
      supported &&
      !highContrast;
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
            color: scheme.surface.withValues(alpha: 0.96),
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
