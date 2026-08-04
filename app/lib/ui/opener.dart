import 'package:app/store/library_store.dart';
import 'package:app/store/models.dart';
import 'package:app/store/baidu_session.dart';
import 'package:app/store/cloud115_session.dart';
import 'package:app/store/quark_session.dart';
import 'package:app/store/sftp_session.dart';
import 'package:app/store/webdav_session.dart';
import 'package:app/ui/reader_page.dart';
import 'package:flutter/material.dart';

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
  if (source.needsSession) {
    try {
      session = source.isWebDav
          ? await webdavSessionFor(source)
          : source.isSftp
              ? await sftpSessionFor(source)
              : source.isBaidu
                  ? await baiduSessionFor(source)
                  : source.isQuark
                      ? await quarkSessionFor(source)
                      : await cloud115SessionFor(source);
    } catch (e) {
      if (context.mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(content: Text('连接远程书源失败:$e')),
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
