# Technical design: app theme and font settings

## Existing behavior

- `RchApp` rebuilds its `MaterialApp` from `LibraryStore` settings and currently selects `ThemeData.light()` or `ThemeData.dark()` from `AppSettings.themeMode`.
- `AppSettings` is JSON-backed and already persists `themeMode`. Missing JSON fields can be assigned backwards-compatible constructor defaults.
- The Appearance & Layout settings group already contains the light/dark selector and responsive layout options.
- Most pages inherit the global Material theme, but several widgets still use fixed gray/black/white values. These generic UI colors would bypass new palettes and need to use semantic theme roles.

## Settings model and compatibility

Add two independent, persisted settings to `AppSettings`:

- `themePalette`: `classic`, `seaGlass`, `warmPaper`, or `inkNight`.
- `appFont`: `systemDefault`, `systemSerif`, `wenKaiGbLite`, or `zhiMangXing`.

Keep `themeMode` (`light`/`dark`) separate. Defaults for old or unknown values are `classic` and `systemDefault`; preserve the current `dark` default. Store these as local app preferences through the existing settings serialization path. Do not add palette/font values to book metadata or reading-progress synchronization.

## Theme construction

Create one theme factory used by `RchApp` to construct light and dark `ThemeData` from the selected palette and font. It should produce Material 3 `ColorScheme` role sets for each preset and brightness. Palette values are application-owned constants, not system dynamic colors, so the choices are predictable on Android, Windows, and other supported targets.

- **Classic** preserves the current Material light/dark appearance.
- **Sea Glass** uses muted blue-green roles based on the fixed `#3A7773` accent, with distinct light and dark schemes.
- **Warm Paper** uses restrained warm neutral and brown roles based on the fixed `#876044` accent, with distinct light and dark schemes.
- **Ink Night** uses a neutral ink/slate accent based on `#617187`; its dark scheme uses `#000000`/near-black surfaces for OLED displays, and its light scheme uses neutral light surfaces.

Use Material 3 semantic roles for surfaces, text, outlines, primary actions, selection, and inverse content. Audit normal-size text pairs against WCAG AA (4.5:1); do not rely solely on Material's minimum pair contrast. Keep error, warning, success, and other domain-specific status hues semantic, but select contrast-safe shades for each brightness. Avoid globally recoloring the comic page/canvas: reader background remains a separate reader preference, while reader chrome follows the app theme.

The global font choice is applied through the generated `ThemeData`/text theme. System Default uses Flutter's platform default; System Serif uses the platform's generic serif family. Bundle one regular-weight font file for each opt-in calligraphic choice:

- **霞鹜文楷 GB 轻便版** (`wenKaiGbLite`): a readable Kai-style face for simplified Chinese. The upstream Lite edition is intended for app embedding and omits some rare characters; let the platform's fallback font render any missing glyphs.
- **钟齐志莽行书** (`zhiMangXing`): an expressive Xingshu display face. Make clear in the selector preview that it may be less legible in dense, small interface text.

The two raw assets are approximately 13.5 MB and 3.9 MB respectively (about 17 MB total before platform packaging/compression). Keep System Default as the initial selection. Include the full SIL Open Font License 1.1 notice and attribution for each font alongside the assets. Do not fetch fonts over the network. All font choices affect app interface text only, not glyphs already present in comic images. Flutter supports bundling font assets and applying a selected family through the app theme.

## UI placement

Extend the existing Appearance & Layout settings section rather than adding another global settings surface. Keep brightness, palette, and font clearly labeled as separate controls. Palette choices should include small color previews and wrap/reflow within available width; font choice should use a labeled selector with a sample preview in each option. Preserve comfortable spacing on narrow screens and the current wider desktop layout.

## Global color migration

Audit app UI for fixed black/white/gray foregrounds and generic surfaces/borders that make pages ignore the palette. Replace generic styling with `ColorScheme` and `TextTheme` roles. Keep deliberate semantic/status colors, image overlays, QR-code colors, and comic canvas colors only where their fixed color is part of the control's meaning or rendering contract. All routes use the same `MaterialApp` theme, so recent reading, statistics, tags, sources, and the reader's app chrome update together.

## Validation expectations for the implementation phase

- Verify JSON round-tripping, defaults for legacy settings, and unknown-value fallback.
- Check that every mode/palette pair has readable semantic foreground/background roles, including Ink Night's near-black surfaces.
- Review the Appearance settings at compact and desktop widths.
- Run the project's applicable static quality checks before claiming completion. Do not alter the separate in-progress wide-page changes while implementing this task.

## References

- [Material Design 3 color roles](https://m3.material.io/styles/color/the-color-system)
- [WCAG 2.1 contrast minimum](https://www.w3.org/WAI/WCAG21/Understanding/contrast-minimum)
- [Flutter `ThemeData.from`](https://api.flutter.dev/flutter/material/ThemeData/ThemeData.from.html)
- [Flutter custom fonts](https://docs.flutter.dev/cookbook/design/fonts)
- [LXGW WenKai GB Lite project and license](https://github.com/lxgw/LxgwWenKaiGB-Lite)
- [LXGW WenKai GB Lite font file](https://github.com/lxgw/LxgwWenKaiGB-Lite/blob/main/fonts/TTF/LXGWWenKaiGBLite-Regular.ttf)
- [Zhi Mang Xing project and license](https://github.com/googlefonts/zhimangxing)
- [Zhi Mang Xing font package metadata](https://www.npmjs.com/package/@fontpkg/zhi-mang-xing)
- [Android `Typeface` families](https://developer.android.com/reference/android/graphics/Typeface)
- [Mihon reader canvas/background options](https://mihon.app/docs/guides/reader-settings)
