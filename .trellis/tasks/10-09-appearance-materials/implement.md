# Implementation Plan

**Goal:** Polish RCH appearance settings and add opt-in Android dynamic colors and Windows Mica/Acrylic materials.

**Architecture:** Implement four sequential, independently reviewable plans: appearance settings/system brightness; Android dynamic color; Windows Mica; Windows Acrylic overlays. Persist preferences in `AppSettings`; keep platform-specific work behind theme and overlay abstractions.

**Plan index:** [RCH appearance and materials plan](../../../docs/superpowers/plans/2026-10-09-rch-appearance-materials-plan.md)

## Execution order

1. [Appearance settings and system brightness](../../../docs/superpowers/plans/2026-10-09-rch-appearance-settings-plan.md)
2. [Android dynamic color](../../../docs/superpowers/plans/2026-10-09-rch-android-dynamic-color-plan.md)
3. [Windows Mica](../../../docs/superpowers/plans/2026-10-09-rch-windows-mica-plan.md)
4. [Windows Acrylic overlays](../../../docs/superpowers/plans/2026-10-09-rch-acrylic-overlays-plan.md)

The existing dirty files in the workspace predate this task and must be preserved. Do not stage unrelated file changes.

## Execution status

- `flutter analyze --no-pub`, `flutter build windows --debug`, and `flutter build apk --debug` pass.
- The Android compact appearance settings were inspected on the API 36 emulator; the selector, palette cards, font sample, and mobile layout fit without horizontal clipping.
- Windows Mica/Acrylic runtime appearance and the desktop-width settings layout still need visual inspection. The current workspace changes remain uncommitted, so the Trellis task stays active.
