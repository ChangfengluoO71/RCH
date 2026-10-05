import 'package:app/store/models.dart';
import 'package:app/store/library_store.dart';
import 'package:app/ui/reader_page.dart';
import 'package:flutter/material.dart';

/// Open a book, allowing the reader page to show connection and loading state.
Future<void> openBook(
  BuildContext context,
  BookSource source,
  String path,
  String title,
) async =>
    _open(context, source, path, title, false);

/// Open a book without using an AI-upscaled page cache.
Future<void> openBookNoAi(
  BuildContext context,
  BookSource source,
  String path,
  String title,
) async =>
    _open(context, source, path, title, true);

Future<void> _open(
  BuildContext context,
  BookSource source,
  String path,
  String title,
  bool skipAiCache,
) async {
  final store = LibraryStore.instance;
  final initialPage = store.recordOf(source, path)?.lastPage ?? 0;
  if (!context.mounted) return;
  Navigator.of(context).push(
    MaterialPageRoute(
      builder: (_) => ReaderPage(
        path: path,
        title: title,
        source: source,
        initialPage: initialPage,
        skipAiCache: skipAiCache,
      ),
    ),
  );
}
