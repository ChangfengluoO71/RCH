# RCH Appearance and Materials Implementation Plan

> **For agentic workers:** Implement the linked subplans in order, in the current shared workspace. Keep existing unrelated edits intact.

**Goal:** Polish RCH appearance settings and add system-aware color and Windows material options while preserving the existing themes, fonts, reader canvas, and local settings compatibility.

**Architecture:** Split this work into four independently reviewable deliverables: appearance settings and system brightness, Android dynamic color, Windows Mica, and Flutter Acrylic overlays. Each deliverable owns its own settings and platform integration; shared AppSettings and appearance UI changes are applied in sequence.

**Tech Stack:** Flutter Material 3, Dart JSON settings, `dynamic_color`, Flutter `BackdropFilter`, Windows Win32 DWM system backdrops.

**Spec:** [2026-10-09-rch-appearance-materials-design.md](../specs/2026-10-09-rch-appearance-materials-design.md)

## Global Constraints

- Preserve the existing four palettes and four fonts.
- Keep dark mode as the new-install and legacy-settings default.
- Keep appearance preferences in local `AppSettings`; do not add them to book metadata or sync rows.
- Apply Android wallpaper dynamic colors only on Android 12+; otherwise use the saved RCH palette.
- Apply Mica only on Windows 11 22H2 (build 22621) and newer; unsupported systems use standard surfaces.
- Apply Acrylic only to RCH `AlertDialog` and modal bottom-sheet surfaces; anchored menus, dropdowns, file pickers, covers, grids, and reader images remain standard.
- Keep normal-size text contrast at or above 4.5:1 and disable Acrylic when Flutter reports high contrast.
- Preserve existing user changes in shared files; do not stage unrelated working-tree edits.

## Review Focus

- Legacy or unknown `themeMode` values must continue to resolve to dark mode.
- Android dynamic colors may be absent even when the setting is enabled; the saved fixed palette must remain available and unchanged.
- `ThemeMode.system` must switch between the matching light and dark dynamic schemes without resetting the selected font.
- A Windows DWM failure or unsupported OS must leave the Flutter window readable with standard surfaces.
- Acrylic must remain clipped to modal surfaces and must not blur the entire app or alter comic image pixels.

## Execution Order

1. [Appearance settings and system brightness](2026-10-09-rch-appearance-settings-plan.md)
2. [Android dynamic color](2026-10-09-rch-android-dynamic-color-plan.md)
3. [Windows Mica](2026-10-09-rch-windows-mica-plan.md)
4. [Windows Acrylic overlays](2026-10-09-rch-acrylic-overlays-plan.md)

This plan assumes native, in-session implementation. The four deliverables share `home_page.dart` and `AppSettings`, so execute them sequentially and review each diff before moving to the next. Delegation is only used if the user explicitly requests it.
