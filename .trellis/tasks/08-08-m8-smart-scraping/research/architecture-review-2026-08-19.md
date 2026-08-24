# M8 architecture review against v0.5.1

## Scope

This review compares the 2026-08-08 M8 plan with the current v0.5.1 codebase. It does not approve implementation or revise the product scope by itself.

## Current architecture evidence

- `app/rust/src/db/mod.rs:131-486` initializes and evolves the SQLite schema in place. `schema_version` and `CURRENT_SCHEMA_VERSION` at `:551-569` describe the one-time `library.json` import, not a general ordered DDL migration ledger.
- `app/lib/repository/book_repository.dart:1-5` makes `BookRepository` the owner of `BookSource` and `BookMeta`; `LibraryStore` is the UI-facing facade. Rust-side writes performed outside this path require an explicit Dart reload, as the sync engine already does.
- `app/rust/src/sync/base.rs:11-17` and `sync/snapshot.rs:338-350` enumerate seven synchronized entity types explicitly. Metadata alone gets field-level three-way merge (`sync/merge.rs:128-188`); other entities use entry-level LWW.
- ADR-021 (`docs/project/DECISION.md:327-334`) separates reconstructible physical discovery data from irreplaceable user cognition data. ADR-024 (`:366-377`) defines WebDAV state files and three-way merge; `.rchpkg` is not the daily sync protocol.
- `app/rust/src/document/mod.rs:18-35` defines a four-field `DocumentMeta` used by every document reader. `document/comicinfo.rs:15-31` already parses many richer fields but collapses them to those four fields at `:41-49`.
- `app/rust/src/source/mod.rs:65-75` provides random-access `ByteSource`. Remote sources are deliberately streaming/range-oriented; requiring a full-file SHA-256 would download every remote comic.
- `app/rust/Cargo.toml:11-16` enables Tokio without the `time` feature and reqwest with both async JSON support and the blocking feature. The existing downloader is a blocking, file/range transport and is not a suitable provider API abstraction.
- `app/lib/store/models.dart:373-449` treats `BookMeta` as per-asset user metadata. `bookKeyOf` at `:500-502` is local-source keyed, while the sync layer converts it to a stable `source_fingerprint + normalized_path` identity.

## Required design corrections

### 1. Separate canonical knowledge from scrape working state

`works.status = unprocessed/suggested/confirmed/rejected/manual` mixes two lifecycles. A work is canonical knowledge; `suggested`, `rejected`, and confidence belong to a match proposal.

Recommended split:

- Canonical and syncable: `works`, `work_external_ids`, `work_links`, confirmed field provenance, user-curated tag synonym rules.
- Local/reconstructible by default: `scrape_jobs`, `scrape_candidates`, `scrape_evidence`, `provider_cache`, fingerprints, OCR output, embeddings.
- A confirm transaction materializes or updates the canonical work, links the local book asset, applies confirmed tags, records provenance, and closes the proposal.

This also removes the contradictory requirement that suggested data must be persisted for resumability but must not be written into canonical metadata before confirmation.

### 2. Use an explicit asset-to-work relation

`book_metas.work_id` is too narrow for the stated model of volumes, chapters, editions, and alternate files. Use a relation such as:

```text
work_links(work_id, book_id, relation_kind, volume, chapter, edition, updated_at, deleted)
```

`book_id` must use the sync layer's stable identity, not the local `type|source_id|path` key. A separate relation preserves per-file `book_metas`, supports future many-to-many cases such as anthologies, and gives tombstones an unambiguous sync key.

### 3. Do not overload the legacy import marker as DDL migration state

Introduce an ordered migration runner (`PRAGMA user_version` or a dedicated `db_migrations` table) before adding M8 schema. Keep `CURRENT_SCHEMA_VERSION = 2` solely for the existing JSON-to-SQLite import contract unless that contract is deliberately redesigned and regression-tested.

### 4. Define synchronization per entity, not per table

