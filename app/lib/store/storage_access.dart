import 'package:flutter/services.dart';
import 'package:app/ui/common.dart';

const MethodChannel _channel = MethodChannel('rch/storage');

/// Android 是否已授予"所有文件访问"（Android 11+）或读取存储权限（10-）。
Future<bool> isAllFilesAccessGranted() async {
  if (!isAndroidPlatform) return true;
  try {
    return await _channel.invokeMethod<bool>('isAllFilesAccessGranted') ?? false;
  } catch (_) {
    return false;
  }
}

/// 打开系统授权页（Android 11+ 特殊访问页；Android 10 及以下请求运行时权限）。
Future<void> openAllFilesAccessSettings() async {
  if (!isAndroidPlatform) return;
  try {
    await _channel.invokeMethod<void>('openAllFilesAccessSettings');
  } catch (_) {}
}
