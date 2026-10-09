import 'package:flutter/material.dart';

/// Global app palette and UI font factory.
///
/// Reader page pixels and canvas backgrounds remain controlled by reader
/// settings; this theme only styles RCH's interface and reader chrome.
abstract final class AppTheme {
  static const classic = 'classic';
  static const seaGlass = 'seaGlass';
  static const warmPaper = 'warmPaper';
  static const inkNight = 'inkNight';

  static const systemDefault = 'systemDefault';
  static const systemSerif = 'systemSerif';
  static const wenKaiGbLite = 'wenKaiGbLite';
  static const zhiMangXing = 'zhiMangXing';

  static const wenKaiFontFamily = 'RchWenKaiGbLite';
  static const zhiMangXingFontFamily = 'RchZhiMangXing';

  static String? fontFamilyFor(String choice) => switch (choice) {
    systemSerif => 'serif',
    wenKaiGbLite => wenKaiFontFamily,
    zhiMangXing => zhiMangXingFontFamily,
    _ => null,
  };

  static ThemeData build({
    required Brightness brightness,
    required String palette,
    required String font,
  }) {
    final scheme = _readableScheme(_colorScheme(palette, brightness));
    final base = palette == classic
        ? (brightness == Brightness.dark
              ? ThemeData.dark(useMaterial3: true)
              : ThemeData.light(useMaterial3: true))
        : ThemeData.from(colorScheme: scheme, useMaterial3: true);
    final fontFamily = fontFamilyFor(font);

    return base.copyWith(
      colorScheme: scheme,
      textTheme: base.textTheme.apply(fontFamily: fontFamily),
      primaryTextTheme: base.primaryTextTheme.apply(fontFamily: fontFamily),
      scaffoldBackgroundColor: scheme.surface,
      canvasColor: scheme.surface,
    );
  }

  static ColorScheme _colorScheme(String palette, Brightness brightness) {
    if (palette == classic) {
      return brightness == Brightness.dark
          ? ThemeData.dark(useMaterial3: true).colorScheme
          : ThemeData.light(useMaterial3: true).colorScheme;
    }

    final seed = switch (palette) {
      seaGlass => const Color(0xFF3A7773),
      warmPaper => const Color(0xFF876044),
      inkNight => const Color(0xFF617187),
      _ => const Color(0xFF6750A4),
    };
    var scheme = ColorScheme.fromSeed(
      seedColor: seed,
      brightness: brightness,
      contrastLevel: 1,
    );

    if (palette == inkNight && brightness == Brightness.dark) {
      scheme = scheme.copyWith(
        surface: const Color(0xFF000000),
        surfaceDim: const Color(0xFF000000),
        surfaceBright: const Color(0xFF242424),
        surfaceContainerLowest: const Color(0xFF000000),
        surfaceContainerLow: const Color(0xFF080808),
        surfaceContainer: const Color(0xFF101010),
        surfaceContainerHigh: const Color(0xFF181818),
        surfaceContainerHighest: const Color(0xFF202020),
      );
    }
    return scheme;
  }

  static ColorScheme _readableScheme(ColorScheme scheme) {
    Color safe(Color foreground, Color background) =>
        _ensureContrast(foreground, background);
    final primary = safe(scheme.primary, scheme.surface);
    final secondary = safe(scheme.secondary, scheme.surface);
    final tertiary = safe(scheme.tertiary, scheme.surface);
    final error = safe(scheme.error, scheme.surface);

    return scheme.copyWith(
      primary: primary,
      secondary: secondary,
      tertiary: tertiary,
      error: error,
      onSurface: safe(scheme.onSurface, scheme.surface),
      onSurfaceVariant: safe(scheme.onSurfaceVariant, scheme.surface),
      onPrimary: safe(scheme.onPrimary, primary),
      onPrimaryContainer: safe(
        scheme.onPrimaryContainer,
        scheme.primaryContainer,
      ),
      onSecondary: safe(scheme.onSecondary, secondary),
      onSecondaryContainer: safe(
        scheme.onSecondaryContainer,
        scheme.secondaryContainer,
      ),
      onTertiary: safe(scheme.onTertiary, tertiary),
      onTertiaryContainer: safe(
        scheme.onTertiaryContainer,
        scheme.tertiaryContainer,
      ),
      onError: safe(scheme.onError, error),
      onErrorContainer: safe(scheme.onErrorContainer, scheme.errorContainer),
      onInverseSurface: safe(scheme.onInverseSurface, scheme.inverseSurface),
    );
  }

  static Color _ensureContrast(Color foreground, Color background) {
    if (_contrastRatio(foreground, background) >= 4.5) return foreground;

    final target = background.computeLuminance() > 0.179
        ? const Color(0xFF000000)
        : const Color(0xFFFFFFFF);
    var low = 0.0;
    var high = 1.0;
    for (var i = 0; i < 16; i++) {
      final middle = (low + high) / 2;
      final candidate = Color.lerp(foreground, target, middle)!;
      if (_contrastRatio(candidate, background) >= 4.5) {
        high = middle;
      } else {
        low = middle;
      }
    }
    return Color.lerp(foreground, target, high)!;
  }

  static double _contrastRatio(Color first, Color second) {
    final a = first.computeLuminance();
    final b = second.computeLuminance();
    final brighter = a > b ? a : b;
    final darker = a > b ? b : a;
    return (brighter + 0.05) / (darker + 0.05);
  }
}
