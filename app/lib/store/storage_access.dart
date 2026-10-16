import 'dart:io';

import 'package:app/ui/rch_overlay.dart';
import 'package:flutter/services.dart';
import 'package:flutter/material.dart';
import 'package:app/ui/common.dart';
import 'package:path_provider/path_provider.dart';

const MethodChannel _channel = MethodChannel('rch/storage');

/// Paths in shared Android storage need the user-granted storage permission.
/// App-private paths (for example `/data/user/0/...`) do not.
bool isAndroidSharedStoragePath(String rawPath) {
  final path = rawPath.trim().replaceAll('\\', '/');
  if (path.isEmpty) return false;
  const roots = [
    '/sdcard',
    '/storage',
    '/mnt/media_rw',
    '/mnt/runtime',
    '/mnt/user',
  ];
  return roots.any((root) => path == root || path.startsWith('$root/'));
}

/// Android 是否已授予"所有文件访问"（Android 11+）或读取存储权限（10-）。
Future<bool> isAllFilesAccessGranted() async {
  if (!isAndroidPlatform) return true;
  try {
    return await _channel.invokeMethod<bool>('isAllFilesAccessGranted') ??
        false;
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

/// Opens the Android system folder picker at a shared-storage directory.
/// RCH does not retain a new Storage Access Framework grant from this action.
Future<bool> openAndroidDirectoryPicker(String directory) async {
  if (!isAndroidPlatform || directory.trim().isEmpty) return false;
  try {
    return await _channel.invokeMethod<bool>('openEhOutputDirectory', {
          'path': directory,
        }) ??
        false;
  } catch (_) {
    return false;
  }
}

/// Returns true once for each app installation that still needs storage
/// access, so Android users see the explanation on their first launch.
Future<bool> claimStartupAllFilesAccessPrompt() async {
  if (!isAndroidPlatform || await isAllFilesAccessGranted()) return false;
  try {
    final appSupport = await getApplicationSupportDirectory();
    final marker = File(
      '${appSupport.path}${Platform.pathSeparator}all_files_access_prompted',
    );
    if (await marker.exists()) return false;
    await marker.writeAsString('shown', flush: true);
  } catch (_) {
    // If the marker cannot be persisted, still show the prompt this launch.
  }
  return true;
}

/// 确保 Android 已授予"所有文件访问"权限；未授予时弹出引导对话框并打开系统设置页。
/// 返回是否已具备写入外部存储（导出到所选目录）的权限。
Future<bool> ensureAllFilesAccess(
  BuildContext context, {
  String explanation = '读取手机共享存储中的漫画、识别本地文件夹或导出到外部目录，需要授予“所有文件访问”权限。',
}) async {
  if (!isAndroidPlatform) return true;
  if (await isAllFilesAccessGranted()) return true;
  if (!context.mounted) return false;
  final go = await showRchDialog<bool>(
    context: context,
    builder: (c) => AlertDialog(
      title: const Text('需要“所有文件访问”权限'),
      content: Text('$explanation\n\n你可以暂不授权，之后在使用相关功能时再开启。'),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(c).pop(false),
          child: const Text('暂不授权'),
        ),
        FilledButton(
          onPressed: () => Navigator.of(c).pop(true),
          child: const Text('去授权'),
        ),
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
