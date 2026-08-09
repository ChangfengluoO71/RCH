// Library Index 生成服务（ADR-020/021）。
//
// 职责：把书源的物理资产（本地目录树 / 云端目录树）扫描成 library_index
// 条目并落 SQLite。只写 path/size/mtime/cover_ref（物理发现层），
// 绝不触碰 book_metas / read_records（用户认知层）。

import 'dart:convert';
import 'dart:io';

import 'package:app/src/rust/api/db.dart' as frb;
import 'package:app/src/rust/api/library.dart' as frblib;
import 'package:app/store/folder_snapshot_store.dart';
import 'package:app/store/models.dart';
import 'package:crypto/crypto.dart';

class LibraryIndexService {
  LibraryIndexService._();
  static final LibraryIndexService instance = LibraryIndexService._();

  /// 与 SourceBrowser 一致的漫画扩展名清单。
  static const List<String> comicExts = [
    '.cbz', '.zip', '.epub', '.cb7', '.7z', '.cbt', '.tar',
    '.pdf', '.cbr', '.rar', '.mobi', '.azw', '.azw3',
  ];

  static bool isComicPath(String path) {
    final lower = path.toLowerCase();
    return comicExts.any(lower.endsWith);
  }

  /// 索引路径规范化（ADR-028 §12.4）：`\`→`/`、Windows 盘符小写、去尾斜杠（根 `/` 保留）。
  /// 必须与 Rust `db::normalize_index_path` 完全一致，否则两端 book_id/parent_id 不一致。
  static String normalizeIndexPath(String path) {
    var s = path.trim().replaceAll('\\', '/');
    while (s.endsWith('/') && s.length > 1) {
      s = s.substring(0, s.length - 1);
    }
    if (s.length >= 2 && s.codeUnitAt(1) == 0x3A /* ':' */) {
      final first = s.substring(0, 1).toLowerCase();
      s = '$first${s.substring(1)}';
    }
    return s;
  }

  /// 稳定条目 id：`sha256(fingerprint + "|" + normalizeIndexPath(path))`，
  /// 与 Rust `db::library_index_id` 一致。
  static String libraryIndexId(String fingerprint, String path) =>
      sha256
          .convert(utf8.encode('$fingerprint|${normalizeIndexPath(path)}'))
          .toString();

  /// 目录变化指纹：按 path 排序聚合条目字段的 sha256（判断是否需要重新写入）。
  static String rootHashOf(List<frb.LibraryIndexDto> entries) {
    final lines = entries
        // 含条目 id：book_id 规范化规则变更（ADR-028 §12.4）后旧 id 不再匹配，
        // rootHash 变化会触发一次重写，自动把旧 raw-path id 迁移为新 id。
        .map((e) =>
            '${e.id}|${e.path}|${e.entryType}|${e.name}|${e.size}|${e.modifiedAt}')
        .toList()
      ..sort();
    return sha256.convert(utf8.encode(lines.join('\n'))).toString();
  }

  /// 触及即补：缓存/已读/标签过的书自动入离线索引（本地 upsert，零网络）。
  /// ADR-029：即使未生成全量索引，被标记/读/缓存过的书也能出现在离线列表并随同步传播。
  static Future<void> ensureIndexed(
    BookSource source,
    String path, {
    String? name,
    String entryType = 'file',
  }) async {
    try {
      await frb.dbEnsureIndexEntry(
        sourceId: source.id,
        path: path,
        entryType: entryType,
        name: name ?? path.split(RegExp(r'[\\/]')).last,
        // ADR-029：扁平路径源（夸克/115）从浏览快照反查父目录，避免层级挂错；
        // 层级路径源无快照时回退为 None（Rust 从 path 推导）。
        parentPath: FolderSnapshotStore.instance.parentDirOf(source, path),
      );
    } catch (_) {
      // 补条目失败不影响主流程
    }
  }

  /// 浏览即索引：把在线浏览到的目录直接子项写入索引（含父链；本地，零网络）。
  /// 用户浏览到哪里，离线索引就积累到哪里，无需手动全量生成。
  static Future<void> indexDirSnapshot(
    BookSource source,
    String path,
    List<FolderSnapshotEntry> entries,
  ) async {
    try {
      await frb.dbEnsureIndexEntries(
        sourceId: source.id,
        entries: entries
            .map((e) => frb.IndexEntryInput(
                  path: e.path,
                  entryType: e.isDir ? 'dir' : 'file',
                  name: e.name,
                  size: null,
                  modifiedAt: null,
                  // ADR-029：显式父目录 = 浏览时的当前目录（扁平路径源关键）
                  parentPath: path,
                ))
            .toList(),
      );
    } catch (_) {}
  }

