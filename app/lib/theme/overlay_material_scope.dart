import 'package:flutter/widgets.dart';

/// Exposes the locally selected overlay surface to shared modal presenters.
class OverlayMaterialScope extends InheritedWidget {
  const OverlayMaterialScope({
    super.key,
    required this.acrylicSelected,
    required super.child,
  });

  final bool acrylicSelected;

  static OverlayMaterialScope? maybeOf(BuildContext context) =>
      context.dependOnInheritedWidgetOfExactType<OverlayMaterialScope>();

  @override
  bool updateShouldNotify(OverlayMaterialScope oldWidget) =>
      acrylicSelected != oldWidget.acrylicSelected;
}
