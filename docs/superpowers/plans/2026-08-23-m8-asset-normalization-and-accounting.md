# M8 Asset Identity and Catalog Context Normalization Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prevent physical catalog assets from colliding, normalize local/Quark/115 catalog context into one safe `CatalogSnapshot`, and make scrape accounting reject false-success runs before any canonical materialization.

**Architecture:** Keep the existing logical `book_key` for canonical metadata and sync compatibility, but introduce a physical `asset_key` derived from the persisted `library_index.id`. Scrape queue, proposal, and materialization provenance are keyed by `asset_key`; each row also carries the logical `book_key`. A source-aware normalizer removes file extensions, source category buckets, and self-nodes before the frozen parser sees ancestors/siblings. The coordinator remains proposal-only until the new accounting and 115 corpus gates pass.

**Tech Stack:** Rust (`rusqlite`, serde, parser/API), Flutter/Dart FRB DTOs and coordinator, SQLite idempotent migrations, Rust regression tests, Flutter analysis/tests, 389-item real-library dry-run export.

**Spec:** `.trellis/tasks/08-19-m8-a0-automation-integration/design.md`, `.trellis/spec/backend/automation-pipeline.md`, `.trellis/spec/backend/local-first-scraping.md`

## Global Constraints

- Catalog parsing remains filename/ancestor/sibling-only; zero comic-byte reads and zero remote book-source I/O.
- `asset_key` is one physical indexed file; logical `book_key` is not a physical uniqueness key.
- A run cannot be `succeeded` when asset identity collisions, proposal-count mismatches, or queue/proposal accounting mismatches exist.
- 115 and Quark context may use only persisted `library_index` rows; no remote refresh, stat, HEAD, PROPFIND, list, download, or source session.
- Existing canonical `book_metas`, `tags`, `book_tags`, read records, and sync identities remain compatible until a separate ADR changes them.
- Automatic canonical materialization is disabled/held until the new identity and 115 regression gates pass; proposal generation remains available for review.
- Do not add Provider calls, OCR, content metadata, filename correction, or new online enrichment in this round.

---

### Task 1: Freeze production materialization behind an explicit gate

**Files:**
- Modify: `app/lib/store/automation_coordinator.dart`
- Modify: `app/lib/ui/scrape_panel.dart`
- Modify: `app/lib/ui/sync_panel.dart`
- Modify: `.trellis/tasks/08-19-m8-a0-automation-integration/prd.md`
- Modify: `.trellis/tasks/08-19-m8-a0-automation-integration/design.md`
- Modify: `.trellis/tasks/08-19-m8-a0-automation-integration/implement.md`
- Test: `app/rust/src/scrape_projection.rs` and coordinator-facing UI tests where existing

**Interfaces:**
- Add one coordinator constant/setting `automaticMaterializationEnabled = false` for this checkpoint.
- `runCycle` and `runScrapeNow` continue generating proposals and status, but skip `_materializeReadyProposals` while the gate is false.
- UI labels must say “仅生成刮削 proposal” / “同步并生成 proposal” while the gate is held; no button may imply canonical writes.

- [ ] **Step 1: Add a failing Rust/Dart-visible policy assertion**

  Extend the automation status contract so a proposal-only run reports `autoApplied=0` and does not call the projection API. Keep the existing projection unit tests as direct transaction tests.

- [ ] **Step 2: Implement the gate and update product docs**

  Guard both coordinator call sites, preserve sync transport behavior, and rewrite the safe-auto addendum as “held pending asset identity and 115 normalization.”

- [ ] **Step 3: Run existing tests**

  Run `flutter analyze --no-pub` and `flutter test --no-pub`; verify no UI path invokes materialization while the gate is false.

---

### Task 2: Add physical asset identity and migrate working-state tables

