# M8-A0 Automation Coordinator & Sync Integration

## Implementation checkpoint

Implemented the first usable vertical slice and paused for manual sample validation.

- Catalog-only parser reads filename plus persisted ancestor/sibling names; it never
  receives a `ByteSource`, downloader or source session.
- `catalog-rules-v3` separates work title, title aliases, circle/artist, provider/
  release group, source series, event, edition, chapter/issue/volume relations,
  external IDs and resource-level tags with evidence.
- Chapter-only files use parent folders as work context; continuation/side-story/
  front/following parts receive stable relation and sort projections.
- SQLite now persists `scrape_jobs` and `scrape_proposals`.
- The full semantic proposal is persisted as versioned `semantic_json`, while the
  compatibility columns remain available to older callers.
- FRB API exposes pure parsing, batch scraping, proposal listing, and job listing.
- `AutomationCoordinator` is the single owner of startup, local-change debounce, and periodic triggers.
- SyncEngine keeps WebDAV transport, cooldown, and retry behavior but can run without its own timers in coordinator mode.
- The automatic cycle is snapshot/revision check -> catalog-only scrape -> safe-auto
  materialization -> transaction-external sync push; the parser itself remains
  proposal-only and zero-remote-I/O.
- Ready/conflict-free proposals write only empty canonical fields plus additive
  namespaced tags; manual values and review-required proposals are preserved.
- ScrapePanel exposes proposal evidence, input revisions, materialization status,
  skipped fields and errors for human review.

## Verification completed

- Rust focused DB, projection, scraper/API and sync snapshot tests pass.
- The catalog golden corpus test resolves both active and archived Trellis paths.
- Flutter `flutter analyze --no-pub`: no issues found.
- Flutter `flutter test --no-pub`: all tests passed.

## Current production condition

Production automatic materialization is enabled for `Ready` proposals with no
conflicts. The expanded 389-asset pass passed physical identity, context
normalization and accounting gates. Canonical writes remain local transactions;
the existing sync lane is scheduled only after a successful commit. Online
Provider enrichment and remote book-source I/O remain disabled for scraping.

## Asset identity and 115 normalization checkpoint (completed)

The safe-auto materialization decision is approved after introducing a physical
`asset_key`, normalizing local/Quark/115 `CatalogSnapshot` context, and making
run accounting reject proposal collisions or count mismatches. The coordinator
now uses the safe-auto eligibility gate described below.

The previous production default is superseded by:

```text
all catalog states -> local proposal/review queue
Ready + conflict-free -> local canonical materialization -> sync lane
```

This stage must preserve the catalog parser's zero-byte-read and zero-remote-
book-source invariants. It must not add Provider calls, content extraction,
remote directory refresh or inline WebDAV sync.

Implementation order:

1. Add physical asset identity and migrate scrape working-state primary keys.
2. Normalize local/Quark/115 catalog context from persisted index rows only.
3. Add parser safety gates for weak ancestors, structural filenames and mixed
   sibling sequence kinds.
4. Add proposal/run accounting invariants and expanded 389-item regression
   output.
5. Enable the gated local materialization and transaction-external sync step
   after the corrected corpus passes identity, normalization and accounting.

## 2026-08-23 implementation result

- `asset_key = asset|source_type|source_id|library_index.id` is now the
  physical key for proposals, queue rows and materialization provenance;
  logical `book_key` remains the canonical metadata/sync grouping key.
- Legacy scrape tables migrate their old `book_key` primary keys to
  `asset_key`; `.cbz` and `.zip` assets with one logical stem remain separate.
- `CatalogAssetContext` consumes only persisted `library_index` rows. It
  normalizes flattened 115/Quark names, filters category buckets such as
  `日漫`, skips synthetic self-file ancestors, and removes the current file
  from sibling corroboration.
