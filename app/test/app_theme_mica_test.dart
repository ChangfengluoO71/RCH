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

    final actualArgb = actual.toARGB32();
    final expectedArgb = expected.toARGB32();
    expect((actualArgb >> 24) & 0xff, 224);
    expect(actualArgb & 0x00ffffff, expectedArgb & 0x00ffffff);
  });
}
