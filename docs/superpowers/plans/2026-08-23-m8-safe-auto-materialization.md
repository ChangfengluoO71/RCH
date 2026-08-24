# M8 Safe-Auto Materialization and Sync Pipeline Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn the existing local catalog scraper into a durable automation pipeline that generates snapshots, produces offline proposals, automatically materializes only `Ready` proposals without conflicts into compatible metadata/tags, and schedules the existing sync transport to push canonical changes.

**Architecture:** Keep `catalog-rules-v3` as a pure parser over persisted catalog text. Add a local catalog-revision/queue layer, a single Rust projection transaction for `BookMeta` and additive namespaced tags, and a typed event from materialization to the existing sync coordinator. Working proposals, queue state, evidence and provenance stay local; only canonical metadata and tags enter the existing sync snapshot.

**Tech Stack:** Rust (`rusqlite`, `serde`, FRB API), Flutter/Dart repositories and coordinator, SQLite ordered migrations, existing WebDAV sync snapshot, Rust unit/integration tests, Flutter tests and the 347-item catalog regression corpus.

**Spec:** `.trellis/tasks/08-19-m8-a0-automation-integration/design.md`

## Global Constraints

- `catalog-rules-v3` remains filename/ancestor/sibling-only and performs zero comic-byte reads and zero remote book-source I/O.
- `RemoteOnly` assets may use only persisted `library_index`, `FolderSnapshotStore` and local SQLite data; the scrape path cannot receive `ByteSource`, Downloader, source sessions, remote URLs or sync handles.
- Only `Ready` proposals with an empty `conflicts` list are eligible for safe-auto materialization.
- Manual metadata and manual tags are never overwritten or removed by automatic materialization.
- Rust owns the projection and SQLite transaction; Dart/UI consumes its typed result and reloads state instead of reimplementing JSON-to-domain mapping.
- `book_metas`, `tags` and `book_tags` remain syncable; jobs, proposals, evidence and provenance remain local working state.
- Materialization marks sync-dirty but never invokes sync transport inline.
- Preserve unrelated user changes and the frozen parser/golden corpus.

---

### Task 1: Add ordered schema for catalog revisions, queue items and materialization provenance

**Files:**
- Modify: `app/rust/src/db/mod.rs` schema/migration registry and DB test module
- Modify: `app/rust/src/api/db.rs` only if the migration/version API needs a typed result
- Test: Rust DB tests near `app/rust/src/db/mod.rs`

**Interfaces:**
- Produce `CatalogRevision { revision: String, changed_book_keys: Vec<String> }` data sufficient for the coordinator to enqueue work.
- Produce queue operations with stable names: `enqueue_catalog_scrape(book_key, source_id, path, input_revision, rule_version, trigger)`, `claim_due_catalog_scrape(now)`, `complete_catalog_scrape(book_key, input_revision, result)`, and `requeue_stale_catalog_scrape(book_key, current_revision)`.
- Produce `MaterializationRecord` keyed by `(book_key, proposal_revision)` with rule version, applied fields, added tags, skipped fields and timestamps.

- [ ] **Step 1: Write failing migration tests**

Add tests that initialize a fresh in-memory DB and assert the presence and unique constraints of:

~~~
catalog_revisions
scrape_queue
scrape_materializations
~~~

The queue must reject a duplicate `(book_key, input_revision, rule_version)` and retain its materialization status when a proposal is updated.

- [ ] **Step 2: Run the focused DB tests and confirm the schema is absent**

Run from `app`:

~~~
cargo test db::tests::scrape_queue_schema --lib
~~~

Expected: FAIL because the new tables and migration are not present.

- [ ] **Step 3: Add the ordered migration**

Use the repository's existing migration/version mechanism. Define the queue columns explicitly:

~~~
CREATE TABLE IF NOT EXISTS scrape_queue (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  book_key TEXT NOT NULL,
  source_id TEXT NOT NULL,
  path TEXT NOT NULL,
  input_revision TEXT NOT NULL,
  rule_version TEXT NOT NULL,
  trigger TEXT NOT NULL,
  status TEXT NOT NULL,
  attempt INTEGER NOT NULL DEFAULT 0,
  next_run_at INTEGER NOT NULL,
  last_error TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  UNIQUE(book_key, input_revision, rule_version)
);
~~~

Keep the existing run-level `scrape_jobs` table for summaries. Do not overload it with per-book state.

- [ ] **Step 4: Add queue/materialization UPSERT helpers**

Use `ON CONFLICT DO UPDATE` and update only scheduling fields. Never use `INSERT OR REPLACE` for proposal or materialization rows because it can delete status/provenance columns.