  /// 本地化生成：从 FolderSnapshotStore 已有浏览快照构建索引（零网络请求）。
  /// ADR-029：替代"全量爬云端树"的默认行为；未浏览过的目录保持在线浏览。
  static Future<int> buildIndexFromSnapshots(BookSource source) async {
    final folders = FolderSnapshotStore.instance.foldersFor(source);
    var n = 0;
    for (final e in folders.entries) {
      await indexDirSnapshot(source, e.key, e.value);
      n += e.value.length;
    }
    return n;
  }

  /// 条目元数据哈希（Phase 5.0）：sha256(path|name|type|size|mtime)，
  /// 用于增量检测与"同 path 不同 metadata → LWW"判定；**不是漫画内容哈希**。
  static String entryHashOf({
    required String path,
    required String name,
    required String entryType,
    int? size,
    int? modifiedAt,
  }) =>
      sha256
          .convert(utf8.encode(
              '${normalizeIndexPath(path)}|$name|$entryType|$size|$modifiedAt'))
          .toString();

  /// 扫描本地/SMB 目录树 → 索引条目（根目录本身不入库）。
  ///
  /// - `previous`：上一次索引（可选）。非 force 时，目录 mtime 未变的子树跳过遍历，
  ///   旧条目原样保留（增量；新增/删除/改名会改变目录 mtime，内容编辑需 force 全量）。
  static Future<List<frb.LibraryIndexDto>> scanLocalSource({
    required String sourceId,
    required String fingerprint,
    required String rootPath,
    List<frb.LibraryIndexDto>? previous,
    bool force = false,
  }) async {
    final now = DateTime.now().millisecondsSinceEpoch;
    final entries = <frb.LibraryIndexDto>[];
    final root = Directory(rootPath);
    if (!await root.exists()) return entries;
    final rootId = libraryIndexId(fingerprint, rootPath);
    final prevDirs = <String, int>{
      for (final e in previous ?? const <frb.LibraryIndexDto>[])
        if (e.entryType == 'dir' && e.modifiedAt != null) e.path: e.modifiedAt!,
    };

    Future<void> walk(Directory dir, String parentId) async {
      final dirPath = dir.path;
      final isRoot = dirPath == root.path;
      String myId = rootId;
      if (!isRoot) {
        myId = libraryIndexId(fingerprint, dirPath);
        if (!force && prevDirs.containsKey(dirPath)) {
          final st = await dir.stat();
          if (st.modified.millisecondsSinceEpoch == prevDirs[dirPath]) {
            // 子树未变：原样保留旧条目（含 dir 自身与子级），不再遍历
            for (final e in previous ?? const <frb.LibraryIndexDto>[]) {
              if (e.path == dirPath ||
                  e.path.startsWith('$dirPath${Platform.pathSeparator}')) {
                entries.add(e);
              }
            }
            return;
          }
        }
        final dirStat = await dir.stat();
        final dirMtime = dirStat.modified.millisecondsSinceEpoch;
        final name = dirPath.split(Platform.pathSeparator).last;
        entries.add(frb.LibraryIndexDto(
          id: myId,
          sourceId: sourceId,
          parentId: parentId,
          name: name,
          path: dirPath,
          entryType: 'dir',
          size: null,
          modifiedAt: dirMtime,
          coverPath: null,
          hash: entryHashOf(
            path: dirPath,
            name: name,
            entryType: 'dir',
            modifiedAt: dirMtime,
          ),
          updatedAt: now,
          deleted: false,
        ));
      }
      await for (final e in dir.list(followLinks: false)) {
        final path = e.path;
        final name = path.split(Platform.pathSeparator).last;
        if (e is Directory) {
          await walk(e, myId);
        } else if (e is File && isComicPath(path)) {
          final st = await e.stat();
          entries.add(frb.LibraryIndexDto(
            id: libraryIndexId(fingerprint, path),
            sourceId: sourceId,
            parentId: myId,
            name: name,
            path: path,
            entryType: 'file',
            size: st.size,
            modifiedAt: st.modified.millisecondsSinceEpoch,
            coverPath: null,
            hash: entryHashOf(
              path: path,
              name: name,
              entryType: 'file',
              size: st.size,
              modifiedAt: st.modified.millisecondsSinceEpoch,
            ),
            updatedAt: now,
            deleted: false,
          ));
        }
      }
    }

    await walk(root, rootId);
    return entries;
  }

