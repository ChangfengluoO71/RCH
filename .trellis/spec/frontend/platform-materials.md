# Platform Material Contracts

## 1. Scope / Trigger

This contract applies when changing appearance preferences, Android wallpaper-derived colors, the Windows native window backdrop, or RCH modal surfaces. It captures the Flutter-to-Win32 channel boundary and the fallback behavior shared by Dart and native code.

## 2. Signatures

```dart
abstract final class WindowMaterialController {
  static Future<bool> isMicaSupported();
  static Future<bool> setMaterial(
    String material, {
    required String themeMode,
  }); // standard | mica
}

Future<T?> showRchDialog<T>({
  required BuildContext context,
  required WidgetBuilder builder,
  bool barrierDismissible = true,
  bool useRootNavigator = true,
});

Future<T?> showRchModalBottomSheet<T>({
  required BuildContext context,
  required WidgetBuilder builder,
  bool isScrollControlled = false,
  bool isDismissible = true,
  bool enableDrag = true,
  bool useRootNavigator = false,
  bool useSafeArea = false,
});

LayoutProfile resolveLayoutProfile(BuildContext context);
void AppOrientationCoordinator.updateHome(LayoutProfile profile);
void AppOrientationCoordinator.enterReader(Object owner);
void AppOrientationCoordinator.leaveReader(Object owner);
```

Native MethodChannel: `rch/window_material`.

- `getCapabilities()` returns `{ "mica": bool }`.
- `setMaterial({ "material": "standard" | "mica", "themeMode": "light" | "dark" | "system" })` returns `bool`.

Local `AppSettings` fields and defaults:

- `themeMode`: `dark | light | system`, default `dark`.
- `useSystemDynamicColors`: boolean, default `false`.
- `windowMaterial`: `standard | mica`, default `standard`.
- `overlayMaterial`: `standard | acrylic`, default `standard`.
- `tabletLayout`: `auto | desktop | tablet | mobile`, default `auto`.
- `tabletOverlayMaterial`: `standard | frosted`, default `standard`.

## 3. Contracts

- Settings are stored in local `AppSettings`; do not add these fields to book metadata or sync rows.
- `WindowMaterialController` returns `false` without a platform call off Windows. A missing plugin or `PlatformException` also resolves to `false`.
- The Windows runner reports Mica support only for a valid window handle on Windows build 22621 or later. It applies `DWMWA_SYSTEMBACKDROP_TYPE` attribute 38 with `DWMSBT_MAINWINDOW` value 2 for Mica and `DWMSBT_NONE` value 1 for standard.
- While Mica is active, the runner sets `DWMWA_USE_IMMERSIVE_DARK_MODE` to an explicit RCH light/dark selection so a light RCH theme does not expose a dark Windows backdrop. `system` clears the app override and uses Windows' current preference. Selecting standard also clears the override and restores the Windows preference.
- Windows colorization/theme messages preserve an explicit RCH brightness override while Mica is active; system mode follows the Windows preference.
- `RchApp` owns capability loading and material application outside `build`. Flutter's root scaffold/canvas become transparent only after the native Mica request succeeds.
- Android dynamic schemes are applied only on Android and only when enabled. A missing scheme leaves the saved RCH palette untouched and active.
- `dynamic_color` 2.x exposes `material_ui.ColorScheme`; import that type with a namespace and keep the conversion helper typed. Its inverse foreground field is `onInverseSurface`, matching Flutter's role name. Avoid `dynamic` here so misspelled role names fail analysis instead of crashing when the option is selected.
- The Windows Acrylic presenter uses a clipped backdrop filter only when Mica capability is available, Acrylic is selected, and high-contrast mode is off. Android uses the independent tablet overlay preference and never requires Windows Mica. Standard mode delegates to Flutter's normal dialog or sheet API.
- Acrylic dialog and sheet routes use a light scrim so the backdrop filter samples the app content instead of an opaque modal barrier. Keep the tinted surface translucent enough for the blur to show while preserving text contrast; avoid near-opaque fills such as 96%.
- A `Dialog` occupies the whole route even when its visible Material is small. The dialog presenter locates the actual Material surface and clips the backdrop to its bounds; remeasure after layout or paint changes. If no Dialog/Material can be located, leave the content opaque and standard. Never wrap the entire route in a filter.
- App-owned dialog and modal-sheet entrypoints use the shared presenters. Keep the reader canvas, comic images, covers, anchored menus, dropdowns, and system pickers outside the blur.
- On Android the tablet frosted option applies only to the tablet experience and only outside the reader. High contrast or the system reduce-animations preference falls back to standard. There is no automatic low-performance detection; users can choose standard when performance is poor. The settings page supplies a preview and does not call the effect Windows Mica.
- Resolve tablet identity independently of compact window width. `tablet` is an explicit experience; `auto` selects Android tablet when the logical display's shortest side is at least 600 dp (use display size rather than split-screen window size). The viewport may still be compact below 600 dp. `desktop` and `mobile` retain their stored values and the mobile selection keeps compact navigation.
- Android home orientation follows `LayoutProfile.lockPortrait`, not `isCompact` alone. The coordinator keeps the latest home policy while readers permit rotation; a reader may restore it only with its own owner token, so a closing old reader cannot override a newly opened one. Recompute the home policy on settings or viewport changes.