- [ ] **Step 5: Run the focused tests**

Run:

~~~
cargo test db::tests::scrape_queue_schema db::tests::scrape_queue_deduplicates db::tests::materialization_record_is_idempotent --lib
~~~

Expected: PASS, including duplicate enqueue and restart-safe status assertions.

- [ ] **Step 6: Commit the schema unit**

~~~
git add app/rust/src/db/mod.rs app/rust/src/api/db.rs
git commit -m "feat: add catalog scrape queue and materialization state"
~~~

### Task 2: Make snapshot generation independent from sync transport

**Files:**
- Modify: `app/lib/store/library_index_service.dart`
- Modify: `app/lib/store/sync_engine.dart`
- Modify: `app/lib/store/automation_coordinator.dart`
- Modify: `app/lib/main.dart` only if a catalog revision store must be initialized
- Test: Dart store tests and Rust/Dart local-first integration tests

**Interfaces:**
- Add `Future<CatalogSnapshotResult> refreshCatalogSnapshots({required String trigger})` to the local catalog service.
- Add a typed `CatalogChanged` event containing source id, revision and changed book keys.
- Add the exact API `SyncEngine.syncNow({bool refreshCatalog = true})` so the coordinator can refresh indexes once and run transport without duplicating scans.

- [ ] **Step 1: Write failing tests for no-sync snapshot generation**

Cover both source classes:

~~~
local source + no WebDAV -> local filesystem index is refreshed and a scrape revision is emitted
remote source + persisted FolderSnapshotStore -> library_index is rebuilt from snapshots with zero listRemote calls
remote source + missing persisted snapshot -> no refresh is attempted and the source remains unavailable/partial
~~~

- [ ] **Step 2: Run the tests and confirm the current early return fails the no-sync case**

Run:

~~~
flutter test test/automation_snapshot_test.dart --no-pub
~~~

Expected: FAIL because `SyncEngine.syncNow()` exits before local index refresh when WebDAV is not configured.

- [ ] **Step 3: Extract the local snapshot phase**

The phase may call `scanLocalSource` for local/SMB sources and `buildIndexFromSnapshots` for remote sources. It must never call `crawlRemoteSource`, `listRemote`, `PROPFIND`, `HEAD`, SFTP stat or cloud APIs from the scrape-triggering path.

- [ ] **Step 4: Persist and compare catalog revision**

Compute a deterministic source revision from persisted index rows/root hashes and store it in `catalog_revisions`. Return only changed `book_key`s. Do not use wall-clock time alone as the revision identity.

- [ ] **Step 5: Add typed event routing**

Replace the coordinator's broad `LibraryStore` notification dependency with a catalog-change event. Catalog changes enqueue scrape work; canonical metadata/tag changes enqueue sync work; proposal/evidence writes enqueue neither remote lane.

- [ ] **Step 6: Run the local-first tests**

~~~
flutter test test/automation_snapshot_test.dart test/remote_only_scrape_test.dart --no-pub
~~~

Expected: PASS with remote request spies at zero for the remote-source cases.

### Task 3: Persist one proposal per canonical book key and drain the queue

**Files:**
- Modify: `app/rust/src/api/scraper.rs`
- Modify: `app/rust/src/db/mod.rs`
- Modify: `app/rust/src/scraper.rs` only for shared key helper/tests; do not change v3 parsing rules
- Modify: `app/lib/store/automation_coordinator.dart`
- Regenerate: `app/lib/src/rust/api/scraper.dart` and FRB generated files if signatures change
- Test: Rust scraper/API tests and Dart coordinator tests

**Interfaces:**
- Add `db_enqueue_catalog_scrape(...)`, `db_claim_catalog_scrape(...)` and `db_complete_catalog_scrape(...)` FRB functions.
- Make the Rust proposal key use the same normalized path semantics as Dart `bookKeyOf`; retain raw `path` separately for catalog evidence.
- Add proposal fields `input_revision`, `materialization_status` and `materialization_error` without dropping existing semantic JSON.

- [ ] **Step 1: Write failing key-parity and queue-dedupe tests**

Assert that `/books/a.zip`, `/books/a.cbz` and `/books/a` map as Dart `bookKeyOf` requires, and that one book/revision/rule creates one queue item.

- [ ] **Step 2: Run the focused tests and record the current raw-path mismatch**

~~~
cargo test api::scraper db::tests::book_key --lib
flutter test test/comic_path_alias_test.dart --no-pub
~~~

Expected: the new parity assertion exposes the raw `entry.path` construction in `app/rust/src/api/scraper.rs`.