  /// 枚举云端书源目录树（BFS）。
  /// - 复用 FolderSnapshotStore：非 force 时已缓存目录不发请求（增量）；
  /// - 每目录 250ms 节流，规避 WAF/限流（115 教训）。
  static Future<List<frb.LibraryIndexDto>> crawlRemoteSource({
    required BookSource source,
    required String fingerprint,
    required Future<List<FolderSnapshotEntry>> Function(String path) listRemote,
    bool force = false,
  }) async {
    final now = DateTime.now().millisecondsSinceEpoch;
    final entries = <frb.LibraryIndexDto>[];
    final visited = <String>{};
    final queue = <String>[source.path.isEmpty ? '/' : source.path];

    while (queue.isNotEmpty) {
      final dirPath = queue.removeAt(0);
      if (visited.contains(dirPath)) continue;
      visited.add(dirPath);

      final cached = FolderSnapshotStore.instance.entriesFor(source, dirPath);
      List<FolderSnapshotEntry> list;
      if (!force && cached != null) {
        list = cached;
      } else {
        list = await listRemote(dirPath);
        FolderSnapshotStore.instance.put(source, dirPath, list);
        await Future<void>.delayed(const Duration(milliseconds: 250));
      }

      final parentId = libraryIndexId(fingerprint, dirPath);
      for (final e in list) {
        final id = libraryIndexId(fingerprint, e.path);
        if (e.isDir) {
          entries.add(frb.LibraryIndexDto(
            id: id,
            sourceId: source.id,
            parentId: parentId,
            name: e.name,
            path: e.path,
            entryType: 'dir',
            size: null,
            modifiedAt: null,
            coverPath: null,
            hash: entryHashOf(path: e.path, name: e.name, entryType: 'dir'),
            updatedAt: now,
            deleted: false,
          ));
          if (!visited.contains(e.path)) queue.add(e.path);
        } else if (isComicPath(e.path)) {
          entries.add(frb.LibraryIndexDto(
            id: id,
            sourceId: source.id,
            parentId: parentId,
            name: e.name,
            path: e.path,
            entryType: 'file',
            size: null,
            modifiedAt: null,
            coverPath: null,
            hash: entryHashOf(path: e.path, name: e.name, entryType: 'file'),
            updatedAt: now,
            deleted: false,
          ));
        }
      }
    }
    return entries;
  }

  /// 刷新某书源目录索引。
  ///
  /// - 本地/SMB：直接扫描文件树；
  /// - 云端：`listRemote` 提供目录列表（调用方按书源类型建会话）；
  /// - `force=false` 且 root_hash 未变 → 不写库直接返回"无变化"。
  Future<({int entries, String message})> refreshSourceIndex({
    required BookSource source,
    bool force = false,
    Future<List<FolderSnapshotEntry>> Function(String path)? listRemote,
  }) async {
    final fp = await frb.dbGetSourceFingerprint(sourceId: source.id);
    if (fp == null || fp.isEmpty) {
      return (entries: 0, message: '书源缺少 fingerprint，无法建立索引');
    }

    final List<frb.LibraryIndexDto> entries;
    if (source.isLocalFs) {
      final previous = await frb.dbLoadLibraryIndexForSource(sourceId: source.id);
      // ADR-028 §12.4 迁移：旧 raw-path id 与新规范化 id 不一致 → 强制全量重扫，
      // 否则增量扫描会原样保留旧 id 子树，两端 book_id/parent_id 仍对不上。
      final schemeChanged = previous.isNotEmpty &&
          previous.any((e) => e.id != libraryIndexId(fp, e.path));
      entries = await scanLocalSource(
        sourceId: source.id,
        fingerprint: fp,
        rootPath: source.path,
        previous: previous,
        force: force || schemeChanged,
      );
    } else {
      if (listRemote == null) {
        return (entries: 0, message: '云端书源需要目录列表回调');
      }
      entries = await crawlRemoteSource(
        source: source,
        fingerprint: fp,
        listRemote: listRemote,
        force: force,
      );
    }

    final rootHash = rootHashOf(entries);
    if (!force) {
      final snap = await frb.dbGetSourceSnapshot(sourceId: source.id);
      final live = await frblib.dbSourceIndexCount(sourceId: source.id);
      // ADR-028 §12.5：rootHash 相同但 live 行数对不上（例如同步墓碑软删后）
      // 必须重写索引，否则本地索引永远停留在"已删除"状态。
      if (snap?.rootHash == rootHash && live == entries.length) {
        return (entries: 0, message: '目录无变化');
      }
    }

    await frb.dbReplaceSourceLibraryIndex(sourceId: source.id, entries: entries);
    await frb.dbSetSourceSnapshot(
      sourceId: source.id,
      lastScanTime: DateTime.now().millisecondsSinceEpoch,
      entryCount: entries.length,
      rootHash: rootHash,
    );
    return (entries: entries.length, message: '已索引 ${entries.length} 个条目');
  }
}
