# RCH Android Dynamic Color Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let Android 12+ users opt into wallpaper-derived Material colors while retaining their selected RCH palette as a fallback.

**Architecture:** Use the Material Foundation `dynamic_color` package to receive light and dark schemes. Keep fixed palettes as the default source, apply dynamic schemes only on Android when enabled and available, and expose availability to the appearance settings through a small inherited scope.

**Tech Stack:** Flutter Material 3, `dynamic_color: ^2.1.0`, Dart JSON settings.

**Spec:** [Appearance and materials design](../specs/2026-10-09-rch-appearance-materials-design.md)

## Global Constraints

- Preserve the existing four palettes and four fonts.
- Keep dynamic colors opt-in and default it to `false`.
- Keep settings in local `AppSettings`; do not add them to book metadata or sync rows.
- Use dynamic wallpaper colors only on Android 12+; ignore dynamic-color values reported for other platforms.
- If dynamic colors are absent, use the saved RCH palette without modifying that saved palette.
- Keep normal-size text contrast at or above 4.5:1.

## Review Focus

- Android API 31+ with the dynamic setting enabled uses platform light/dark schemes.
- Android API 30 and lower retain the fixed RCH scheme and do not expose a usable dynamic option.
- A null dynamic scheme falls back cleanly without clearing `themePalette`.
- `ThemeMode.system` selects the dynamic scheme matching current system brightness.
- Switching dynamic colors does not reset the user-selected font or fixed fallback palette.

---

### Task 1: Persist the dynamic-color preference and install the package

**Files:**
- Modify: `app/pubspec.yaml`
- Modify: `app/pubspec.lock`
- Modify: `app/lib/store/models.dart`

**Interfaces:**
- Produces: `AppSettings.useSystemDynamicColors`, default `false`, serialized as `useSystemDynamicColors`.

- [x] **Step 1: Add the compatible package dependency**

Add `dynamic_color: ^2.1.0` and resolve the lockfile using the repository's configured Flutter SDK.

- [x] **Step 2: Add the persisted preference**

Add the boolean field, constructor default, JSON write, and null-safe read. Unknown/missing values resolve to `false`.

- [x] **Step 3: Run Flutter static analysis**

Run from `app/`: `flutter analyze --no-pub`.

Expected: exit code 0.

### Task 2: Select the Android dynamic light and dark schemes

**Files:**
- Create: `app/lib/theme/system_dynamic_color_scope.dart`
- Modify: `app/lib/theme/app_theme.dart`
- Modify: `app/lib/main.dart`

**Interfaces:**
- `AppTheme.build` consumes optional `ColorScheme? dynamicColorScheme` and uses it before fixed-palette construction when provided.
- `SystemDynamicColorScope` exposes `dynamicColorsAvailable` to descendants of `MaterialApp`.
- `RchApp` reports availability whenever Android returns a dynamic scheme; it passes a scheme to `AppTheme` only when `defaultTargetPlatform == TargetPlatform.android` and `settings.useSystemDynamicColors` is true.

- [x] **Step 1: Add the availability scope**

Create an immutable `InheritedWidget` with `dynamicColorsAvailable` and a `maybeOf(BuildContext)` accessor.

- [x] **Step 2: Extend the shared theme factory**

Allow the caller to pass a dynamic scheme. Pass it through the existing readability adjustment; keep fixed palettes unchanged when the argument is null.

- [x] **Step 3: Wrap MaterialApp construction in DynamicColorBuilder**

Always let `DynamicColorBuilder` report capability. Set `dynamicColorsAvailable` when running on Android and at least one scheme is non-null. Pass the light and dark schemes to `AppTheme` only when the preference is enabled. Put the availability scope above `MaterialApp` so settings descendants can read it.

- [x] **Step 4: Run Flutter static analysis and Android debug build**

Run from `app/`: `flutter analyze --no-pub`, then `flutter build apk --debug`.

Expected: both commands exit 0.

### Task 3: Add a platform-aware color-source selector

**Files:**
- Modify: `app/lib/ui/home_page.dart`
- Consume: `app/lib/theme/system_dynamic_color_scope.dart`

- [x] **Step 1: Add the source selector**

Show “RCH 配色 / 系统动态” only when running on Android and `dynamicColorsAvailable` is true. Keep the four palette choices visible in RCH mode and preserve the last selected palette when the user switches to dynamic mode.

- [x] **Step 2: Explain fallback behavior in the setting**

Add one short helper line that says unsupported Android versions use the selected RCH palette.

- [x] **Step 3: Run Flutter static analysis and inspect Android layouts**

Run from `app/`: `flutter analyze --no-pub`. Inspect the source selector on a compact phone and ensure the palette controls do not crowd the section.

Expected: analysis exits 0; unsupported devices do not show an enabled dynamic-color choice.
