# Source refresh and stale-data cleanup

## Goal

Make source configuration changes and user-triggered remote cleanup converge on
the same persisted catalog state without requiring an application restart, while
preserving the existing local-first scraping boundary. A newly added or edited
source must appear at once in the source tree; a 115 root change must affect
browsing and indexing; and a confirmed remote deletion must remove its local
reading record, metadata, tags, AI work and cached bytes.

## Confirmed facts

- `AddSourceDialog` mutates `LibraryStore`, but the source tree is owned by
  `LibraryCatalogStore` and currently reloads from Rust/SQLite independently.
- `LibraryStore.saveToDisk()` is debounced. The edit dialog calls
  `LibraryCatalogStore.loadTree()` before that save is guaranteed to finish.
- 115 browsing/indexing uses `BookSource.rootId` for sessions but uses
  `BookSource.path` as the initial browser/crawler path. Editing only the root
  ID therefore leaves the effective root stale.
- `purgeStaleData()` removes links with `TagRepository.setBookTags`, but does
  not remove now-unused tag definitions. The later tag snapshot can recreate
  those orphan tags in SQLite.
- Automatic catalog scraping intentionally uses persisted snapshots only and
  must not refresh remote book sources. Remote alignment is explicit cleanup
  behavior.
- Feishu feedback reports: source additions require restart to appear; edited
  115 roots keep the old directory; deleted remote books remain readable after
  cleanup; and their cached/metadata/tag state is not fully removed.

## Requirements

### R1. Source CRUD convergence

1. Source add/edit flows await persistence before reloading the catalog tree.
2. The source tree is refreshed after a successful source mutation without a
   process restart.
3. Existing callers that intentionally fire-and-forget source credential updates
   remain source-compatible, but the shared CRUD path must expose a completion
   future for UI flows that need convergence.

### R2. 115 root consistency

1. For 115, the configured root folder ID is the single effective catalog root.
2. Editing a 115 root updates the persisted path/root representation used by
   browser listing, remote index crawling and cleanup alignment.
3. Empty root input means the 115 root (`0`) consistently.
4. Non-115 source path behavior remains unchanged.

### R3. Deletion cleanup

1. Explicit remote alignment must treat a missing fingerprint or a disallowed
   remote refresh as a failed alignment and must not claim successful cleanup.
2. After cleanup, the catalog tree reloads so deleted nodes disappear in the
   current session.
3. Removing the final link for a stale book prunes its orphan tag definitions
   from both in-memory repositories and SQLite; tags still linked to another
   book remain.
4. Existing Rust deletion/cache cleanup remains authoritative and idempotent.
5. Automatic/local-only scraping continues to use `alignRemote: false`; this
   change does not add implicit remote source I/O to the scraper.

### R4. Regression coverage

Add tests for:

- source mutation completion being followed by a catalog reload;
- 115 effective-root normalization and crawler/browser root selection;
- missing/disallowed remote refresh being reported as failed;
- orphan tag pruning while shared tags survive;
- catalog tree refresh after purge and zero-remote-I/O automatic scraping.

## Acceptance criteria

- After adding a remote source from the UI, its source-tree node is visible
  without restarting the app.
- After editing a 115 root ID, a fresh browser/index crawl starts at that ID and
  does not use the old directory.
- When a remote source is aligned and a previously indexed file is gone,
  cleanup removes its read record, metadata, book-tag links, orphan tags, AI
  task and stale page/raw/cover cache; reopening the app cannot resurrect them.
- If remote alignment cannot run, cleanup reports an alignment failure and does
  not delete data based on an unverified absence.
- A tag linked to another live book is preserved.
- Existing Flutter tests and Rust tests pass; no automatic catalog-scrape path
  performs remote book-source I/O.

## Out of scope

- Changing the local-first scraper contract or adding background remote scans.
- New provider integrations, parser rules, canonical metadata policy or UI
  redesign.
- Deleting user data when remote alignment fails or when a source is offline.

## Open questions

None blocking implementation. The recommended policy is to keep remote
alignment explicit (the existing cleanup action) and keep the automatic scrape
lane snapshot-only.
