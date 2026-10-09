# Implementation Plan

**Goal:** Add four readable global color palettes and four app font choices across RCH mobile and desktop, while preserving the current dark Classic appearance as the default.

**Architecture:** Persist `themePalette` and `appFont` in `AppSettings`. Build light/dark Material 3 themes from the selected palette in a shared theme factory and apply the chosen font through `ThemeData`. Keep comic canvas rendering independent. Replace generic fixed UI colors with semantic theme roles. Bundle WenKai GB Lite and Zhi Mang Xing with their OFL notices.

**Tech Stack:** Flutter Material 3, Dart JSON settings, `pubspec.yaml` font assets.

## Tasks

### 1. Persist appearance preferences and add a shared theme factory

**Files:** `app/lib/store/models.dart`, `app/lib/main.dart`, new `app/lib/theme/app_theme.dart`

- Add `themePalette` and `appFont` settings with serialization, legacy defaults, and safe fallback for unknown persisted values.
- Keep the existing `themeMode` behavior and default (`dark`).
- Implement Classic, Sea Glass, Warm Paper, and Ink Night light/dark schemes with accessible semantic roles; preserve `ThemeData.light/dark` appearance for Classic.
- Apply the system default, system serif, WenKai, and Xingshu families from the same theme factory.

### 2. Add offline Chinese calligraphic font assets

**Files:** `app/pubspec.yaml`, new font assets and license/attribution notice under `app/assets/fonts/`

- Add the regular WenKai GB Lite and Zhi Mang Xing font files from the referenced upstream projects.
- Include each font's SIL Open Font License 1.1 text and required attribution.
- Declare the font families in Flutter assets; keep System Default as the initial choice.

### 3. Add appearance controls to Settings

**Files:** `app/lib/ui/home_page.dart`

- Keep brightness separate from palette and font.
- Add the four palette previews and four font options, including sample text for the Kai/Xingshu choices and a readability hint for Xingshu.
- Ensure selectors wrap or reflow across narrow phone and desktop widths without clipping or crowding.
- Persist selections through the existing `LibraryStore` settings update path and apply them immediately.

### 4. Migrate generic UI colors to theme roles

**Files:** affected widgets under `app/lib/ui/`, prioritizing `backup_panel.dart`, `ai_floating_progress.dart`, `book_detail_page.dart`, `comic_cover.dart`, `library_page.dart`, `source_browser.dart`, `sync_panel.dart`, and `webdav_page.dart`.

- Replace generic fixed black/white/gray text, surface, border, and selection colors with `ColorScheme`/`TextTheme` roles.
- Preserve colors whose fixed value carries meaning or is part of image/QR rendering, including semantic status colors and the reader canvas.
- Review book cards, recent reading, statistics, tags, source browsing, settings, and reader chrome for global palette/font coverage.

### 5. Review and validate the implementation

- Run `flutter analyze --no-pub` from `app/` and resolve findings introduced by this task.
- Run `git diff --check` and review all theme/font diffs.
- Confirm no unrelated wide-page-splitting edits are staged or committed.
- Do not add or run tests in this task unless the user separately asks for tests or implementation verification.
