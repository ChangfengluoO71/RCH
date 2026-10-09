# Platform Material Contracts

## 1. Scope / Trigger

This contract applies when changing appearance preferences, Android wallpaper-derived colors, the Windows native window backdrop, or RCH modal surfaces. It captures the Flutter-to-Win32 channel boundary and the fallback behavior shared by Dart and native code.

## 2. Signatures

```dart
abstract final class WindowMaterialController {
  static Future<bool> isMicaSupported();
  static Future<bool> setMaterial(String material); // standard | mica
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
```

Native MethodChannel: `rch/window_material`.

- `getCapabilities()` returns `{ "mica": bool }`.
- `setMaterial({ "material": "standard" | "mica" })` returns `bool`.

Local `AppSettings` fields and defaults:

- `themeMode`: `dark | light | system`, default `dark`.
- `useSystemDynamicColors`: boolean, default `false`.
- `windowMaterial`: `standard | mica`, default `standard`.
- `overlayMaterial`: `standard | acrylic`, default `standard`.

## 3. Contracts

- Settings are stored in local `AppSettings`; do not add these fields to book metadata or sync rows.
- `WindowMaterialController` returns `false` without a platform call off Windows. A missing plugin or `PlatformException` also resolves to `false`.
- The Windows runner reports Mica support only for a valid window handle on Windows build 22621 or later. It applies `DWMWA_SYSTEMBACKDROP_TYPE` attribute 38 with `DWMSBT_MAINWINDOW` value 2 for Mica and `DWMSBT_NONE` value 1 for standard.
- `RchApp` owns capability loading and material application outside `build`. Flutter's root scaffold/canvas become transparent only after the native Mica request succeeds.
- Android dynamic schemes are applied only on Android and only when enabled. A missing scheme leaves the saved RCH palette untouched and active.
- `dynamic_color` 2.x exposes `material_ui.ColorScheme`; import that type with a namespace and keep the conversion helper typed. Its inverse foreground field is `onInverseSurface`, matching Flutter's role name. Avoid `dynamic` here so misspelled role names fail analysis instead of crashing when the option is selected.
- The Acrylic presenter uses a clipped backdrop filter only on Windows when Mica capability is available, Acrylic is selected, and high-contrast mode is off. Standard mode delegates to Flutter's normal dialog or sheet API.
- Acrylic dialog and sheet routes use a light scrim so the backdrop filter samples the app content instead of an opaque modal barrier. Keep the tinted surface translucent enough for the blur to show while preserving text contrast; avoid near-opaque fills such as 96%.
- App-owned dialog and modal-sheet entrypoints use the shared presenters. Keep the reader canvas, comic images, covers, anchored menus, dropdowns, and system pickers outside the blur.

## 4. Validation & Error Matrix

| Condition | Required behavior |
| --- | --- |
| Missing or unknown persisted material value | Normalize to `standard`; do not throw during settings load. |
| Windows build below 22621 or invalid HWND | Report Mica unavailable and render standard Flutter surfaces. |
| DWM rejects Mica | Clear the native backdrop, return `false`, and keep the Flutter root opaque. |
| Standard material selected | Clear the DWM backdrop and do not create a Flutter backdrop filter. |
| Android dynamic color scheme is absent | Keep using the selected RCH palette; do not rewrite the preference. |
| Dynamic-color preference is enabled off Android | Ignore the preference for rendering and use the fixed RCH theme. |
| Acrylic is selected with high contrast enabled | Render the standard modal surface. |
| Acrylic modal is shown | Blur is clipped to the dialog or sheet surface; page content outside that clip remains unchanged. |
| Native channel call fails or plugin is absent | Resolve capability/application as `false`; retain standard surfaces. |

## 5. Good / Base / Bad Cases

- Good: A Windows Mica preference is applied after the HWND capability check; only a successful native response makes the Flutter backdrop transparent.
- Base: A legacy settings JSON without the new fields loads with the dark RCH theme, RCH palette, and standard materials.
- Good: Android returns no wallpaper scheme; the app continues with the selected fixed palette and keeps the dynamic-color preference unchanged.
- Bad: Calling a platform channel from `build`, or making the root transparent before DWM confirms Mica.
- Bad: Applying `BackdropFilter` to the app's full content stack or reader viewport.
- Bad: Calling Flutter's `showDialog` directly from an app-owned entrypoint and bypassing the shared material gate.

## 6. Tests Required

- Settings serialization: missing, valid, and unknown values normalize to the documented defaults and round-trip without changing unrelated settings.
- Dart controller: non-Windows, missing-plugin, and platform-error paths return `false`.
- Windows runner/channel: verify capability results around build 22621, valid/invalid HWND handling, method names, accepted payloads, and DWM failure fallback.
- Theme integration: verify light/dark/system selection and Android-only dynamic-scheme application; null schemes retain the fixed palette.
- Modal presenter: verify standard mode makes no filter, Acrylic is clipped, high contrast disables Acrylic, and non-Windows uses standard surfaces.
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
final applied = supported && await WindowMaterialController.setMaterial('mica');

showRchDialog(context: context, builder: buildDialog);
```

The correct pattern keeps platform I/O outside rendering and centralizes modal material and fallback policy.