The original statement that every new table joins `.rchpkg` incremental sync is obsolete and conflicts with ADR-024. Each new entity needs:

- stable sync key;
- whether it is user-authored/canonical or reconstructible/local;
- merge policy (field-level three-way, set union + tombstone, or LWW);
- snapshot/apply/pending-resolution behavior;
- protocol compatibility and downgrade behavior.

Provider response caches, raw evidence, OCR results, pHash, and embeddings should remain local by default. Confirmed works, work links, manual edits, and user-curated tag mappings should sync.

### 5. Make fingerprinting source-aware and budgeted

Replace mandatory `file_sha256` with a tiered identity:

1. Stable asset identity from source fingerprint + normalized path.
2. Cheap observation fingerprint from size, modified time/etag, page count, and selected page hashes.
3. Strong full SHA-256 only for local or already fully cached files, computed lazily.

The fingerprint record must include algorithm/version so future changes do not create false matches. pHash is a similarity signal, not a primary-key-quality identity.

### 6. Add a rich extraction DTO without widening reader metadata

Keep `DocumentMeta` stable for the reader. Add an `ExtractedMetadata`/`MetadataEvidence` contract owned by the scraper, with typed fields, normalized values, source, confidence, and warnings. Format adapters can reuse existing parsers without forcing every `Document` implementation and FRB consumer to change at once.

### 7. Respect current repository ownership and UI refresh semantics

Recommended boundary:

- Rust `scraper` owns analysis, provider calls, ranking, confirmation transactions, and persistent query APIs.
- Dart `WorkRepository` owns the canonical work read model exposed to Flutter.
- Dart `ScrapeController` owns only job progress, candidate selection, and user interaction state.
- `LibraryStore` coordinates reload/notifications after confirm or sync, rather than duplicating work data inside `BookRepository`.

FRB should expose typed coarse-grained operations (`start_scan`, `get_job_snapshot`, `confirm_proposal`, `cancel_job`) instead of table CRUD.

### 8. Use a dedicated provider HTTP runtime

Do not route metadata APIs through the existing range/file downloader. Build a shared provider client around async reqwest with user-agent, timeout, concurrency, retry-after handling, cancellation, and response size limits. Provider failures must be typed so the pipeline can distinguish offline, rate limited, unauthorized, not found, malformed response, and transient server failure.

### 9. Reduce the first delivery scope

The original nine phases combine identity, migrations, four external providers, taxonomy, a batch-review UI, OCR, visual matching, semantic search, recommendations, and Obsidian export. That is several independently valuable products and locks the schema before the core matching loop is validated.

Recommended first vertical slice:

1. Migration framework and canonical work/link model.
2. Filename + embedded metadata evidence and tiered local fingerprint.
3. Provider framework with AniList and Bangumi only.
4. Pure ranking with explainable evidence.
5. Review/confirm UI and confirmation transaction.
6. Sync only confirmed canonical entities.

Defer MangaDex/ComicVine, tag ontology/merge tooling, OCR/CLIP, semantic search, recommendations, and Obsidian export to later child tasks after the first 100-book corpus validates the model.

## Proposed module boundary

```text
Flutter
  WorkRepository (canonical read model)
  ScrapeController (job/review state)
        |
       FRB: job-oriented typed API
        |
Rust scraper/
  service.rs       orchestration/cancellation/progress
  extract/         filename + embedded metadata evidence
  fingerprint.rs   source-aware versioned fingerprints
  provider/        async provider clients + cache policy
  rank.rs          pure explainable scoring
  confirm.rs       canonical transaction
        |
  db/work_repository.rs + db/scrape_repository.rs
        |
  SQLite canonical entities / local working entities
        |
  sync adapters for confirmed entities only
```

## Blocking product decision

Decide whether the first M8 delivery is the recommended vertical slice or retains the original all-in-one M8.1-M8.9 scope. This decision changes the schema commitment, task tree, acceptance criteria, and the point at which users get a usable feature.
