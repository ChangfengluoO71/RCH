import 'package:app/theme/app_theme.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('light Mica scaffold keeps a translucent light theme surface', () {
    final standardTheme = AppTheme.build(
      brightness: Brightness.light,
      palette: AppTheme.seaGlass,
      font: AppTheme.systemDefault,
    );
    final micaTheme = AppTheme.build(
      brightness: Brightness.light,
      palette: AppTheme.seaGlass,
      font: AppTheme.systemDefault,
      transparentWindowBackdrop: true,
    );
    final expected = standardTheme.colorScheme.surface;
    final actual = micaTheme.scaffoldBackgroundColor;

    expect(actual.alpha, 224);
    expect(actual.red, expected.red);
    expect(actual.green, expected.green);
    expect(actual.blue, expected.blue);
  });
}