- [ ] **Step 3: Add one canonical key helper and conformance tests**

Implement Rust `db::normalize_book_key_path` and expose `db_book_key_of` through FRB; make Dart `bookKeyOf` call that generated helper. The test suite must compare archive aliases and ordinary files.

- [ ] **Step 4: Change the scrape pass to consume claimed queue items**

Build each `CatalogSnapshot` only from persisted `library_index` rows, ancestor chains and same-parent siblings. Exclude the current row from sibling corroboration. Capture `input_revision` before parsing and mark stale rows for requeue instead of applying them.

- [ ] **Step 5: Make proposal persistence status-preserving**

Update semantic/evidence fields while preserving `materialization_status`, review decisions and provenance columns. Use an explicit UPSERT clause.

- [ ] **Step 6: Run Rust and coordinator tests**

~~~
cargo test scraper:: db:: --lib
flutter test test/automation_coordinator_test.dart --no-pub
~~~

### Task 4: Implement the safe-auto projection transaction

**Files:**
- Create: `app/rust/src/scrape_projection.rs`
- Modify: `app/rust/src/db/mod.rs`
- Modify: `app/rust/src/api/scraper.rs`
- Modify: `app/lib/repository/tag_repository.dart` only for read/reload helpers; do not duplicate projection logic
- Test: Rust projection/transaction tests

**Interfaces:**

~~~
pub struct MaterializeResult {
    pub book_key: String,
    pub status: String, // applied | skipped | stale | rejected
    pub changed_fields: Vec<String>,
    pub added_tags: Vec<String>,
    pub skipped_fields: Vec<String>,
    pub sync_dirty: bool,
}

pub fn db_materialize_ready_proposal(
    book_key: String,
    expected_revision: String,
) -> Result<MaterializeResult, String>;
~~~

- [ ] **Step 1: Write failing projection tests**

Test the exact policy:

~~~
Ready + no conflicts -> title/author-safe fields and namespaced tags are written
Ready + conflict -> no canonical write
Partial/Ambiguous -> no canonical write
circle -> not author
artist/author/writer -> author candidate
release_group -> release-group:<name>
resource fields -> resource:<category>:<value>
manual title/tag -> preserved and listed in skipped_fields
repeat same proposal revision -> idempotent
stale input revision -> rejected and no write
~~~

- [ ] **Step 2: Run the tests and confirm no projection API exists**

~~~
cargo test scrape_projection --lib
~~~

Expected: FAIL because the projection transaction is not implemented.

- [ ] **Step 3: Define the projection mapper**

Map `work_title` to `book_metas.title` only when empty or previously auto-owned. Map person creator roles to `book_metas.author`; never map circle/provider/resource labels to author. Keep aliases, publication title and uncertain source series in proposal semantic data until their canonical schema exists.

- [ ] **Step 4: Define namespaced tag normalization**

Use deterministic tags such as:

~~~
resource:language:zh
resource:translation:translated
resource:edition:digital
resource:censorship:uncensored
release-group:<name>
sequence:part:front
sequence:chapter:10
~~~

Add tags without deleting existing manual/system tags. Store the exact added/skipped set in provenance.

- [ ] **Step 5: Implement one SQLite transaction**

Within one transaction: load proposal and current catalog revision, reject stale input, load current `book_metas` and `book_tags`, apply field-level ownership checks, upsert metadata/tags with `updated_at`, write materialization provenance, update proposal status and return `MaterializeResult`. Do not call Dart repositories or sync transport from this transaction.

- [ ] **Step 6: Run projection and sync-row tests**

~~~
cargo test scrape_projection db::tests::sync_rows_after_materialization --lib
~~~

### Task 5: Integrate materialization into the coordinator and schedule sync push

**Files:**
- Modify: `app/lib/store/automation_coordinator.dart`
- Modify: `app/lib/store/sync_engine.dart`
- Modify: `app/lib/store/library_store.dart`
- Modify: `app/rust/src/api/scraper.rs`
- Test: Dart coordinator integration tests and Rust sync snapshot tests

**Interfaces:**
- Add `db_materialize_ready_proposal` FRB binding.
- Add a coordinator local stage `materializeReadyProposals(runId)`.
- Emit `CanonicalChanged` only after a successful local transaction.

- [ ] **Step 1: Write failing ordering and no-inline-sync tests**

Assert:

~~~
snapshot -> scrape -> materialize -> canonical-dirty -> scheduled sync
materialize transaction -> sync transport call count remains zero
no WebDAV -> snapshot/scrape/materialize still complete locally
~~~

- [ ] **Step 2: Run the integration tests and confirm current coordinator stops after proposals**

