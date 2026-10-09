import 'package:flutter/widgets.dart';

/// Reports whether Android supplied a wallpaper-derived color scheme.
class SystemDynamicColorScope extends InheritedWidget {
  const SystemDynamicColorScope({
    super.key,
    required this.dynamicColorsAvailable,
    required super.child,
  });

  final bool dynamicColorsAvailable;

  static SystemDynamicColorScope? maybeOf(BuildContext context) =>
      context.dependOnInheritedWidgetOfExactType<SystemDynamicColorScope>();

  @override
  bool updateShouldNotify(SystemDynamicColorScope oldWidget) =>
      dynamicColorsAvailable != oldWidget.dynamicColorsAvailable;
}
