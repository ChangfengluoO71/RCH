# M8 智能刮削：Catalog-Only 通用识别闭环 — 技术设计

## 1. Architecture Boundary

```text
SQLite catalog snapshot
  filename + ancestor directories + persisted catalog fields
                 │
                 ▼
Catalog-only parser / rule engine
  tokenization → chapter/platform filtering → ancestor relation → role classification
                 │
                 ▼
NameRoleProposal
  title? author? provider? volume/chapter? evidence + conflicts + rule version
                 │
                 ├── local preview / scrape working proposal
                 └── later Provider enrichment and user confirmation
```

The first usable version has no `ByteSource`, `Downloader`, remote source session, Provider client, or sync transport dependency. It consumes the same persisted catalog information for local and RemoteOnly assets.

## 1.1 Automation Coordinator and sync integration

The scraper is a background job, not a second sync implementation. A planned `AutomationCoordinator` reuses the existing `SyncEngine` lifecycle while keeping three lanes explicit:

```text
AutomationCoordinator
 ├─ sync_transport       → existing SyncEngine / WebDAV sync state
 ├─ catalog_scrape       → CatalogSnapshot → local NameRoleProposal
 └─ provider_enrichment  → AniList/Bangumi + provider_cache (optional)
```

There is one scheduler owner: after integration the coordinator owns startup/timer/debounce triggers; `SyncEngine` remains the sync executor and status adapter. Existing UI `syncNow` and `setAutoSync` calls are compatibility facades and must not start a parallel timer or bypass coordinator locks.

### Triggers

1. **Startup**: after `LibraryStore`, `FolderSnapshotStore`, `LibraryCatalogStore`, and `SyncManager` are ready, enqueue the configured sync cycle. When that cycle completes, consume the local catalog revision delta and enqueue scrape jobs. If sync is disabled or unconfigured, start the local lane immediately.
2. **Catalog revision**: a persisted catalog/index revision change starts the same 2-second debounce used by local sync changes. The event contains changed `book_key`s; it never asks a source adapter to list, stat, HEAD, PROPFIND, download, or refresh.
3. **Sync completed**: pull/apply changes are treated as local SQLite changes. The coordinator schedules one scrape job per `(book_key, catalog_revision, rule_version)` and never re-reads the book source.
4. **Periodic tick**: the existing foreground 60-second tick lets the sync lane apply its own remote-revision/backoff policy and independently drains due local scrape jobs. A scraper tick cannot issue a remote check.
5. **Manual**: “Run scrape now” drains local jobs only; “Run full cycle” runs sync and then local scrape. Neither path is an implicit remote catalog scan.

The coordinator must classify events instead of wiring every `LibraryStore` notification to every lane: catalog/index revision changes enqueue local scrape, canonical sync-dirty enqueues transport, and working proposal/candidate/evidence/Provider-cache writes enqueue nothing. This is the loop-prevention rule for `catalog → scrape → confirm → sync`.

### Cycle ordering and isolation

```text
sync_transport (optional)
        ↓ completion
read local catalog delta
        ↓
catalog_scrape → working proposal
        ↓ optional
provider_enrichment
        ↓ user review
confirm_proposal → SQLite canonical transaction + sync-dirty
        ↓ later, outside transaction
existing sync automation
```

`catalog_scrape` accepts no `ByteSource`, Downloader, source session, remote URL, or sync handle. `sync_transport` remains the only lane allowed to call WebDAV sync state. `provider_enrichment` receives normalized text only. Each lane has its own active-job lock and error/backoff state; a Provider or scraper failure cannot cancel sync, and a sync failure cannot make local proposals unavailable.

### Queue and restart semantics

Use a persisted job record keyed by job kind, scope, input revision and rule/provider version. A newer catalog revision supersedes an older queued scrape, while a running job finishes against its captured snapshot and schedules the newer revision once. Operational failures enter bounded retry; missing context and role conflicts are terminal `degraded` results. On app pause/exit the queue is flushed; on resume/startup due jobs are recovered. This is still foreground-lifecycle automation, not an OS background service.

## 2. Input Contract

```rust
struct CatalogSnapshot {
    book_key: String,
    filename: String,
    ancestor_dirs: Vec<String>, // nearest first; sourced from local SQLite
    source_fingerprint: Option<String>,
    size: Option<i64>,
    modified_at: Option<i64>,
    etag: Option<String>,
    user_title: Option<String>,
    user_author: Option<String>,
}

struct CatalogParseRequest {
    snapshot: CatalogSnapshot,
    ancestor_depth: u8, // default 3
    rule_version: String,
}
```

`ancestor_dirs` is a snapshot, not a request to enumerate a source. If the source has not been browsed deeply enough to populate a parent name, the parser records missing context and continues.

