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

- `flutter test` passes (247 tests); `flutter build windows --debug` passes.
- The Android compact appearance settings were inspected on the API 36 emulator; the selector, palette cards, font sample, and mobile layout fit without horizontal clipping.
- The desktop-width appearance settings and Windows material controls were visually inspected. After the light-theme/Mica regression fix, Windows shows the translucent light RCH surface over Mica instead of exposing a black backdrop when Windows itself is dark.
- Android dynamic color, Windows Mica/Acrylic, theme/font settings, and the light-theme Mica regression are implemented and verified; this task is ready to archive.
