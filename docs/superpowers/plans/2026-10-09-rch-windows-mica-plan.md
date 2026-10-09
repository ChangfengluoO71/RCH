# RCH Windows Mica Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an optional Windows 11 Mica main-window backdrop with a reliable standard-surface fallback.

**Architecture:** A typed Dart controller communicates with the Win32 runner through one method channel. The runner checks Windows build 22621 or newer and applies `DWMSBT_MAINWINDOW`; Flutter keeps its root surface transparent only while Mica is both selected and supported.

**Tech Stack:** Flutter, `MethodChannel`, Win32 C++, DWM `DWMWA_SYSTEMBACKDROP_TYPE`.

**Spec:** [Appearance and materials design](../specs/2026-10-09-rch-appearance-materials-design.md)

## Global Constraints

- Mica is Windows-only and opt-in; `windowMaterial` defaults to `standard`.
- Enable Mica only on Windows 11 22H2 (build 22621) and newer.
- DWM failure or unsupported Windows must use standard surfaces.
- Do not change Android, Linux, or macOS window backgrounds.
- Keep normal-size text contrast at or above 4.5:1.

## Review Focus

- Windows 10 and Windows 11 builds below 22621 report Mica unsupported.
- Windows 11 22H2+ applies `DWMSBT_MAINWINDOW` and shows the Flutter content over it.
- DWM errors clear the backdrop and preserve normal app rendering.
- Changing the preference while the app is open applies it without restarting.
- A saved Mica value on a newly unsupported environment resolves visually to standard.

---

### Task 1: Add the Win32 appearance method channel

**Files:**
- Modify: `app/windows/runner/flutter_window.h`
- Modify: `app/windows/runner/flutter_window.cpp`
- Modify: `app/windows/runner/win32_window.h`
- Modify: `app/windows/runner/win32_window.cpp`

**Interfaces:**
- Channel: `rch/window_material`.
- Method `getCapabilities` returns `{mica: bool}`.
- Method `setMaterial` consumes `{"material": "standard" | "mica"}` and returns whether the requested material was applied.

- [x] **Step 1: Add a native backdrop capability check**

Report Mica supported only when Windows build is at least 22621 and the window handle is valid.

- [x] **Step 2: Add safe backdrop application**

Use `DWMWA_SYSTEMBACKDROP_TYPE` with `DWMSBT_MAINWINDOW` for Mica and `DWMSBT_NONE` for standard. If the SDK lacks the attribute declaration, use a guarded compatibility definition like the existing dark-mode attribute definition.

- [x] **Step 3: Register and retain the method channel**

Register the handler on the runner engine messenger after engine creation; store the channel for the engine lifetime and return explicit success/failure values.

- [x] **Step 4: Build the Windows runner**

Run from `app/`: `flutter build windows --debug`.

Expected: native and Flutter compilation succeeds.

### Task 2: Add the Dart controller and apply Mica through app lifecycle

**Files:**
- Create: `app/lib/store/window_material_controller.dart`
- Create: `app/lib/theme/window_material_scope.dart`
- Modify: `app/lib/main.dart`
- Modify: `app/lib/theme/app_theme.dart`

**Interfaces:**
- `WindowMaterialController.isMicaSupported() -> Future<bool>`.
- `WindowMaterialController.setMaterial(String material) -> Future<bool>`.
- `WindowMaterialScope` exposes the asynchronously loaded `micaSupported` capability above `MaterialApp`.
- `AppTheme.build` retains the dynamic-scheme input from the Android plan and adds `bool transparentWindowBackdrop = false`.

- [x] **Step 1: Implement a typed Dart channel wrapper**

Wrap `rch/window_material`; on non-Windows targets report unsupported without making a platform call.

- [x] **Step 2: Apply the stored preference outside build methods**

Convert `RchApp` to a stateful coordinator, query capability once, apply the stored value at startup, and listen for settings changes so updates are immediate. Do not invoke the platform channel from `build`.

- [x] **Step 3: Reveal the native backdrop only when effective**

Use a transparent root scaffold only when the platform is Windows, native Mica is supported, and `windowMaterial == 'mica'`. Keep cards and interactive Material surfaces themed.

- [x] **Step 4: Run static analysis and rebuild Windows**

Run `flutter analyze --no-pub` and `flutter build windows --debug` from `app/`.

Expected: both commands exit 0.

### Task 3: Add the Windows-only setting

**Files:**
- Modify: `app/lib/store/models.dart`
- Modify: `app/lib/ui/home_page.dart`
- Consume: `app/lib/theme/window_material_scope.dart`

- [x] **Step 1: Persist `windowMaterial`**

Add `standard` / `mica` values with `standard` as constructor, missing-value, and unknown-value fallback.

- [x] **Step 2: Add the capability-aware selector**

Show “标准 / 云母” only on Windows. Disable Mica with a Windows 11 22H2 requirement hint when native capability reports false.

- [ ] **Step 3: Inspect supported and unsupported Windows behavior**

Verify a supported Windows 11 build displays the backdrop and an unsupported build keeps the normal solid surface; check changing the selector applies immediately.
