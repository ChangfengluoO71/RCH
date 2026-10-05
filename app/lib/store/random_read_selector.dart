import 'dart:io';
import 'dart:math';

import 'library_catalog.dart';
import 'models.dart';

typedef RandomSourceLookup = BookSource? Function(String sourceId);
typedef RandomCandidateBatchLoader = Future<List<RandomCatalogCandidate>>
    Function({
      required List<String> excludedBookKeys,
      required List<String> unavailableCandidateKeys,
      required int limit,
    });
typedef LocalComicPathExists = Future<bool> Function(String path);

class RandomReadCandidate {
  const RandomReadCandidate({
    required this.bookKey,
    required this.source,
    required this.path,
    required this.title,
  });

  final String bookKey;
  final BookSource source;
  final String path;
  final String title;
}

/// Shared random-reading policy for the home page and reader actions.
///
/// Candidate rows come from the published library index. The selector avoids
/// the last ten distinct comics when possible, checks local paths outside the
/// database call, and keeps fetching batches when an indexed path has gone
/// stale. If all remaining candidates are recent, it releases history oldest
/// first so small libraries keep cycling without getting stuck.
class RandomReadSelector {
  RandomReadSelector({Random? random, LocalComicPathExists? localPathExists})
    : _random = random ?? Random(),
      _localPathExists = localPathExists ?? _defaultLocalPathExists;

  static final instance = RandomReadSelector();

  static const recentHistoryLimit = 10;
  static const _candidateBatchSize = 32;

  final Random _random;
  final LocalComicPathExists _localPathExists;

  Future<RandomReadCandidate?> pick({
    required RandomCandidateBatchLoader loadCandidates,
    required RandomSourceLookup sourceById,
    required Iterable<String> recentBookKeys,
    required void Function(String key) recordSelection,
    BookSource? excludeSource,
    String? excludePath,
  }) async {
    final recentKeys = recentBookKeys.toList();
    final excludedCurrentKey = excludePath == null
        ? null
        : excludeSource == null
        ? null
        : bookKeyOf(excludeSource.type, excludeSource.id, excludePath);
    final unavailableKeys = <String>{};

    while (true) {
      final excludedKeys = <String>{
        ...recentKeys,
        ?excludedCurrentKey,
      }.toList();

      while (true) {
        final batch = await loadCandidates(
          excludedBookKeys: excludedKeys,
          unavailableCandidateKeys: unavailableKeys.toList(),
          limit: _candidateBatchSize,
        );
        if (batch.isEmpty) break;

        final availability = await Future.wait(
          batch.map((candidate) async {
            final source = sourceById(candidate.sourceId);
            if (source == null ||
                source.type != candidate.sourceType ||
                source.remoteOnly) {
              return null;
            }
            if (source.needsSession) {
              return RandomReadCandidate(
                bookKey: candidate.bookKey,
                source: source,
                path: candidate.path,
                title: candidate.title,
              );
            }
            try {
              if (await _localPathExists(candidate.path)) {
                return RandomReadCandidate(
                  bookKey: candidate.bookKey,
                  source: source,
                  path: candidate.path,
                  title: candidate.title,
                );
              }
            } catch (_) {
              // A stale or unreadable path should not invalidate other books.
            }
            return null;
          }),
        );

        final usable = <RandomReadCandidate>[];
        for (var i = 0; i < batch.length; i++) {
          final candidate = availability[i];
          if (candidate == null) {
            unavailableKeys.add(batch[i].bookKey);
          } else {
            usable.add(candidate);
          }
        }
        if (usable.isNotEmpty) {
          final selected = usable[_random.nextInt(usable.length)];
          recordSelection(selected.bookKey);
          return selected;
        }
      }

      // The catalog has no remaining candidate outside the current exclusions.
      // Release only its oldest recent key; the active book remains excluded.
      if (recentKeys.isEmpty) return null;
      recentKeys.removeAt(0);
    }
  }

  static Future<bool> _defaultLocalPathExists(String path) async {
    if (await File(path).exists()) return true;
    return Directory(path).exists();
  }
}
