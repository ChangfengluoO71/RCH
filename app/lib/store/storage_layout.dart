/// Pure storage-layout helpers shared by startup code and regression tests.
///
/// Android's application-support directory maps to the app-private `filesDir`;
/// cache contents are intentionally kept outside this durable data root.
class StorageLayout {
  const StorageLayout._();

  static String _separatorFor(String root) => root.contains(r'\') ? r'\' : '/';

  static String _join(String root, String child) {
    final separator = _separatorFor(root);
    final trimmed = root.endsWith('/') || root.endsWith(r'\')
        ? root.substring(0, root.length - 1)
        : root;
    return '$trimmed$separator$child';
  }

  static String androidDataRoot(String applicationSupportRoot) =>
      _join(_join(applicationSupportRoot, 'RCH'), 'data');

  /// Candidate order is part of the migration contract. The Rust layer
  /// validates each path and selects the first valid one; no mtime comparison
  /// belongs here.
  static List<String> legacyDatabaseCandidates({
    required String dataRoot,
    String? markerRoot,
    required String defaultCacheRoot,
    required String supportRoot,
  }) {
    final roots = <String>[
      dataRoot,
      if (markerRoot != null && markerRoot.isNotEmpty) markerRoot,
      defaultCacheRoot,
      supportRoot,
      _join(supportRoot, 'RCH'),
    ];
    final seen = <String>{};
    return [
      for (final root in roots)
        if (seen.add(_join(root, 'database.db'))) _join(root, 'database.db'),
    ];
  }
}
