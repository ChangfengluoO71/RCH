// 远程书源目录列表与会话编排（清理失效数据 / 全量重建索引共用）。
//
// 职责：按书源类型建立会话、列出目录并转换为 FolderSnapshotEntry。
// 单独成文件以避免 store 层之间的循环 import：
// LibraryStore / SourceBrowser 都需要这份编排，而 session store 会 import LibraryStore。
//
// 与 source_browser.dart 中"全量重建索引（联网）"的 listRemote 回调同构；
// 任何新增书源类型只需在此扩展 switch。

import 'package:app/src/rust/api/source.dart';
import 'package:app/store/baidu_session.dart';
import 'package:app/store/cloud115_session.dart';
import 'package:app/store/folder_snapshot_store.dart';
import 'package:app/store/models.dart';
import 'package:app/store/quark_session.dart';
import 'package:app/store/sftp_session.dart';
import 'package:app/store/webdav_session.dart';

/// 按书源类型建立会话；未知类型返回 null（调用方视为不可用，跳过该源）。
/// 会话建立失败以异常上抛，由调用方捕获降级。
Future<BigInt?> remoteSessionFor(BookSource source) async {
  return switch (source.type) {
    'webdav' => await webdavSessionFor(source),
    'sftp' => await sftpSessionFor(source),
    'baidu' => await baiduSessionFor(source),
    '115' => await cloud115SessionFor(source),
    'quark' => await quarkSessionFor(source),
    _ => null,
  };
}

/// 按书源类型列出远程目录（一个目录一条 list 请求），转为离线索引统一结构。
Future<List<FolderSnapshotEntry>> listRemoteDirFor(
  BookSource source, {
  required BigInt session,
  required String path,
}) async {
  final list = switch (source.type) {
    'webdav' => await webdavList(session: session, path: path),
    'sftp' => await sftpList(session: session, path: path),
    'baidu' => await baiduList(session: session, path: path),
    '115' => await cloud115ListFor(source, session: session, path: path),
    'quark' => await quarkList(session: session, path: path),
    _ => null,
  };
  if (list == null) return const [];
  return list
      .map((e) => FolderSnapshotEntry(name: e.name, path: e.path, isDir: e.isDir))
      .toList();
}