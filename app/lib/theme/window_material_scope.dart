import 'package:flutter/widgets.dart';

/// Exposes native window-material support to settings and overlay surfaces.
class WindowMaterialScope extends InheritedWidget {
  const WindowMaterialScope({
    super.key,
    required this.micaSupported,
    required super.child,
  });

  final bool micaSupported;

  static WindowMaterialScope? maybeOf(BuildContext context) =>
      context.dependOnInheritedWidgetOfExactType<WindowMaterialScope>();

  @override
  bool updateShouldNotify(WindowMaterialScope oldWidget) =>
      micaSupported != oldWidget.micaSupported;
}
