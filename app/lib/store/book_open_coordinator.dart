import 'dart:async';

import 'package:app/src/rust/api/book.dart' as frbbook;
import 'package:app/src/rust/api/source.dart' as frbsource;
import 'package:app/store/baidu_session.dart';
import 'package:app/store/cloud115_session.dart';
import 'package:app/store/library_catalog.dart';
import 'package:app/store/models.dart';
import 'package:app/store/quark_session.dart';
import 'package:app/store/sftp_session.dart';
import 'package:app/store/webdav_session.dart';

enum BookOpenStage { connecting, openingBook, loadingFirstPage, ready, failed }

class BookOpenResult {
  const BookOpenResult({
    required this.book,
    required this.remoteImageFolder,
    required this.providerPath,
  });

  final frbbook.BookInfo book;
  final bool remoteImageFolder;
  final String providerPath;
}

class BookOpenCancelled implements Exception {
  const BookOpenCancelled();
}

/// Owns provider session setup and native book opening without holding a
/// widget, BuildContext, or Navigator reference.
class BookOpenCoordinator {
  BookOpenCoordinator._();

  static final instance = BookOpenCoordinator._();

  Future<BookOpenResult> open({
    required BookSource? source,
    required String path,
    required String title,
    required BookOpenStrategy strategy,
    required bool Function() isActive,
    required void Function(BookOpenStage stage) onStage,
    required void Function(double progress) onProgress,
    void Function(bool remoteImageFolder)? onRemoteImageFolder,
    void Function(String providerPath)? onProviderPathResolved,
    BigInt? existingSession,
  }) async {
    final sourceType =
        source?.type ?? (existingSession == null ? 'local' : 'webdav');
    void stage(BookOpenStage value) {
      if (isActive()) onStage(value);
    }

    if (source?.remoteOnly == true) {
      throw StateError('此书源来自其他设备，仅有索引，当前设备无法打开');
    }

    var session = existingSession;
    if (source?.needsSession == true) {
      stage(BookOpenStage.connecting);
      final watch = Stopwatch()..start();
      try {
        session = await _sessionFor(source!);
        await _logTiming(
          stage: 'connect',
          sourceType: sourceType,
          result: 'success',
          elapsed: watch.elapsedMilliseconds,
        );
      } catch (_) {
        await _logTiming(
          stage: 'connect',
          sourceType: sourceType,
          result: 'error',
          elapsed: watch.elapsedMilliseconds,
        );
        rethrow;
      }
      if (!isActive()) throw const BookOpenCancelled();
    }

    final openWatch = Stopwatch()..start();
    try {
      var remoteImageFolder = false;
      if (source != null &&
          source.needsSession &&
          session != null &&
          !source.remoteOnly) {
        remoteImageFolder = await frbsource.remoteImageFolderManifestComplete(
          sourceId: source.id,
          path: path,
        );
      }
      if (!isActive()) throw const BookOpenCancelled();
      stage(BookOpenStage.openingBook);
      onRemoteImageFolder?.call(remoteImageFolder);

      final providerPath = source != null &&
              source.needsSession &&
              !remoteImageFolder
          ? await LibraryCatalogStore.instance.providerPathFor(
              source: source,
              path: path,
            )
          : path;
      if (!isActive()) throw const BookOpenCancelled();
      onProviderPathResolved?.call(providerPath);

      final book = await _openProvider(
        source: source,
        sourceType: sourceType,
        session: session,
        path: path,
        providerPath: providerPath,
        title: title,
        strategy: strategy,
        remoteImageFolder: remoteImageFolder,
        isActive: isActive,
        onProgress: onProgress,
      );
      await _logTiming(
        stage: 'book_open',
        sourceType: sourceType,
        result: 'success',
        elapsed: openWatch.elapsedMilliseconds,
      );
      if (!isActive()) {
        await _closeLateBook(book);
        throw const BookOpenCancelled();
      }
      stage(BookOpenStage.loadingFirstPage);
      return BookOpenResult(
        book: book,
        remoteImageFolder: remoteImageFolder,
        providerPath: providerPath,
      );
    } catch (error) {
      if (error is! BookOpenCancelled) {
        await _logTiming(
          stage: 'book_open',
          sourceType: sourceType,
          result: 'error',
          elapsed: openWatch.elapsedMilliseconds,
        );
      }
      rethrow;
    }
  }