- `06+续`/`08+续` and their plain numeric siblings are interpreted as one
  chapter stream; multi-axis `季`/`话` ranges are stored as typed
  `season_range`/`chapter_range`; `[完結]`/`[End]` becomes resource
  completeness.
- The run now fails before success on physical asset collisions or proposal
  count mismatch. The copied 389-item corpus reports
  `input_assets=389`, `proposals_written=389`, `asset_collision_count=0`,
  `accounting_status=pass`.
- Corrected export: `docs/reports/catalog-dry-run-2026-08-23-after-normalization.json`.

## Manual validation notes retained

Do not add Provider calls or more parsing heuristics without a new corpus review.
Canonical writes are now enabled only through the `Ready` + conflict-free local
transaction gate. Manual validation should continue to check:

1. title vs chapter separation, including `10续` and chapter-only folders;
2. circle/artist/author vs provider/release-group separation;
3. ancestor depth and nested folder semantics;
4. missing-author, title-alias and ambiguous cases;
5. resource completeness/edition/source tags are not promoted to work state;
6. RemoteOnly entries produce proposals without source I/O.

## 2026-08-23 semantic follow-up

- Ancestor title candidates now use the same leading-bracket grammar as
  filenames, so `[Vchan]Work` yields `Work` plus an unresolved attribution
  candidate instead of contaminating the work title.
- Parenthetical `上+下`/full-width variants are sequence-member evidence and
  no longer become source-context candidates.
- `全集` marks a resource collection, while `End`/`完結` remains a completion
  hint; compound labels such as `全集无修正` preserve both collection and
  uncensored attributes.
- `フルカラー版` is classified as `color_state=full_color`.
- The same copied 389-asset corpus passes accounting and is now eligible for
  the production `Ready` + conflict-free local materialization gate.
- Canonical auto-materialization is enabled; the sync push remains a separate
  coordinator step after the local transaction and never runs from the parser.
- Final semantic export: `docs/reports/catalog-dry-run-2026-08-23-after-semantic-fixes.json`.

## 2026-08-23 isolated materialization validation

- `app/rust/examples/catalog_materialize_dry_run.rs` runs the same catalog
  scrape and `Ready + conflicts=[]` projection against a copied SQLite file.
- The copy pass applied `389/389` eligible proposals, leaving `397` canonical
  metadata rows, `169` tags and `2250` book-tag links in the copy.
- The report explicitly records `remote_book_source_io=false`,
  `provider_requests=false`, `content_reads=false` and
  `sync_transport_invoked=false`. The real database and all remote sources
  were untouched.
- Export: `docs/reports/catalog-materialize-dry-run-2026-08-23.json`.

## 2026-08-23 production local projection

- Created a byte-identical backup at
  `D:\Temp\rch-production-backup-20260823-01\database.db` before touching the
  production database.
- Applied the enabled safe-auto path locally to `D:\Documents\RCH\database.db`:
  `389/389` proposals applied, `397` metadata rows, `169` tags and `2250`
  links. Physical asset accounting remained `389/389` with zero collisions.
- Re-running the same pass skipped all `389` rows and produced no additional
  writes, confirming proposal-revision idempotency.
- This command invoked no source adapter, remote book-source I/O, Provider
  request or sync transport. The separate sync push remains owned by
  `AutomationCoordinator` on its next enabled application cycle.
- Production report: `docs/reports/catalog-materialize-production-apply-2026-08-23.json`.

## 2026-08-23 FRB native hash repair

- A Windows launch reported Dart hash `1918694319` versus the stale native
  library hash `-686510377`. The generated Dart/Rust sources already agreed;
  only `rust/target/release/rust_lib_app.dll` had not been rebuilt.
- Running `app/codegen.ps1` regenerated the bindings and rebuilt the release
  DLL. A native `RustLib.init()` smoke test passed, followed by clean Flutter
  analysis and all `58` Flutter tests.
- After FRB/Rust API changes, use `app/codegen.ps1` and a cold restart; hot
  reload/restart cannot replace a loaded native DLL.
