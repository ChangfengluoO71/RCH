import 'package:flutter/services.dart';
import 'package:flutter/material.dart';
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

/// 确保 Android 已授予"所有文件访问"权限；未授予时弹出引导对话框并打开系统设置页。
/// 返回是否已具备写入外部存储（导出到所选目录）的权限。
Future<bool> ensureAllFilesAccess(BuildContext context) async {
  if (!isAndroidPlatform) return true;
  if (await isAllFilesAccessGranted()) return true;
  if (!context.mounted) return false;
  final go = await showDialog<bool>(
    context: context,
    builder: (c) => AlertDialog(
      title: const Text('需要存储权限'),
      content: const Text('导出到手机目录需要授予"所有文件访问"权限，请在系统设置中开启后重试。'),
      actions: [
        TextButton(onPressed: () => Navigator.of(c).pop(false), child: const Text('取消')),
        FilledButton(onPressed: () => Navigator.of(c).pop(true), child: const Text('去授权')),
      ],
    ),
  );
  if (go == true) await openAllFilesAccessSettings();
  return false;
}

/// Android 应用原生库目录（含打包的 libpdfium.so），供 Rust 端 pdfium 加载。
Future<String?> nativeLibraryDir() async {
  if (!isAndroidPlatform) return null;
  try {
    return await _channel.invokeMethod<String>('nativeLibraryDir');
  } catch (_) {
    return null;
  }
}
