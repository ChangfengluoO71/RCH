import 'dart:io';

import 'package:path_provider/path_provider.dart';

/// 缓存根标记文件（位于应用支持目录）：
/// 内容为当前应用根目录路径，空字符串表示默认根。
///
/// 用途：启动时在打开数据库之前恢复自定义根——
/// 数据库在根目录里，而根目录的设置又存在数据库里，
/// 标记文件提供了不依赖数据库的恢复入口。
const String _markerName = 'cache_root.txt';

Future<File> _markerFile() async {
  final dir = await getApplicationSupportDirectory();
  return File('${dir.path}${Platform.pathSeparator}$_markerName');
}

/// 读取缓存根标记；无标记或内容为空返回 null（= 默认根）。
Future<String?> readCacheRootMarker() async {
  try {
    final f = await _markerFile();
    if (!await f.exists()) return null;
    final s = (await f.readAsString()).trim();
    return s.isEmpty ? null : s;
  } catch (_) {
    return null;
  }
}

/// 写入缓存根标记（root 为空字符串 = 恢复默认根）。
Future<void> writeCacheRootMarker(String root) async {
  try {
    final f = await _markerFile();
    await f.writeAsString(root.trim());
  } catch (e) {
    // 标记写入失败不阻塞主流程（library.json 兜底仍可用）
    // ignore: avoid_print
    print('[cache_root_marker] 写入失败: $e');
  }
}
