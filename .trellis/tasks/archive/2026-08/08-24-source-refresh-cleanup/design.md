# Source refresh and stale-data cleanup design

## Boundaries and data flow

```text
source UI
  -> LibraryStore source CRUD
  -> debounced SQLite save (awaitable completion)
  -> LibraryCatalogStore.loadTree()
  -> SourceTreePanel

115 root edit
  -> BookSource effectiveRootPath
  -> browser / remote crawler / alignment

explicit cleanup
  -> remote index alignment
  -> tombstones
  -> Dart memory cleanup + Rust deletion/cache cleanup
  -> orphan-tag pruning
  -> SQLite save + catalog tree reload
```

## Source persistence contract

Keep `LibraryStore` as the facade owner. Add one private awaitable helper that
waits for the existing debounced `saveToDisk()` and then reloads the catalog
projection. `addSource` and `updateSource` return that future; old callers may
ignore it, while UI submit/edit handlers await it before closing or announcing
success. Do not make `LibraryCatalogStore` observe every `LibraryStore` change:
the explicit completion boundary avoids stale reads and notification loops.

`removeSourceWithCleanup` already has an async boundary; finish it by reloading
the catalog tree after its DB/cache cleanup. The simple legacy `removeSource`
method remains compatible but uses the same persistence helper when possible.

## 115 effective root

Add a small model-level helper (`effectiveRootPath`) with this rule:

```text
115: non-empty rootId, otherwise "0"
other sources: path, with "/" fallback only where existing code already does so
```

The edit dialog writes both `rootId` and `path` from the normalized 115 root.
Browser initialization and `crawlRemoteSource` consume the helper as defense in
depth. This avoids depending on every caller remembering that 115 `path` is a
catalog root ID while retaining the existing path field for compatibility.

## Cleanup and tag ownership

Add `TagRepository.removeBookTagsAndPrune(bookKey)`:

1. normalize the persisted book key;
2. remove links for that book;
3. collect tag IDs that have no remaining links;
4. remove those tag entities from memory and call the existing Rust tag-delete
   API (or the repository's equivalent persistence operation);
5. notify once.

`purgeStaleData` calls this method instead of `setBookTags(..., [])`. It then
continues the existing idempotent Rust record/meta/cache/AI cleanup and awaits
`saveToDisk`. A tag linked to another book is not in the unused set and is
preserved. The early no-key return still reloads the catalog projection when a
remote alignment changed tombstones/tree state.

`_alignRemoteIndex` inspects the typed refresh result. `missing-fingerprint` and
`remote-refresh-not-allowed` are failures; only a completed replacement/listing
is success. No deletion is inferred from a failed alignment.

## Compatibility and rollback

- Public Dart source CRUD signatures remain callable by existing code; changing
  the return type from `void` to `Future<void>` is source-compatible for ignored
  futures, and critical UI paths are updated to await them.
- Existing snapshots and source JSON need no migration.
- If a regression appears, revert only the new helper call sites; the existing
  DB deletion operations remain independently safe and idempotent.
- Automatic coordinator calls keep `alignRemote: false`, so the local-first
  invariant is unchanged.

## Verification

- Pure Dart tests cover effective-root and orphan-prune helpers.
- Focused Flutter tests cover source add/edit completion and tree reload wiring
  with the existing test doubles/mocks.
- `flutter analyze --no-pub` and `flutter test --no-pub`.
- Rust focused DB tests for tag deletion/cache cleanup if the modified API path
  requires them, followed by the project Rust test suite.