**Files:**
- Modify: `app/rust/src/db/mod.rs`
- Modify: `app/rust/src/api/db.rs` if identity helpers need FRB exposure
- Modify: `app/rust/src/api/scraper.rs`
- Modify: `app/rust/src/scrape_projection.rs`
- Regenerate: `app/lib/src/rust/api/scraper.dart`, `app/lib/src/rust/frb_generated*.dart`, `app/rust/src/frb_generated.rs`
- Test: DB and projection tests in `app/rust/src/db/mod.rs` and `app/rust/src/scrape_projection.rs`

**Interfaces:**
- Add `pub fn asset_key_of(source_type: &str, source_id: &str, library_index_id: &str) -> String` with stable format `asset|<source_type>|<source_id>|<library_index_id>`.
- Extend `ScrapeProposalRow`, `ScrapeQueueRow`, and `ScrapeMaterializationRow` with `asset_key` and retain `book_key` as the logical canonical key.
- Add `asset_key` to `ScrapeProposalDto` and `ScrapeQueueDto`; `book_key` remains the logical key shown to callers.
- Materialization API accepts `asset_key` plus `expected_revision`, loads the logical `book_key` from the proposal, and writes canonical metadata/tags under that logical key.

- [ ] **Step 1: Write failing identity and collision tests**

  Cover two entries with the same path stem but different `library_index.id` values; assert distinct asset keys, one logical book key, two proposal rows, and no overwrite. Assert `.cbz` and `.zip` no longer share the proposal primary key.

- [ ] **Step 2: Add idempotent schema migration**

  Create/rebuild working tables so `scrape_proposals.asset_key` is the primary key, queue uniqueness is `(asset_key, input_revision, rule_version)`, and materialization provenance is `(asset_key, proposal_revision)`. Backfill legacy rows with `asset_key = book_key`, `book_key = book_key`, and preserve empty legacy revisions so they remain review-only.

- [ ] **Step 3: Update CRUD and projection queries**

  Replace proposal/queue/materialization lookups and UPSERT conflict targets with `asset_key`; retain logical `book_key` in payloads and use it only for canonical metadata/tag writes. Preserve materialization idempotency and stale-input checks.

- [ ] **Step 4: Regenerate FRB and run focused tests**

  Run `cargo test db::tests::book_key_normalization_matches_dart_archive_aliases db::tests::asset_key_is_physical_and_unique scrape_projection:: --lib` and `flutter analyze --no-pub`.

---

### Task 3: Normalize local/Quark/115 catalog context before parsing

**Files:**
- Create: `app/rust/src/catalog_context.rs`
- Modify: `app/rust/src/lib.rs`
- Modify: `app/rust/src/api/scraper.rs`
- Modify: `app/rust/src/scraper.rs` only for parser safety gates and new typed range/completeness fields
- Test: `app/rust/src/catalog_context.rs` and `app/rust/src/scraper.rs`

**Interfaces:**
- Add `CatalogAssetContext { asset_key, book_key, source_type, filename, ancestor_dirs, parent_siblings }`.
- Add `normalize_catalog_context(entry, source_type, by_id, max_depth) -> Result<CatalogAssetContext, CatalogContextError>`.
- Normalize file names to basename-without-extension; normalize ancestor directory names similarly; exclude the current asset from siblings.
- Drop generic/source buckets such as `日漫`, `漫画`, `Manga`, `Comic`, `单行本`, `连载`, `合集`, `全集`, format folders, and 115/Quark category nodes from work-title candidates while preserving real work folders.
- For 115/Quark, trust only persisted `parent_id` and `name`; never split opaque IDs in `path` into semantic ancestors.

- [ ] **Step 1: Add source-context unit tests**

  Test local, Quark, and 115 snapshots with `日漫/漫画/<work>/<file>`; assert `日漫`/`漫画` are absent from semantic ancestors, `<work>` remains, and the file extension is absent from `filename`/ancestor candidates. Test current-file sibling exclusion.

- [ ] **Step 2: Implement the normalizer and source adapter boundary**

  Build contexts from persisted `LibraryIndexRow` data only. Return a typed error for missing asset identity or malformed parent chains; do not fall back to path tokenization for remote IDs.