## 3. Normalization and Role Rules

The parser is deterministic and keeps the original token spans for explanation.

### 3.1 Tokenization

- Normalize separators, Unicode whitespace, common bracket pairs, and case-folded comparison values while preserving display text.
- Split filename stem and each ancestor directory into tokens by separators (`[](){}-_`, whitespace, dots) without discarding the original span.
- Detect volume/chapter/edition markers first: `Vol`, `Volume`, `Ch`, `Chapter`, `第…卷`, `第…话`, `话`, `卷`, `OVA`, `特典`, numeric range markers and configured aliases.
- Mark detected chapter tokens as structural metadata; they cannot be selected as title tokens.

### 3.2 Provider / Platform Classification

- Apply explicit provider/platform markers and a versioned provider lexicon before author inference: bracketed group/publisher/platform tokens, known platform/source aliases, `raw`, `scan`, `汉化组`, `出版社`, and configured provider labels.
- A provider candidate receives negative author weight when it matches a provider lexicon or structural provider marker.
- Provider is optional; unknown group names remain `unknown_role` rather than being forced into author.

### 3.3 Author Classification

- Apply explicit author markers (`作者`, `原作`, `作`, `画`, `著`, `by`, `author`) and configured person-name patterns.
- A candidate already classified as provider/platform or chapter cannot become author unless an explicit author marker overrides it; the conflict is retained in evidence.
- Multiple authors are represented as a list; an absent or ambiguous author is valid.

### 3.4 Title and Ancestor Relation

- Remove structural chapter/volume/provider tokens from title candidates.
- Score repeated/stable tokens shared by filename and nearest non-provider ancestor as title evidence.
- Compare up to `ancestor_depth` ancestors: nearest title-like directory may be a series/work title, the next may be author or provider, and deeper directories may be collection/category context.
- Prefer explicit title markers and repeated filename/ancestor agreement over depth-only guesses.
- A title candidate containing a chapter/volume marker is rejected or split into `{title, chapter}`; it is never silently retained as a title.

### 3.5 Proposal Output

```rust
struct NameRoleProposal {
    pub title: Option<RoleValue>,
    pub authors: Vec<RoleValue>,
    pub provider: Option<RoleValue>,
    pub volume: Option<RoleValue>,
    pub chapter: Option<RoleValue>,
    pub evidence: Vec<RoleEvidence>,
    pub conflicts: Vec<RoleConflict>,
    pub state: ParseState, // ready, partial, ambiguous, unmatched
    pub rule_version: String,
}
```

Each `RoleValue` carries original text, normalized text, source (`filename` or `ancestor(level)`), matched rule, confidence, and span. `RoleConflict` is visible to the caller and prevents silent role mixing.

## 4. Working-State Storage

The first version may persist the parser result in a local working table such as `scrape_name_proposals` keyed by `(book_key, rule_version)`. It must contain the proposal JSON, status, created/updated timestamps and rerun information. It does not write `works`, `work_links`, `book_metas`, or sync payloads.

## 5. Optional Provider Enrichment

After the catalog proposal exists, AniList/Bangumi may receive a normalized query built from `title` and optional `authors`. Provider calls are best-effort and never replace the local proposal. Provider errors produce enrichment status only; no remote book source fallback is permitted.

## 6. Confirmation and Later Canonicalization

M8-M2 introduces independent ordered DDL and canonical tables after the parser contract is stable. M8-M5 review accepts or edits a proposal and then atomically materializes `works`, `work_external_ids`, `work_links`, and provenance. The transaction only marks local sync-dirty; existing sync scheduling handles transport later.

## 7. Validation Matrix

| Input | Expected result | Network allowed |
|---|---|---|
| Local file catalog | title/author/provider/chapter proposal | none |
| Fully cached remote asset catalog | same catalog-only proposal | none |
| RemoteOnly asset catalog | filename + ancestor-only proposal; missing fields allowed | none to book source; Provider only when enrichment is explicitly requested |
| Missing author | `author = None`, explainable missing evidence | none |
| Provider unavailable | local proposal remains usable; enrichment degraded | no book-source fallback |

Required fixtures must cover provider/author collisions, title/chapter collisions, multi-level ancestors, bracket groups, numeric volumes, and absent metadata.

## 8. Compatibility

- Existing `DocumentMeta`, `ByteSource`, source adapters, reading, and catalog indexing remain unchanged.
- Existing remote source browsing may continue to populate the catalog under its normal user-driven behavior; starting a scrape job cannot refresh it.
- No automatic file rename, move, sidecar write, or sync transport is introduced by the first version.
- Existing sync automation remains responsible for transport and its current retry/limit policy; the coordinator only sequences it with local scrape jobs and observes sync-dirty after confirmation.
