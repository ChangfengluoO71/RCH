# Tag, Metadata, Reader and Catalog Cleanup Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task with verification checkpoints.

**Goal:** Make automatic scraping produce a small, Chinese, user-facing tag set; show scraped metadata in the detail page; make tag results open the real physical asset; and make catalog additions/deletions flow through scraping and complete cleanup safely.

**Architecture:** Keep `book_metas` as the work metadata source of truth and `book_tags` as the user-facing tag relation. Rust owns canonical tag projection and deletion cleanup inside local SQLite transactions. Dart resolves a logical `book_key` to a live `library_index` asset before opening details/readers. The automation coordinator refreshes persisted catalog state, cleans deleted assets, scrapes only the resulting local catalog, materializes safe proposals, reloads repositories, and then performs the existing sync boundary.

**Tech Stack:** Rust, rusqlite, Flutter/Dart, flutter_rust_bridge, existing `library_index`, `scrape_*`, `book_metas`, `book_tags`, and sync tombstone tables.

**Spec:** `docs/project/SPEC.md`, `.trellis/spec/backend/local-first-scraping.md`, `.trellis/spec/backend/automation-pipeline.md`, and the approved design in the preceding task discussion.

## Global Constraints

- Catalog scraping and materialization remain zero-byte-read and zero-remote-book-source-I/O.
- Provider APIs are not added in this change.
- `asset_key` remains the physical asset identity; normalized `book_key` remains the logical work grouping key.
- Deleting one physical asset must preserve a logical work while another live asset with the same `book_key` exists.
- Existing dirty worktree changes belong to the user; do not reset or revert them.
- Use `apply_patch` for source edits and run focused tests after each task.

---

### Task 1: Canonical user-facing tag projection and migration

**Files:**
- Modify: `app/rust/src/scrape_projection.rs`
- Modify: `app/lib/repository/tag_repository.dart`
- Modify: `app/lib/store/library_store.dart`
- Test: `app/rust/src/scrape_projection.rs` unit tests
- Test: `app/test/tag_repository_test.dart` (create if absent)

**Interfaces:**
- Produce a Rust helper `canonical_tag_names(semantic: &serde_json::Value) -> Vec<String>` used by `materialize_ready_proposal_on`.
- Produce a Dart `Future<void> normalizeGeneratedTags()` on `TagRepository` that maps old generated namespaces to canonical names and persists tombstones through existing DB tag APIs.
- Extend `TagRepository.loadFromSqlite({bool force = false})` so a forced store reload actually reloads SQLite tags and links.

- [ ] **Step 1: Add failing projection tests** for `uncensored`, `full_color`, `colorized`, `complete/collection`, translated Chinese, digital, machine translation, release groups, and ignored sequence/publication/provider fields. Assert canonical Chinese names and no `resource:*`, `sequence:*`, `publication:*`, or `release:*` output.
- [ ] **Step 2: Run the focused Rust test** with `cargo test --manifest-path app/rust/Cargo.toml --lib scrape_projection --quiet` and verify the new tests fail against the current namespaced output.
- [ ] **Step 3: Implement `canonical_tag_names`** with these stable mappings:
  - `uncensored`, `无修正`, `無修正` -> `无修正`
  - `censored`, `有修正` -> `有修正`
  - `full_color`, `colorized`, `全彩`, `彩漫` -> `彩漫`
  - explicit `color_pages`/`彩页` -> `彩页`
  - `complete`, `collection`, `合集`, `全集` -> `合集`
  - translated language/state -> `中文翻译`
  - machine translation -> `机翻`
  - digital/DL -> `数字版`
  - explicit release group -> `汉化组：<name>`
  - unknown/raw/sequence/publication/provider/event fields -> no tag
- [ ] **Step 4: Add migration tests** for old namespaced tags, duplicate synonyms, a manual plain tag, and an old release-group tag. Assert links move to the canonical target, unknown generated tags disappear, manual tags remain, and old tags produce deletion persistence calls.
- [ ] **Step 5: Implement `normalizeGeneratedTags()`** over loaded tags and links. Recognize only known generated prefixes, merge links into canonical names, delete obsolete generated tags through `dbDeleteTag`, and leave plain/manual names untouched. Keep the operation idempotent.
- [ ] **Step 6: Force-load and migrate at startup** in `LibraryStore._loadFromSqlite()`, then run the focused Dart tests.
- [ ] **Step 7: Run Rust and Dart tag tests** and inspect the production DB read-only to confirm the exposed tag set contains canonical Chinese names only for generated semantics.

