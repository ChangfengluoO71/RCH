import 'package:app/src/rust/api/book.dart';
import 'package:app/store/models.dart';

typedef BatchTagDirectoryLister = Future<List<DirEntry>> Function(String path);
typedef BatchTagComicFolderChecker = Future<bool> Function(String path);

/// Returns matching tags with exact matches first, then prefixes, then
/// substring matches. Ties are sorted alphabetically for predictable results.
List<String> rankBatchTagSuggestions(String query, Iterable<String> tags) {
  final normalizedQuery = query.trim().toLowerCase();
  if (normalizedQuery.isEmpty) return const <String>[];

  final matches = tags
      .where((tag) => tag.toLowerCase().contains(normalizedQuery))
      .toList();

  int priority(String tag) {
    final normalizedTag = tag.toLowerCase();
    if (normalizedTag == normalizedQuery) return 0;
    if (normalizedTag.startsWith(normalizedQuery)) return 1;
    return 2;
  }

  matches.sort((a, b) {
    final priorityComparison = priority(a).compareTo(priority(b));
    if (priorityComparison != 0) return priorityComparison;
    final caseInsensitiveComparison = a.toLowerCase().compareTo(
      b.toLowerCase(),
    );
    if (caseInsensitiveComparison != 0) return caseInsensitiveComparison;
    return a.compareTo(b);
  });
  return matches;
}

/// Reuses an in-flight or completed comic-folder check for the same path.
class MemoizedBatchTagComicFolderChecker {
  final BatchTagComicFolderChecker _check;
  final Map<String, Future<bool>> _cache = {};

  MemoizedBatchTagComicFolderChecker(this._check);

  Future<bool> call(String path) =>
      _cache.putIfAbsent(path, () => _check(path));

  void clear() => _cache.clear();
}

/// Expands the current selection into concrete file or comic-folder targets.
///
/// A local directory containing images is itself the comic item. It must be
/// returned before walking its children, because image files are intentionally
/// not treated as standalone comic entries by the browser.
Future<List<BatchTagTarget>> collectBatchTagTargets({
  required Iterable<String> selectedPaths,
  required Iterable<DirEntry> currentEntries,
  required String effectiveRootPath,
  required bool isLocalFs,
  required BatchTagDirectoryLister listDirectory,
  required BatchTagComicFolderChecker isComicFolder,
  required bool Function(DirEntry entry) isComicEntry,
}) async {
  final entries = currentEntries.toList(growable: false);
  final result = <BatchTagTarget>[];
  for (final path in selectedPaths) {
    final isDir = entries.any((entry) => entry.path == path && entry.isDir);
    final isHiddenFolder =
        !entries.any((entry) => entry.path == path) &&
        path != effectiveRootPath;
    if (isDir || isHiddenFolder) {
      result.addAll(
        await _collectFromDirectory(
          dirPath: path,
          isLocalFs: isLocalFs,
          listDirectory: listDirectory,
          isComicFolder: isComicFolder,
          isComicEntry: isComicEntry,
        ),
      );
    } else {
      result.add(BatchTagTarget.file(path));
    }
  }
  return result;
}

Future<List<BatchTagTarget>> _collectFromDirectory({
  required String dirPath,
  required bool isLocalFs,
  required BatchTagDirectoryLister listDirectory,
  required BatchTagComicFolderChecker isComicFolder,
  required bool Function(DirEntry entry) isComicEntry,
}) async {
  if (isLocalFs && await isComicFolder(dirPath)) {
    return [BatchTagTarget.directory(dirPath)];
  }

  final result = <BatchTagTarget>[];
  final pending = <String>[dirPath];
  while (pending.isNotEmpty) {
    final path = pending.removeAt(0);
    try {
      final entries = await listDirectory(path);
      for (final entry in entries) {
        if (entry.isDir) {
          if (isLocalFs && await isComicFolder(entry.path)) {
            result.add(BatchTagTarget.directory(entry.path));
          } else {
            pending.add(entry.path);
          }
        } else if (isComicEntry(entry)) {
          result.add(BatchTagTarget.file(entry.path));
        }
      }
    } catch (_) {
      // Skip directories that cannot be read and keep the rest of the batch.
    }
  }
  return result;
}