~~~
flutter test test/automation_materialization_test.dart --no-pub
~~~

Expected: FAIL because `AutomationCoordinator.runCycle()` currently has no materialization stage.

- [ ] **Step 3: Add the local materialization stage**

After a scrape run, claim eligible proposals (`Ready`, empty conflicts, current revision), call the Rust transaction, and record applied/skipped/stale counts. Ambiguous and conflicted proposals remain visible to the review UI.

- [ ] **Step 4: Route canonical-dirty to the existing sync lane**

Schedule a sync job after commit using the coordinator's single scheduling owner. Reuse existing cooldown/backoff. Do not introduce a second timer or call `syncNow()` directly from the Rust projection API.

- [ ] **Step 5: Keep sync snapshots unchanged**

Verify `sync/snapshot.rs` continues to include metas/tags but not scrape jobs/proposals/provenance. Add a regression test that a materialized title and namespaced tag appear in the sync delta.

- [ ] **Step 6: Reload Dart state after FRB result**

Refresh `LibraryStore`/`TagRepository` from SQLite and notify once after the batch. Avoid calling `setBookTags()` with a full replacement list.

- [ ] **Step 7: Run coordinator and sync tests**

~~~
flutter test test/automation_materialization_test.dart test/sync_engine_test.dart --no-pub
cargo test sync::snapshot --lib
~~~

### Task 6: Extend review UI and expose automatic-apply status

**Files:**
- Modify: `app/lib/ui/scrape_panel.dart`
- Modify: `app/lib/store/automation_coordinator.dart` status model
- Regenerate: FRB Dart bindings if DTOs change
- Test: Flutter widget tests

**Interfaces:**
- Display `materialization_status`, `changed_fields`, `added_tags`, `skipped_fields`, `last_error` and `input_revision`.
- Keep manual apply/review controls for `Partial`, `Ambiguous` and conflicted proposals.

- [ ] **Step 1: Add failing widget assertions**

Render one automatically applied proposal, one review-required conflict and one stale proposal. Verify the UI never labels a review-required row as canonical.

- [ ] **Step 2: Implement status and conflict presentation**

Show whether the row was auto-applied, skipped due to manual ownership, rejected as stale or awaiting review. Preserve the existing evidence display.

- [ ] **Step 3: Run Flutter verification**

~~~
flutter analyze --no-pub
flutter test --no-pub
~~~

### Task 7: Full regression, real-corpus verification and archive checkpoint

**Files:**
- Modify: `.trellis/tasks/08-19-m8-a0-automation-integration/implement.md`
- Modify: `.trellis/tasks/08-19-m8-a0-automation-integration/check.jsonl`
- Test: Rust, Flutter, local-first spies and the 347-item corpus

- [ ] **Step 1: Run all Rust tests**

~~~
cargo test --lib
~~~

- [ ] **Step 2: Run all Flutter checks**

~~~
flutter analyze --no-pub
flutter test --no-pub
~~~

- [ ] **Step 3: Run the 347-item regression**

Assert:

~~~
false_title = 0
false_creator/provider = 0
false_sequence = 0
explicit release-group token loss = 0
materialization writes only Ready + conflict-free proposals
~~~

Do not alter golden expectations to hide a projection regression.

- [ ] **Step 4: Run RemoteOnly zero-I/O tests**

Use WebDAV/Quark/SFTP spies and assert zero remote book-source requests, zero `ByteSource` reads, zero content reads and zero directory refresh calls during snapshot-to-materialization. Provider traffic remains disabled in this phase.

- [ ] **Step 5: Run sync-boundary tests**

Verify that only `book_metas`, `tags` and `book_tags` are present in the sync delta; proposals, queue rows, evidence and provenance remain local.

- [ ] **Step 6: Record the checkpoint**

Update the A0 implementation/check JSONL with test counts, safe-auto policy and any remaining review-only cases. Do not mark the task complete until the user has reviewed the real-library auto-materialized output.

## Self-review

- Snapshot generation, no-WebDAV mode and persisted remote snapshots are covered by Task 2.
- Canonical key parity and durable dedupe are covered by Task 3.
- Safe-auto eligibility, manual-field protection, namespaced tags, idempotency and stale input are covered by Task 4.
- Coordinator ordering, sync-dirty and no-inline-transport behavior are covered by Task 5.
- UI observability and review fallback are covered by Task 6.
- 347-corpus, zero-I/O and sync-boundary checks are covered by Task 7.
- The parser itself is deliberately not expanded in this plan; `catalog-rules-v3` remains the frozen offline baseline.