### Task 2: Metadata detail page and non-destructive tag editing

**Files:**
- Modify: `app/lib/ui/book_detail_page.dart`
- Modify: `app/lib/store/library_store.dart`
- Modify: `app/lib/repository/tag_repository.dart`
- Modify: `app/lib/store/models.dart` only if a small display DTO is required
- Test: `app/test/book_detail_page_test.dart` (create if absent)

**Interfaces:**
- `BookDetailPage` reads displayed tags from `TagRepository.tagsForBook(bookKey)`.
- `LibraryStore.updateMeta(BookMeta)` updates metadata fields without replacing all `book_tags`.
- Explicit tag actions call `TagRepository.link/unlink` and persist only that relation.

- [ ] **Step 1: Add a widget/unit regression test** that creates a metadata row plus canonical SQLite tag links, builds the detail page, and asserts title, author, series, and canonical tags are visible.
- [ ] **Step 2: Add a regression test** that edits a title while automatic tags exist and asserts the tag links remain unchanged.
- [ ] **Step 3: Replace `_meta.tags` display/editing** with a live `TagRepository.tagsForBook(_meta.key)` view. Keep `BookMeta.tags` only as legacy JSON compatibility data.
- [ ] **Step 4: Change `_addTag` and `_removeTag`** to link/unlink through `TagRepository`, persist the book links, and refresh the UI.
- [ ] **Step 5: Remove the destructive `setBookTags(m.key, m.tags)` call** from `LibraryStore.updateMeta`; retain the existing read-state behavior and metadata persistence.
- [ ] **Step 6: Add a compact metadata summary section** showing title, Chinese title/aliases when present, author, and series before the editable fields.
- [ ] **Step 7: Run the focused Flutter tests and `flutter analyze --no-pub`**.

### Task 3: Resolve logical book keys to physical readable assets

**Files:**
- Modify: `app/lib/store/library_store.dart`
- Modify: `app/lib/ui/home_page.dart`
- Modify: `app/lib/ui/book_detail_page.dart`
- Modify: `app/rust/src/api/library.rs`
- Test: `app/test/library_store_tag_resolution_test.dart` (create if absent)
- Test: Rust library query tests in `app/rust/src/api/library.rs`

**Interfaces:**
- Add a Dart helper `Future<({String path, String name})?> resolveLiveAsset(String bookKey)` that compares normalized `bookKeyOf(sourceType, sourceId, index.path)` against live `library_index` rows.
- `recordsByTag()` uses the resolver for unread/tag-only books and returns the physical path, including `.zip/.cbz` where applicable.
- Rust library SQL uses the same archive-extension normalization as `db::book_key_of` when joining `book_metas` and `book_tags`.

- [ ] **Step 1: Add a failing test** for a tag-only local `.zip` asset whose link key omits `.zip`; assert tag detail resolution returns the real `.zip` path.
- [ ] **Step 2: Add a failing test** for a Quark/115 id-path asset and assert the persisted fid/path is returned unchanged.
- [ ] **Step 3: Implement `resolveLiveAsset`** with one index load per source, preferring a live file row and using the index display name. Never query the remote source.
- [ ] **Step 4: Update `recordsByTag`** to use the resolver and to omit deleted/unresolvable physical assets from the reader grid rather than manufacturing a path from the logical key.
- [ ] **Step 5: Add a reader guard** in the detail page: if no live asset resolves, show index-only metadata and disable the read action with a clear message.
- [ ] **Step 6: Add a Rust SQL normalization expression** covering `.cbz`, `.zip`, `.cbr`, `.rar`, `.cb7`, `.7z`, `.cbt`, `.tar`, `.azw`, and `.azw3` so catalog tag/meta joins use logical keys.
- [ ] **Step 7: Run Dart resolver tests, Rust library tests, and manually verify one local archive and one id-path source entry through the tag screen.**

### Task 4: Transactional catalog deletion cleanup

**Files:**
- Modify: `app/rust/src/db/mod.rs`
- Modify: `app/rust/src/api/db.rs` only if the cleanup result/API needs exposure
- Modify: `app/rust/src/sync/apply.rs`
- Modify: `app/lib/store/library_index_service.dart`
- Modify: `app/lib/store/library_store.dart`
- Test: Rust DB tests in `app/rust/src/db/mod.rs`
- Test: `app/test/library_index_service_test.dart`

**Interfaces:**
- Add internal Rust `cleanup_deleted_asset_on(conn, source_id, index_id, path) -> Result<()>`.
- Invoke it whenever a file `library_index` row becomes deleted, including source replacement, sync tombstones, and source deletion.
- Cleanup is idempotent and uses `asset_key_of` for scrape working-state rows and `book_key_of` for logical metadata/tag rows.