- [ ] **Step 3: Add parser safety gates**

  Keep strong filename title evidence ahead of weak/category ancestors; classify a structural-only filename such as `06+续` as `Partial` without a work context; prevent extension-bearing residuals from becoming titles; allow optional event/context prefixes before `[Circle (Artist)]`; add typed `season_range`, `chapter_range`, and completeness/status fields for multi-axis/finished labels.

- [ ] **Step 4: Run parser regression tests**

  Add cases for `COMEX/FGO`, `Alice Crazy ...zip`, `富家女姐姐 1-137 全集.zip`, `06+续`, the Vchan sibling set, `(アズレン夢想) [CAT GARDEN (ねこてゐ)] ...`, and `第1-4季 第1-144話[完結]`.

---

### Task 4: Make scrape run accounting collision-safe

**Files:**
- Modify: `app/rust/src/db/mod.rs`
- Modify: `app/rust/src/api/scraper.rs`
- Modify: `app/lib/store/automation_coordinator.dart`
- Modify: `app/lib/ui/scrape_panel.dart`
- Regenerate: FRB generated files
- Test: Rust API/DB tests and Flutter UI/status tests

**Interfaces:**
- Extend `ScrapeJobRow`/`ScrapeRunDto` with `input_assets`, `unique_assets`, `proposals_written`, `asset_collision_count`, `book_group_collision_count`, and `accounting_status`.
- `run_catalog_pass` must construct all work items before queue/proposal writes, reject duplicate `asset_key`, and assert `proposals_written == input_assets`.
- A run with accounting failure is `failed`/`degraded`, never `succeeded`, and cannot enqueue materialization.

- [ ] **Step 1: Add failing accounting tests**

  Use two physical entries sharing a logical book key; assert `input_assets=2`, `unique_assets=2`, `proposals_written=2`, `book_group_collision_count=1`, and `accounting_status=ok`. Use duplicate asset IDs to assert `asset_collision_count=1` and failed status with zero canonical writes.

- [ ] **Step 2: Refactor the batch runner into work-item preparation**

  Prepare contexts/proposals in memory, count physical assets, detect collisions, then enqueue/claim and persist. Do not mark queue rows succeeded until the proposal count check passes.

- [ ] **Step 3: Persist and expose accounting fields**

  Add idempotent columns/defaults to `scrape_jobs`, populate them on success/failure, and show the counts in the scrape panel.

- [ ] **Step 4: Run focused and full tests**

  Run `cargo test api::scraper:: db::tests::scrape_queue --lib`, `flutter analyze --no-pub`, and `flutter test --no-pub`.

---

### Task 5: Re-run the real corpus and hold for manual review

**Files:**
- Create/update: `docs/reports/catalog-dry-run-2026-08-23.json`
- Modify: `.trellis/tasks/08-19-m8-a0-automation-integration/check.jsonl`
- Modify: `.trellis/tasks/08-19-m8-a0-automation-integration/task.json`
- Modify: `.trellis/tasks/08-19-m8-a0-automation-integration/implement.md`

**Interfaces:**
- Export one JSON record per physical `asset_key`, including logical `book_key`, source type, normalized context, semantic proposal, evidence, conflicts, and accounting summary.
- Acceptance requires `input_assets == proposals_written`, `asset_collision_count == 0`, no extension/category false titles in the 389-item corpus, and consistent Vchan sibling sequence interpretation.

- [ ] **Step 1: Run the isolated 389-item catalog pass**

  Copy the user database to an isolated temporary root, run catalog-only parsing, and do not invoke SyncEngine, Provider APIs, source sessions, ByteSource, Downloader, or content readers.

- [ ] **Step 2: Inspect JSON and record corpus findings**

  Check local, Quark, 115 counts; inspect the listed false-title/sequence cases; record unresolved items as review-only rather than loosening Ready.

- [ ] **Step 3: Keep production gate closed until manual approval**

  Do not run canonical materialization or sync push from the real database. Only after the user confirms the 115 corpus results may Task 1’s gate be reopened.

---
