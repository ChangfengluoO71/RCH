import 'package:app/src/rust/api/source.dart';
import 'package:app/store/library_store.dart';
import 'package:app/store/models.dart';
import 'package:app/ui/reader_page.dart';
import 'package:flutter/material.dart';

/// WebDAV 会话缓存(按书源 id),避免每次打开都重新连接。
final Map<String, BigInt> _webdavSessions = {};

/// 获取/重连某 WebDAV 书源的会话(带缓存,避免每次打开都重连)。
Future<BigInt> webdavSessionFor(BookSource source) async {
  final cached = _webdavSessions[source.id];
  if (cached != null) return cached;
  final s = await webdavConnect(
    url: source.url ?? '',
    username: source.username ?? '',
    password: source.password ?? '',
  );
  _webdavSessions[source.id] = s.id;
  // 保存探测到的能力标记
  if (s.capabilityLabel.isNotEmpty && source.capabilityLabel.isEmpty) {
    source.capabilityLabel = s.capabilityLabel;
    LibraryStore.instance.updateSourceCapability(source.id, s.capabilityLabel);
  }
  return s.id;
}

/// 打开一本书（使用 AI 超分版本，如果缓存存在）。
Future<void> openBook(
  BuildContext context,
  BookSource source,
  String path,
  String title,
) async => _open(context, source, path, title, false);

/// 打开一本书（不使用 AI 超分缓存，始终读原始版本）。
Future<void> openBookNoAi(
  BuildContext context,
  BookSource source,
  String path,
  String title,
) async => _open(context, source, path, title, true);

Future<void> _open(
  BuildContext context,
  BookSource source,
  String path,
  String title,
  bool skipAiCache,
) async {
  BigInt? session;
  if (source.isWebDav) {
    try {
      session = await webdavSessionFor(source);
    } catch (e) {
      if (context.mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(content: Text('连接 WebDAV 失败:$e')),
        );
      }
      return;
    }
  }
  final store = LibraryStore.instance;
  // 记录一次"打开"(readCount+1),并取出上次进度。
  await store.recordRead(source: source, path: path, title: title);
  final initialPage = store.recordOf(source, path)?.lastPage ?? 0;
  if (!context.mounted) return;
  Navigator.of(context).push(
    MaterialPageRoute(
      builder: (_) => ReaderPage(
        path: path,
        title: title,
        webdavSession: session,
        source: source,
        initialPage: initialPage,
        skipAiCache: skipAiCache,
      ),
    ),
  );
}
