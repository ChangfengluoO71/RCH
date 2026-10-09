# RCH Acrylic Overlay Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an optional, bounded Acrylic-like blur to RCH dialog and modal-sheet surfaces on Windows.

**Architecture:** A shared Flutter presentation helper wraps RCH dialog routes and modal bottom sheets in a clipped `BackdropFilter` with a high-opacity themed surface. The helper reads an inherited overlay-material preference, so UI entrypoints do not access persistence or platform APIs directly.

**Tech Stack:** Flutter Material 3, `BackdropFilter`, `ImageFilter`, inherited appearance scope.

**Spec:** [Appearance and materials design](../specs/2026-10-09-rch-appearance-materials-design.md)

## Global Constraints

- `overlayMaterial` defaults to `standard` and is available only on Windows 11 22H2 (build 22621) and newer, matching the supported material settings surface.
- Acrylic applies to RCH `AlertDialog` and modal bottom sheets only.
- Anchored menus, dropdowns, system pickers, comic covers, poster grids, and reader images remain standard.
- Blur stays clipped to the dialog or modal-sheet surface.
- Disable Acrylic when Flutter reports high contrast; normal-size text contrast remains at least 4.5:1.
- Keep the preference in local `AppSettings`, outside book metadata and sync rows.

## Review Focus

- Standard mode creates no backdrop filter and retains the current dialog appearance.
- Acrylic mode blurs only inside the modal surface's clip bounds.
- High-contrast mode uses the standard material even when Acrylic is selected.
- Non-Windows targets and Windows builds below 22621 keep standard material.
- Reader overlays do not create a filter over the full reading viewport.

---

### Task 1: Persist the overlay-material preference and expose it to overlays

**Files:**
- Modify: `app/lib/store/models.dart`
- Modify: `app/lib/main.dart`
- Create: `app/lib/theme/overlay_material_scope.dart`
- Modify: `app/lib/ui/home_page.dart`
- Consume: `app/lib/theme/window_material_scope.dart`

**Interfaces:**
- `AppSettings.overlayMaterial` accepts `standard` or `acrylic`; default and unknown-value fallback are `standard`.
- `OverlayMaterialScope` exposes the persisted `acrylicSelected` value to descendants of `MaterialApp`; the presentation helper checks platform and high-contrast state at the overlay context.

- [x] **Step 1: Add the persisted setting**

Add constructor, JSON write, and normalized JSON read for `overlayMaterial`.

- [x] **Step 2: Add the inherited overlay scope**

Expose the persisted selection above `MaterialApp`. Keep Windows build capability and `MediaQuery.highContrastOf` checks inside the dialog/sheet presenter, where the effective overlay context is available.

- [x] **Step 3: Add the Windows-only selector**

Show “标准 / 亚克力” only when `WindowMaterialScope.micaSupported` is true, with a short description that Acrylic affects app dialog and modal-sheet surfaces.

### Task 2: Implement shared dialog and modal-sheet presenters

**Files:**
- Create: `app/lib/ui/rch_overlay.dart`
- Consume: `app/lib/theme/overlay_material_scope.dart`

**Interfaces:**
- `showRchDialog<T>({required BuildContext context, required WidgetBuilder builder, bool barrierDismissible = true, bool useRootNavigator = true}) -> Future<T?>`.
- `showRchModalBottomSheet<T>({required BuildContext context, required WidgetBuilder builder, bool isScrollControlled = false}) -> Future<T?>`.

- [x] **Step 1: Preserve the standard dialog path**

When Acrylic is disabled, delegate to Flutter `showDialog` and `showModalBottomSheet` without adding filters or changing current route behavior.

- [x] **Step 2: Add a clipped Acrylic surface**

When enabled, wrap the dialog or sheet in `ClipRRect` and `BackdropFilter(ImageFilter.blur(...))`; place a high-opacity `ColorScheme.surface` layer above the filter and render the dialog content with transparent dialog background.

- [x] **Step 3: Preserve existing route options**

Forward dismissal, root-navigator, scroll-control, barrier, and safe-area options used by current call sites.

### Task 3: Route existing app dialogs through the shared presenters

**Files:**
- Modify: `app/lib/main.dart`
- Modify: `app/lib/ui/backup_panel.dart`
- Modify: `app/lib/ui/cloud115_qr_scan.dart`
- Modify: `app/lib/ui/book_detail_page.dart`
- Modify: `app/lib/ui/book_navigation.dart`
- Modify: `app/lib/ui/cache_manager.dart`
- Modify: `app/lib/ui/eh_subscription_panel.dart`
- Modify: `app/lib/ui/home_page.dart`
- Modify: `app/lib/ui/quark_qr_scan.dart`
- Modify: `app/lib/store/ai_upscale_manager.dart`
- Modify: `app/lib/ui/update_panel.dart`
- Modify: `app/lib/ui/source_browser.dart`
- Modify: `app/lib/ui/reader_page.dart`
- Modify: `app/lib/store/storage_access.dart`

- [x] **Step 1: Migrate RCH `showDialog` call sites**

Replace app-owned `showDialog` calls in the listed files with `showRchDialog`, preserving each call's result type, builder, dismissal behavior, and navigator selection.

- [x] **Step 2: Migrate the modal bottom sheet**

Replace the reader's `showModalBottomSheet` call with `showRchModalBottomSheet`, keeping its current scroll and drag behavior.

- [x] **Step 3: Check for remaining dialog and sheet entrypoints**

Run `rg -n 'showDialog|showModalBottomSheet' app/lib` and confirm remaining matches are only the two calls inside the shared presenter or explicitly excluded platform surfaces.

### Task 4: Verify appearance and platform behavior

**Files:** all files changed in Tasks 1–3.

- [x] **Step 1: Run Flutter static analysis**

Run from `app/`: `flutter analyze --no-pub`.

Expected: exit code 0.

- [x] **Step 2: Build the Windows app**

Run from `app/`: `flutter build windows --debug`.

Expected: exit code 0.

- [ ] **Step 3: Inspect representative overlays**

Inspect one ordinary confirmation dialog, one custom dialog, and the reader modal sheet in standard and Acrylic modes. Confirm the filter remains within each modal surface and text remains legible.
