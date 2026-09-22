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
/// 会话缓存：按书源 id 保存**已建立**的会话 ✓。
///
/// 2026-09-22（用户反馈"点进云端书源要加载一会儿"）：
/// 启动预热（`RemoteScanCoordinator.warmUpSessions`）本已建立过会话 ✓，但浏览器打开时
/// 仍然会重新走一次完整登录 ✗ —— 而一次登录 = PROPFIND(Depth:0) + Range 探测 + RTT 取样，
/// 实测在局域网 WebDAV 上首包就要 ~1.8s ✗。这里把会话缓存起来供复用：
/// 预热填充它 ✓、浏览器直接命中它 ✓、连接失败时由调用方清除 ✓（自愈）。
final Map<String, BigInt> _sessionCache = {};

/// 记录一个已建立的会话（浏览器自己登录成功后写回，供之后进源直接命中）。
void cacheRemoteSession(String sourceId, BigInt session) =>
    _sessionCache[sourceId] = session;

/// 清除某个书源的会话缓存（凭据变更、连接失败、断开时调用，避免复用失效会话）。
void evictRemoteSession(String sourceId) => _sessionCache.remove(sourceId);

/// 复用预热阶段已经建立的会话（若存在），避免重复登录。
Future<BigInt?> cachedRemoteSession(String sourceId) async => _sessionCache[sourceId];

Future<BigInt?> remoteSessionFor(BookSource source) async {
  final cached = _sessionCache[source.id];
  if (cached != null) return cached; // 预热/上次浏览已建会话 ⇒ 直接复用（不再登录）
  final session = switch (source.type) {
    'webdav' => await webdavSessionFor(source),
    'sftp' => await sftpSessionFor(source),
    'baidu' => await baiduSessionFor(source),
    '115' => await cloud115SessionFor(source),
    'quark' => await quarkSessionFor(source),
    _ => null,
  };
  if (session != null) _sessionCache[source.id] = session;
  return session;
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
      .map(
        (e) => FolderSnapshotEntry(
          name: e.name,
          path: e.path,
          isDir: e.isDir,
          // 2026-09-22：此前丢掉 size ✗ ⇒ 索引写 null ⇒ cover_size_missing。现按远端列表原样带上。
          size: e.size.toInt(),
        ),
      )
      .toList();
}
