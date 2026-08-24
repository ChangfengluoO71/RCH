# Source Refresh and Stale-Data Cleanup

This contract records the source CRUD, 115/Quark root, and explicit cleanup
boundaries shared by Flutter, SQLite, and the remote index layer.

## 1. Scope / Trigger

Use this contract when a source is added, edited, removed, or explicitly
aligned for stale-data cleanup. It prevents stale source-tree projections,
wrong 115 roots, and destructive cleanup based on an unverified remote state.

## 2. Signatures

```dart
Future<void> LibraryStore.addSource(BookSource source);
Future<void> LibraryStore.updateSource(String id, {
  String? path,
  String? rootId,
  // other editable source fields...
});
Future<void> LibraryStore.removeSourceWithCleanup(String id);
Future<(int, int, int, int)> LibraryStore.purgeStaleData({
  bool alignRemote = true,
});
String BookSource.effectiveRootPath;
```

## 3. Contracts

- Source add/edit futures complete only after the debounced `saveToDisk()`
  boundary and a `LibraryCatalogStore.loadTree()` refresh. UI submit handlers
  await them before closing or reporting success.
- For `115` and `quark`, `rootId` is the effective catalog root. Trim it;
  blank input becomes `'0'`; persist the compatibility `path` field to the
  same normalized value. Browser, crawler, and cleanup alignment use
  `effectiveRootPath`.
- `purgeStaleData(alignRemote: true)` may remove remote-source data only after
  a successful source-index replacement/listing. `missing-fingerprint` and
  `remote-refresh-not-allowed` revisions are failures.
- Cleanup removes read records, metadata, tags, AI tasks, and stale cache
  bytes. Removing a book's final tag link also removes the orphan tag entity;
  a tag still linked to another live book is preserved.
- Automatic/local-only scraping keeps `alignRemote: false`; it does not open a
  remote book-source session.

## 4. Validation & Error Matrix

| Condition | Required result |
|---|---|
| Source save completes | Refresh the source tree before UI success |
| 115/Quark root is blank | Persist and use root `'0'` |
| Remote refresh has no fingerprint | Alignment fails; do not infer deletion |
| Remote listing callback is unavailable | Alignment fails; do not infer deletion |
| Final tag link for a stale book is removed | Delete the orphan tag entity and DB row |
| Tag is shared by another live book | Remove only the stale link |
| Automatic scrape | No remote source I/O |

## 5. Good / Base / Bad Cases

- Good: edit a 115 root, await the save, and open the browser; the first
  request uses the new root ID.
- Base: add a source while SQLite is available; the source-tree node appears
  without restarting the app.
- Bad: call `loadTree()` immediately after a debounced mutation, or use
  `source.path` when a 115 `rootId` differs.
- Bad: treat a failed remote refresh as an empty directory and delete local
  records, metadata, tags, or cache.

## 6. Tests Required

- Test configured, legacy, and blank 115 roots through `effectiveRootPath`.
- Test remote crawl starts at the effective root and never uses the old path.
- Test failed refresh revisions do not authorize cleanup.
- Test orphan tag pruning deletes only unshared tags.
- Run `flutter analyze --no-pub`, `flutter test --no-pub`, and the serial Rust
  suite after cross-layer changes.

## 7. Wrong vs Correct

### Wrong

```dart
store.updateSource(id, rootId: editedRoot);
LibraryCatalogStore.instance.loadTree(); // save is still debounced
```

### Correct

```dart
await store.updateSource(id, rootId: editedRoot);
// updateSource waits for persistence and refreshes the catalog projection.
```