  Future<BigInt> _sessionFor(BookSource source) {
    if (source.isWebDav) return webdavSessionFor(source);
    if (source.isSftp) return sftpSessionFor(source);
    if (source.isBaidu) return baiduSessionFor(source);
    if (source.is115) return cloud115SessionFor(source);
    if (source.isQuark) return quarkSessionFor(source);
    throw UnsupportedError('不支持的远程书源类型：${source.type}');
  }

  Future<frbbook.BookInfo> _openProvider({
    required BookSource? source,
    required String sourceType,
    required BigInt? session,
    required String path,
    required String providerPath,
    required String title,
    required BookOpenStrategy strategy,
    required bool remoteImageFolder,
    required bool Function() isActive,
    required void Function(double progress) onProgress,
  }) async {
    if (source == null && session == null) {
      return frbbook.openLocalBook(path: path);
    }
    if (source?.isLocalFs == true) {
      return frbbook.openLocalBook(path: path);
    }
    if (session == null) {
      throw StateError('书源连接尚未建立');
    }
    if (remoteImageFolder && source != null) {
      return frbsource.openRemoteFolderBook(
        sourceType: source.type,
        sourceId: source.id,
        session: session,
        path: path,
        title: title,
      );
    }

    late final Future<frbbook.BookInfo> Function() open;
    Future<double> Function()? progress;
    if (sourceType == 'webdav') {
      open = () => frbsource.openWebdavBook(
        session: session,
        path: providerPath,
        strategy: strategy.name,
      );
      progress = () => frbsource.webdavDownloadProgress(session: session);
    } else if (source?.isSftp == true) {
      open = () => frbsource.openSftpBook(
        session: session,
        path: providerPath,
        strategy: strategy.name,
      );
      progress = () => frbsource.sftpDownloadProgress(session: session);
    } else if (source?.isBaidu == true) {
      open = () => frbsource.openBaiduBook(
        session: session,
        path: providerPath,
        strategy: strategy.name,
      );
      progress = () => frbsource.baiduDownloadProgress(session: session);
    } else if (source?.is115 == true) {
      open = () => openCloud115BookFor(
        source!,
        session: session,
        path: providerPath,
        strategy: strategy.name,
      );
      progress = () => cloud115DownloadProgressFor(source!, session: session);
    } else if (source?.isQuark == true) {
      open = () => frbsource.openQuarkBook(
        session: session,
        path: providerPath,
        strategy: strategy.name,
      );
      progress = () => frbsource.quarkDownloadProgress(session: session);
    } else {
      throw UnsupportedError('不支持的书源类型：$sourceType');
    }

    final progressProvider = strategy == BookOpenStrategy.stream
        ? null
        : progress;
    var polling = progressProvider != null;
    final pollTask = _pollProgress(
      progress: progressProvider,
      onProgress: onProgress,
      isActive: isActive,
      shouldContinue: () => polling,
    );
    try {
      return await open();
    } finally {
      polling = false;
      unawaited(pollTask);
    }
  }

  Future<void> _pollProgress({
    required Future<double> Function()? progress,
    required void Function(double progress) onProgress,
    required bool Function() isActive,
    required bool Function() shouldContinue,
  }) async {
    if (progress == null) return;
    while (shouldContinue() && isActive()) {
      await Future<void>.delayed(const Duration(milliseconds: 300));
      if (!shouldContinue() || !isActive()) return;
      try {
        final value = await progress();
        // Providers report 1.0 when no download is active. Treat that as the
        // idle sentinel so auto/stream opens do not flash a false 100% bar.
        if (isActive() && value.isFinite && value >= 0.0 && value < 1.0) {
          onProgress(value);
        }
      } catch (_) {
        // Progress reporting is best effort; the open request owns failure.
      }
    }
  }

  Future<void> _logTiming({
    required String stage,
    required String sourceType,
    required String result,
    required int elapsed,
  }) async {
    try {
      await frbsource.logReaderTiming(
        stage: stage,
        sourceType: sourceType,
        result: result,
        elapsedMs: elapsed,
      );
    } catch (_) {
      // Diagnostics must never block opening a book.
    }
  }

  Future<void> _closeLateBook(frbbook.BookInfo book) async {
    try {
      await frbbook.closeBook(handle: book.handle);
    } catch (_) {
      // The handle is already stale or closed; do not surface cleanup noise.
    }
  }
}
