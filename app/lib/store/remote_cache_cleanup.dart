// The page count is mutable because a Reader is initialized before its remote
// document handle is available; retain the named constructor API for callers.
// ignore_for_file: prefer_initializing_formals

import 'dart:async';

import '../src/rust/api/cache.dart';
import 'models.dart';

/// Completion is intentionally separate from the currently visible list item.
/// A transient layout estimate must not mark a book complete, and navigating
/// back from the final page clears the candidate before Reader exits.
class ReadingCompletionState {
  ReadingCompletionState({required int pageCount, int initialPage = 0})
    : _pageCount = pageCount,
      _stablePage = initialPage;

  int _pageCount;
  int _stablePage;
  bool _candidate = false;

  int get stablePage => _stablePage;
  bool get completionCandidate => _candidate;

  void reset({required int pageCount, int initialPage = 0}) {
    _pageCount = pageCount;
    _stablePage = initialPage.clamp(0, pageCount > 0 ? pageCount - 1 : 0);
    _candidate = false;
  }

  void observeStablePage(int page) {
    if (_pageCount <= 0) return;
    _stablePage = page.clamp(0, _pageCount - 1);
    if (_stablePage == _pageCount - 1) {
      _candidate = true;
    } else if (_stablePage < _pageCount - 1) {
      _candidate = false;
    }
  }
}

class RemoteBookCleanupResult {
  const RemoteBookCleanupResult({required this.freedBytes, this.error});

  final BigInt freedBytes;
  final Object? error;
  bool get succeeded => error == null;
}

typedef RemoteBookContentCleanup =
    Future<BigInt> Function(BookSource source, String path, bool imageFolder);

class VerifiedRemoteTombstone {
  const VerifiedRemoteTombstone({
    required this.logicalPath,
    required this.dependencyPaths,
  });

  final String logicalPath;
  final List<String> dependencyPaths;
}

typedef VerifiedRemoteAssetCleanup =
    Future<BigInt> Function(
      String sourceId,
      String logicalPath,
      List<String> dependencyPaths,
    );

typedef VerifiedRemoteCleanupError =
    void Function(VerifiedRemoteTombstone tombstone, Object error);

class VerifiedRemoteTombstoneCleanupResult {
  VerifiedRemoteTombstoneCleanupResult({
    required this.freedBytes,
    required Iterable<VerifiedRemoteTombstone> verifiedTombstones,
  }) : verifiedTombstones = List.unmodifiable(verifiedTombstones);

  final BigInt freedBytes;
  final List<VerifiedRemoteTombstone> verifiedTombstones;
}

Future<BigInt> purgeVerifiedRemoteTombstones({
  required BookSource source,
  required Iterable<VerifiedRemoteTombstone> tombstones,
  required VerifiedRemoteAssetCleanup cleanup,
}) async {
  var freed = BigInt.zero;
  for (final tombstone in tombstones) {
    freed += await cleanup(
      source.id,
      tombstone.logicalPath,
      List<String>.unmodifiable(tombstone.dependencyPaths),
    );
  }
  return freed;
}

/// Re-verifies each candidate through [cleanup] and returns only the
/// tombstones whose proof-aware Rust purge completed. Callers must use the
/// returned list, rather than the candidates, for destructive catalog work.
Future<VerifiedRemoteTombstoneCleanupResult>
purgeVerifiedRemoteTombstonesSafely({
  required String sourceId,
  required Iterable<VerifiedRemoteTombstone> tombstones,
  required VerifiedRemoteAssetCleanup cleanup,
  VerifiedRemoteCleanupError? onError,
}) async {
  var freed = BigInt.zero;
  final verified = <VerifiedRemoteTombstone>[];
  for (final tombstone in tombstones) {
    try {
      freed += await cleanup(
        sourceId,
        tombstone.logicalPath,
        List<String>.unmodifiable(tombstone.dependencyPaths),
      );
      verified.add(tombstone);
    } catch (error) {
      onError?.call(tombstone, error);
    }
  }
  return VerifiedRemoteTombstoneCleanupResult(
    freedBytes: freed,
    verifiedTombstones: verified,
  );
}

