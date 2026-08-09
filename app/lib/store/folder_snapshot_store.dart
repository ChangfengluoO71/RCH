import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:app/src/rust/api/cache.dart';
import 'package:app/store/models.dart';

/// 远程目录快照条目（只需 name/path/isDir，用于“第一个漫画文件”判定）。
class FolderSnapshotEntry {
  final String name;
  final String path;
  final bool isDir;

  const FolderSnapshotEntry({
    required this.name,
    required this.path,
    required this.isDir,
  });

  Map<String, dynamic> toJson() => {
        'name': name,
        'path': path,
        'isDir': isDir,
      };

  factory FolderSnapshotEntry.fromJson(Map<String, dynamic> j) =>
      FolderSnapshotEntry(
        name: j['name'] as String? ?? '',
        path: j['path'] as String? ?? '',
        isDir: j['isDir'] as bool? ?? false,
      );
}

/// 远程目录快照存储：进程内 Map + 磁盘 JSON（缓存根目录 folder_snapshots.json）。
///
/// 封面检测只读本地快照（绝不触发网盘请求）；快照在用户浏览目录时写入，
/// 复用同一次列表响应。跨页面 / 跨重启保留，解决扁平路径来源
/// （115/夸克用 pickcode/fid，无层级）重启后无法从阅读记录反推
/// “文件夹内第一个漫画文件”的问题。
class FolderSnapshotStore {
  FolderSnapshotStore._();
  static final FolderSnapshotStore instance = FolderSnapshotStore._();

  final Map<String, List<FolderSnapshotEntry>> _snapshots = {};
  bool _loaded = false;
  bool _dirty = false;
  Timer? _saveTimer;

  static String keyOf(BookSource source, String path) =>
      '${source.type}|${source.id}|$path';

  /// 启动时加载（main 中 await 后 UI 才可用）。
  Future<void> load() async {
    if (_loaded) return;
    _loaded = true;
    try {
      final root = await cacheRootPath();
      final f = File('$root${Platform.pathSeparator}folder_snapshots.json');
      if (!await f.exists()) return;
      final j = jsonDecode(await f.readAsString()) as Map<String, dynamic>;
      final folders = j['folders'] as List? ?? [];
      for (final item in folders) {
        final m = Map<String, dynamic>.from(item as Map);
        final key = m['key'] as String?;
        final entries = m['entries'] as List?;
        if (key == null || entries == null) continue;
        _snapshots[key] = entries
            .map((e) => FolderSnapshotEntry.fromJson(
                Map<String, dynamic>.from(e as Map)))
            .toList();
      }
    } catch (_) {
      // 文件损坏时忽略，后续浏览会重建
      _snapshots.clear();
    }
  }

  List<FolderSnapshotEntry>? entriesFor(BookSource source, String path) =>
      _snapshots[keyOf(source, path)];

  /// 该源全部已浏览目录的快照（path → 子项），供"本地化生成离线索引"。
  /// ADR-029：零网络，只基于浏览时留下的缓存。
  Map<String, List<FolderSnapshotEntry>> foldersFor(BookSource source) {
    final prefix = '${source.type}|${source.id}|';
    return {
      for (final e in _snapshots.entries)
        if (e.key.startsWith(prefix)) e.key.substring(prefix.length): e.value,
    };
  }

  /// 反查包含该 path 的父目录。
  /// 夸克/115 等扁平路径源（path 是 fid，无层级前缀）的层级只能依赖浏览快照；
  /// 找不到返回 null（调用方回退为"挂根/路径推导"）。
  String? parentDirOf(BookSource source, String path) {
    for (final e in foldersFor(source).entries) {
      if (e.value.any((x) => x.path == path)) return e.key;
    }
    return null;
  }

  /// 浏览某目录成功后写入快照（复用同一次列表响应，不新增请求）。
  void put(BookSource source, String path, List<FolderSnapshotEntry> entries) {
    _snapshots[keyOf(source, path)] = entries;
    _dirty = true;
    _saveTimer?.cancel();
    _saveTimer = Timer(const Duration(seconds: 2), _save);
  }

  Future<void> _save() async {
    if (!_dirty) return;
    _dirty = false;
    try {
      final root = await cacheRootPath();
      final f = File('$root${Platform.pathSeparator}folder_snapshots.json');
      final payload = {
        'version': 1,
        'folders': _snapshots.entries
            .map((e) => {
                  'key': e.key,
                  'entries': e.value.map((x) => x.toJson()).toList(),
                })
            .toList(),
      };
      await f.parent.create(recursive: true);
      await f.writeAsString(jsonEncode(payload), flush: true);
    } catch (_) {
      _dirty = true; // 下次再试
    }
  }

  /// 应用退出/切后台时兜底落盘。
  Future<void> flush() async {
    _saveTimer?.cancel();
    _saveTimer = null;
    await _save();
  }
}