## 4. Validation & Error Matrix

| Condition | Required behavior |
| --- | --- |
| Missing or unknown persisted material value | Normalize to `standard`; do not throw during settings load. |
| Windows build below 22621 or invalid HWND | Report Mica unavailable and render standard Flutter surfaces. |
| DWM rejects Mica | Clear the native backdrop, return `false`, and keep the Flutter root opaque. |
| RCH light theme while Windows is dark | Apply the light DWM mode while Mica is active; the transparent Flutter root must not reveal a dark/black backdrop. |
| Standard material selected | Clear the DWM backdrop and do not create a Flutter backdrop filter. |
| Android dynamic color scheme is absent | Keep using the selected RCH palette; do not rewrite the preference. |
| Dynamic-color preference is enabled off Android | Ignore the preference for rendering and use the fixed RCH theme. |
| Acrylic is selected with high contrast enabled | Render the standard modal surface. |
| Android tablet frosted is selected with reduced animations or a reader open | Render the standard modal surface. |
| A device struggles with frosted rendering | Offer the explicit standard option; do not claim automatic device-performance detection. |
| Acrylic or frosted modal is shown | Blur is clipped to the actual dialog or sheet surface; page content outside that clip remains unchanged. |
| Dialog content resizes or the window changes | Remeasure the visible surface and update the clipped region. |
| Unknown dialog surface | Keep its normal opaque surface, without backdrop filtering. |
| Tablet in a narrow split-screen window | Use compact navigation but retain tablet selection and rotation. |
| Reader exits after home settings or dimensions change | Restore the latest home orientation policy, ignoring stale reader owners. |
| Native channel call fails or plugin is absent | Resolve capability/application as `false`; retain standard surfaces. |

## 5. Good / Base / Bad Cases

- Good: A Windows Mica preference is applied after the HWND capability check; only a successful native response makes the Flutter backdrop transparent.
- Base: A legacy settings JSON without the new fields loads with the dark RCH theme, RCH palette, and standard materials.
- Good: Android returns no wallpaper scheme; the app continues with the selected fixed palette and keeps the dynamic-color preference unchanged.
- Good: Android auto recognizes a tablet from display size and keeps it rotatable while the split-screen viewport uses compact navigation.
- Base: Old `desktop` and Windows material settings round-trip unchanged; a missing tablet overlay preference is standard.
- Bad: Calling a platform channel from `build`, or making the root transparent before DWM confirms Mica.
- Bad: Applying `BackdropFilter` to the full dialog route (whose size is the screen), the app's full content stack, or the reader viewport.
- Bad: Calling Flutter's `showDialog` directly from an app-owned entrypoint and bypassing the shared material gate.

## 6. Tests Required

- Settings serialization: missing, valid, and unknown values normalize to the documented defaults and round-trip without changing unrelated settings.
- Dart controller: non-Windows, missing-plugin, and platform-error paths return `false`.
- Windows runner/channel: verify capability results around build 22621, valid/invalid HWND handling, method names, accepted payloads, and DWM failure fallback.
- Theme integration: verify light/dark/system selection and Android-only dynamic-scheme application; null schemes retain the fixed palette.
- Modal presenter: verify standard mode makes no filter, Windows Acrylic retains its capability gate, and Android tablet frosted does not require Mica. Assert that the filter's rectangle equals the actual dialog surface before and after resizing, and that sheets clip to their own bounds.
- Verify Android high contrast, reduced animations, explicit phone mode and active reader all use standard modal surfaces; verify independent Android/Windows preferences and the settings preview.
- Verify Android tablet split screen is compact but rotatable, phone auto mode remains portrait, and reader exit restores the latest home policy without stale owners winning.
- Verify app-owned overlay call sites use the shared presenters and reader images are not wrapped in a blur.

## 7. Wrong vs Correct

Wrong:

```dart
Widget build(BuildContext context) {
  WindowMaterialController.setMaterial('mica');
  return const Scaffold();
}

showDialog(context: context, builder: buildDialog);
```

Correct:

```dart
// Load/apply native capability from the app lifecycle coordinator, not build.
final supported = await WindowMaterialController.isMicaSupported();
final settings = LibraryStore.instance.settings;
final applied = supported &&
    await WindowMaterialController.setMaterial(
      'mica',
      themeMode: settings.themeMode,
    );

showRchDialog(context: context, builder: buildDialog);
```

The correct pattern keeps platform I/O outside rendering and centralizes modal material and fallback policy.