- [ ] **Step 1: Add failing Rust tests** for deleting the last asset and for deleting one of two archive aliases sharing a logical key. Assert proposal/queue/materialization cleanup, metadata/read/tag deletion, and preservation while another asset remains.
- [ ] **Step 2: Add a failing test** that repeats cleanup and verifies no duplicate errors or data resurrection.
- [ ] **Step 3: Implement `cleanup_deleted_asset_on`**: remove asset-scoped scrape rows; if no live file has the same logical key, remove `read_records`, `book_metas`, `book_tags`, related AI tasks, and unreferenced tags while writing sync tombstones for syncable entities.
- [ ] **Step 4: Call cleanup from `replace_library_index_for_source_on`** after marking each missing file row deleted.
- [ ] **Step 5: Call cleanup from `sync/apply.rs`** when applying a deleted file index entry.
- [ ] **Step 6: Harden `delete_source_on`** to clean all source assets and orphaned scrape rows before removing the source/index/snapshot records.
- [ ] **Step 7: Fix remote persisted snapshot reconciliation** so a refreshed browsed folder marks children missing from that folder as deleted; do not treat an incomplete unbrowsed remote tree as authoritative.
- [ ] **Step 8: Run Rust DB/sync tests and the library-index Dart tests.**

### Task 5: Integrate cleanup and reloads into the automation coordinator

**Files:**
- Modify: `app/lib/store/automation_coordinator.dart`
- Modify: `app/lib/store/library_index_service.dart`
- Modify: `app/lib/store/library_store.dart`
- Test: `app/test/automation_coordinator_test.dart`

**Interfaces:**
- A catalog refresh returns changed/deleted local state through the existing revision path; Rust cleanup has already committed before scraping starts.
- `AutomationCoordinator.runCycle` reloads repositories after catalog cleanup and after materialization.

- [ ] **Step 1: Add a coordinator test** proving a refresh with a deleted asset causes cleanup/reload before `dbRunCatalogScrape` is invoked.
- [ ] **Step 2: Add a test** proving a newly indexed asset is included in the next scrape run and materialization cycle exactly once.
- [ ] **Step 3: Update `runCycle` and `runScrapeNow`** to reload `LibraryStore` whenever catalog refresh changes data, not only when materialization applies.
- [ ] **Step 4: Keep remote discovery separate**: automatic scraping consumes persisted local snapshots; any online remote listing remains an explicit catalog-discovery action and is never called by the parser/materializer.
- [ ] **Step 5: Verify push sync happens only after local projection/cleanup commits.**
- [ ] **Step 6: Run coordinator tests and the complete Dart test suite.**

### Task 6: Production regression and documentation

**Files:**
- Modify: `.trellis/spec/backend/automation-pipeline.md`
- Modify: `.trellis/spec/backend/local-first-scraping.md`
- Modify: `docs/project/SPEC.md` only for confirmed user-visible tag/cleanup contracts
- Create: `docs/reports/2026-08-24-tag-metadata-cleanup.md`

- [ ] **Step 1: Add regression coverage** for canonical tag vocabulary, metadata detail rendering, archive reader resolution, one-asset/two-asset deletion, source deletion, and zero remote book-source I/O.
- [ ] **Step 2: Run `cargo test --manifest-path app/rust/Cargo.toml --lib --quiet`.**
- [ ] **Step 3: Run `flutter test --no-pub`.**
- [ ] **Step 4: Run `flutter analyze --no-pub`.**
- [ ] **Step 5: If Rust API signatures changed, run `app/codegen.ps1`, rebuild the release native library, and rerun the FRB hash smoke check.**
- [ ] **Step 6: Run a read-only production DB audit** for tag namespaces, orphan links, proposal counts, live index counts, and deleted-book cleanup; do not expose credentials.
- [ ] **Step 7: Record changed files, test results, remaining limitations, and the strict offline invariant in the report.**

## Self-review checklist

- Canonical tag mappings cover all requested meaningful examples and merge synonyms.
- Metadata fields and tags are visible in the detail page without destructive overwrite.
- Tag navigation resolves physical paths instead of deriving paths from normalized logical keys.
- Local, persisted remote snapshot, sync tombstone, and source deletion paths all invoke cleanup.
- Shared logical works survive deletion of one archive alias and are deleted after the final live asset disappears.
- No parser, materializer, cleanup transaction, or tag resolver initiates remote book-source I/O.
