import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

/// Narrow bridge to the Windows runner's native window-backdrop controls.
abstract final class WindowMaterialController {
  static const MethodChannel _channel = MethodChannel('rch/window_material');

  static bool get _isWindows => defaultTargetPlatform == TargetPlatform.windows;

  static Future<bool> isMicaSupported() async {
    if (!_isWindows) return false;
    try {
      final capabilities = await _channel.invokeMapMethod<String, dynamic>(
        'getCapabilities',
      );
      return capabilities?['mica'] == true;
    } on PlatformException {
      return false;
    } on MissingPluginException {
      return false;
    }
  }

  static Future<bool> setMaterial(String material) async {
    if (!_isWindows || (material != 'standard' && material != 'mica')) {
      return false;
    }
    try {
      return await _channel.invokeMethod<bool>('setMaterial', {
            'material': material,
          }) ??
          false;
    } on PlatformException {
      return false;
    } on MissingPluginException {
      return false;
    }
  }
}
