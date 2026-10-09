# RCH Appearance Settings Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make RCH appearance settings easier to scan, preview, and use, including a persistent follow-system brightness mode.

**Architecture:** Keep the existing app-wide `AppTheme` and `AppSettings` flow. Extend the brightness setting with `system`, then present brightness, palette, and font as separate, consistently spaced controls with live samples.

**Tech Stack:** Flutter Material 3, Dart JSON settings, `ThemeData`, `ColorScheme`.

**Spec:** [Appearance and materials design](../specs/2026-10-09-rch-appearance-materials-design.md)

## Global Constraints

- Preserve the existing four palettes and four fonts.
- Keep dark mode as the new-install and legacy-settings default.
- Keep appearance preferences in local `AppSettings`; do not add them to book metadata or sync rows.
- Keep normal-size text contrast at or above 4.5:1.
- Do not redesign settings categories outside the existing appearance section.
- Preserve all unrelated edits already present in `home_page.dart` and other shared files.

## Review Focus

- Missing and unknown `themeMode` values resolve to `dark`.
- Existing saved `light` and `dark` values keep their current meanings.
- System brightness changes update the app without resetting palette or font.
- Palette preview and font sample match the selected app theme.
- Controls reflow without clipping at phone and desktop widths.

---

### Task 1: Add a backward-compatible system brightness mode

**Files:**
- Modify: `app/lib/store/models.dart`
- Modify: `app/lib/main.dart`

**Interfaces:**
- Consumes: `AppSettings.themeMode` values `dark`, `light`, `system`.
- Produces: `MaterialApp.themeMode` maps to `ThemeMode.dark`, `ThemeMode.light`, or `ThemeMode.system`.

- [x] **Step 1: Normalize persisted mode values**

Allow only `dark`, `light`, and `system` in `AppSettings.fromJson`; keep the constructor and missing/unknown-value default at `dark`.

- [x] **Step 2: Map the persisted mode to MaterialApp**

Replace the current light-versus-dark boolean mapping in `RchApp` with an exhaustive mapping to Flutter `ThemeMode`.

- [x] **Step 3: Run Flutter static analysis**

Run from `app/`: `flutter analyze --no-pub`.

Expected: exit code 0 with no new diagnostics.

### Task 2: Refine the appearance controls and previews

**Files:**
- Modify: `app/lib/ui/home_page.dart`
- Modify: `app/lib/theme/app_theme.dart` only if a small shared palette-preview helper is needed

**Interfaces:**
- Consumes: existing `AppSettings.themeMode`, `themePalette`, and `appFont`; existing `AppTheme.build` and `AppTheme.fontFamilyFor`.
- Produces: appearance controls grouped as “外观”, “显示”, and “窗口”; selected palette and font previews.

- [x] **Step 1: Replace the brightness selector with three choices**

Add “跟随系统” beside “深色” and “浅色”; persist immediately through `LibraryStore.instance.updateSettings`.

- [x] **Step 2: Present fixed palettes with readable live previews**

Keep Classic, Sea Glass, Warm Paper, and Ink Night. Each option must show its accent, surface, and sample text using the palette's actual Material 3 roles.

- [x] **Step 3: Present fonts with a sample row**

Retain the four font choices and show a sample in the selected font. Keep the existing Xingshu readability hint.

- [x] **Step 4: Apply consistent spacing and responsive wrapping**

Use the existing theme surface/container roles and ensure controls reflow at narrow widths without changing unrelated settings categories.

- [ ] **Step 5: Run Flutter static analysis and inspect layouts**

Run from `app/`: `flutter analyze --no-pub`. Inspect the appearance section at a compact phone width and a desktop width.

Expected: analysis exits 0; all labels and controls remain visible and legible.
