// 同步路径与冲突副本识别的纯逻辑（无 Rust 依赖，便于单测）。

import 'dart:io';

/// 正式同步包文件名。
const kSyncLatestName = 'latest.rchpkg';

/// 是否为需要忽略的同步文件（网盘冲突副本 / 半成品临时文件）。
///
/// 覆盖常见网盘客户端命名：`latest (冲突副本).rchpkg`、`latest(1).rchpkg`、
/// `latest-xxx.rchpkg`，以及应用自身的 `latest.rchpkg.tmp`。
bool isIgnoredSyncFile(String name) {
  if (name.toLowerCase() == kSyncLatestName) return false;
  final lower = name.toLowerCase();
  return lower.startsWith('latest') &&
      (lower.endsWith('.rchpkg') || lower.endsWith('.rchpkg.tmp'));
}

int countIgnoredSyncFiles(List<String> names) =>
    names.where(isIgnoredSyncFile).length;

/// 模式 B（同步盘目录）：正式包与归档路径。
String syncLatestPath(String dir) =>
    '$dir${Platform.pathSeparator}$kSyncLatestName';

String syncArchiveDir(String dir) =>
    '$dir${Platform.pathSeparator}archive';

String syncArchivePath(String dir, String timestamp) =>
    '${syncArchiveDir(dir)}${Platform.pathSeparator}$timestamp.rchpkg';

/// 模式 A（WebDAV）：远程路径统一用 `/`。
String _normRemoteBase(String base) {
  final b = base.trim();
  if (b.isEmpty || b == '/') return '';
  return b.endsWith('/') ? b.substring(0, b.length - 1) : b;
}

String remoteRchDir(String base) => '${_normRemoteBase(base)}/RCH';

String remoteSyncDir(String base) => '${_normRemoteBase(base)}/RCH/sync';

String remoteLatestPath(String base) =>
    '${remoteSyncDir(base)}/$kSyncLatestName';

String remoteArchiveDir(String base) =>
    '${remoteSyncDir(base)}/archive';

/// 归一化 WebDAV 远程目录：保证以 `/` 开头、去尾部斜杠；空值返回 ''。
String normalizeRemoteDir(String dir) {
  var d = dir.trim();
  if (d.isEmpty || d == '/') return '';
  if (!d.startsWith('/')) d = '/$d';
  while (d.endsWith('/')) {
    d = d.substring(0, d.length - 1);
  }
  return d;
}

/// 远程目录逐级路径（用于幂等 MKCOL）：`/RCH/sync` → ['/RCH', '/RCH/sync']。
List<String> remoteDirLevels(String dir) {
  final d = normalizeRemoteDir(dir);
  if (d.isEmpty) return const [];
  final segs = d.split('/').where((s) => s.isNotEmpty).toList();
  final out = <String>[];
  var cur = '';
  for (final s in segs) {
    cur = '$cur/$s';
    out.add(cur);
  }
  return out;
}

/// 远程目录 + 文件名拼接（统一 `/`）。
String remoteJoin(String dir, String name) {
  final d = normalizeRemoteDir(dir);
  if (d.isEmpty) return '/$name';
  return '$d/$name';
}

/// 时间戳归档名：`yyyyMMdd_HHmmss`。
String formatSyncTimestamp(DateTime t) {
  String two(int v) => v.toString().padLeft(2, '0');
  return '${t.year}${two(t.month)}${two(t.day)}_${two(t.hour)}${two(t.minute)}${two(t.second)}';
}