Future<BigInt> _purgeRemoteBookContent(
  BookSource source,
  String path,
  bool imageFolder,
) {
  return purgeRemoteBookContentCache(
    sourceType: source.type,
    path: path,
    url: source.url,
    port: source.port,
    rootPath: source.effectiveRootPath,
    clientId: source.clientId,
    rootId: source.rootId,
    cookieMode: (source.cookie ?? '').isNotEmpty,
    imageFolder: imageFolder,
  );
}

/// Process-local active-use leases protect a cache from being deleted while a
/// second Reader window still references the same remote book.
class RemoteBookUseRegistry {
  RemoteBookUseRegistry({RemoteBookContentCleanup? cleanup})
    : _cleanup = cleanup ?? _purgeRemoteBookContent;

  static final instance = RemoteBookUseRegistry();

  final RemoteBookContentCleanup _cleanup;

  final Map<String, int> _active = <String, int>{};
  final Map<String, BookSource> _sources = <String, BookSource>{};
  final Map<String, String> _paths = <String, String>{};
  final Map<String, bool> _imageFolders = <String, bool>{};
  final Set<String> _completionCandidates = <String>{};
  final Set<String> _pendingCleanup = <String>{};

  int activeCount(String key) => _active[key] ?? 0;

  RemoteBookUseLease acquire({
    required BookSource source,
    required String path,
    required bool enabled,
    required BookOpenStrategy strategy,
    bool isImageFolder = false,
  }) {
    final key = bookKeyOf(source.type, source.id, path);
    final eligible =
        enabled &&
        source.needsSession &&
        (isImageFolder ||
            strategy == BookOpenStrategy.download ||
            strategy == BookOpenStrategy.auto) &&
        !source.remoteOnly;
    if (eligible) {
      _active[key] = (_active[key] ?? 0) + 1;
      _sources[key] = source;
      _paths[key] = path;
      _imageFolders[key] = isImageFolder;
    }
    return RemoteBookUseLease._(this, key, eligible);
  }

  Future<RemoteBookCleanupResult?> _release(
    String key, {
    required bool completionCandidate,
  }) async {
    if (!_active.containsKey(key)) return null;
    if (completionCandidate) _completionCandidates.add(key);
    final remaining = (_active[key] ?? 1) - 1;
    if (remaining > 0) {
      _active[key] = remaining;
      return null;
    }
    _active.remove(key);
    final shouldCleanup =
        _completionCandidates.contains(key) || _pendingCleanup.contains(key);
    if (!shouldCleanup) {
      _completionCandidates.remove(key);
      _pendingCleanup.remove(key);
      _sources.remove(key);
      _paths.remove(key);
      _imageFolders.remove(key);
      return null;
    }
    final source = _sources[key];
    if (source == null) return null;
    try {
      final freed = await _cleanup(
        source,
        _paths[key] ?? key,
        _imageFolders[key] ?? false,
      );
      _pendingCleanup.remove(key);
      _completionCandidates.remove(key);
      _sources.remove(key);
      _paths.remove(key);
      _imageFolders.remove(key);
      return RemoteBookCleanupResult(freedBytes: freed);
    } catch (error) {
      // Reader close remains successful. Keep a retry marker for the next
      // eligible close; cleanup is opportunistic and never blocks navigation.
      _pendingCleanup.add(key);
      return RemoteBookCleanupResult(freedBytes: BigInt.zero, error: error);
    }
  }
}

class RemoteBookUseLease {
  RemoteBookUseLease._(this._registry, this.key, this.enabled);

  final RemoteBookUseRegistry _registry;
  final String key;
  final bool enabled;
  bool _released = false;

  Future<RemoteBookCleanupResult?> release({
    required bool completionCandidate,
  }) {
    if (_released || !enabled) return Future<RemoteBookCleanupResult?>.value();
    _released = true;
    return _registry._release(key, completionCandidate: completionCandidate);
  }
}
