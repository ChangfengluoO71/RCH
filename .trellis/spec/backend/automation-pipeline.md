# Automation Pipeline Contracts

> This guide defines the planned M8 coordinator that composes the existing sync automation with local-first scraping. It does not replace the current `SyncEngine` transport implementation.

## 1. Scope / Trigger

- The coordinator owns scheduling, deduplication, lifecycle, persistence, cancellation and observability for `sync_transport`, `catalog_scrape` and optional `provider_enrichment` jobs.
- It is initialized after the database, library catalog, folder snapshots and sync configuration are loaded.
- Triggers are startup, persisted catalog revision changes, sync-cycle completion, the existing foreground periodic tick, manual actions and bounded retry.
- There is exactly one scheduler owner for startup, timers and debounce. After coordinator integration, `SyncEngine` is the sync executor/status adapter; its legacy internal timer/listener must not run in parallel.

## 2. Signatures

```rust
enum AutomationJobKind {
    SyncTransport,
    CatalogScrape,
    ProviderEnrichment,
}

enum AutomationJobState {
    Queued,
    Running,
    Succeeded,
    Degraded,
    RetryWait,
    Failed,
    Cancelled,
}

struct AutomationJob {
    job_id: String,
    kind: AutomationJobKind,
    scope_key: String,
    input_revision: String,
    state: AutomationJobState,
    attempt: u32,
    next_run_at: i64,
}

fn enqueue(job: AutomationJob) -> Result<(), AutomationError>;
fn run_due_jobs() -> Result<AutomationSummary, AutomationError>;
```

`CatalogScrape` accepts only a persisted `CatalogSnapshot`; it cannot accept `ByteSource`, Downloader, a remote source session, a remote URL or a sync transport handle. `ProviderEnrichment` accepts normalized local text/query data only.

## 3. Contracts

- Default cycle ordering is `catalog_snapshot` (local/persisted snapshots only) →
  `sync_transport` pull (when enabled/configured) → `catalog_scrape` → safe-auto
  `materialization` → transaction-external `sync_transport` push → optional
  `provider_enrichment`.
- The catalog lane uses the existing 2-second local-change debounce and coalesces by `(book_key, catalog_revision, rule_version)`.
- A newer queued revision supersedes an older queued scrape. A running job finishes against its captured snapshot and schedules the newer revision once.
- Event routing is typed: catalog/index revision → `CatalogScrape`; canonical sync-dirty → `SyncTransport`; working proposal/candidate/evidence/Provider-cache writes → no remote job. Do not connect every broad `LibraryStore` notification to both lanes.
- `confirm_proposal` performs a local transaction and emits sync-dirty; it never invokes the sync lane inline.
- Working state and Provider cache are not included in sync snapshots. Only confirmed canonical data and explicit sync-dirty changes are eligible for transport.
- Each lane has an independent active-job lock and failure/backoff state. Sync failures cannot block local scrape; Provider failures cannot erase local evidence.
- The current lifecycle remains foreground-only. Pause/exit flushes job state; resume/startup recovers due jobs. No OS background execution is implied.
- Existing UI `syncNow` and `setAutoSync` remain compatibility APIs routed through the coordinator; they must not bypass queue locks or create a second sync cycle.
- Safe-auto materialization is eligible only for `Ready` proposals with an empty
  conflict list. It writes empty canonical metadata fields and additive
  namespaced tags in one Rust SQLite transaction, preserves manual values, and
  records `(book_key, input_revision)` provenance. The transaction has no sync
  transport capability; the coordinator schedules the push only after commit.

## 4. Error Matrix

| Condition | Required behavior |
|---|---|
| Sync 429/503 | Keep existing 15-minute cooldown; local scrape remains runnable |
| Sync network/merge error | Keep existing exponential backoff; do not cancel local jobs |
| SQLite transient busy | Bounded local retry with job state |
| Missing ancestor/role conflict | `Degraded` proposal with evidence; no remote fallback |
| Provider offline/timeout/limited | Typed degraded enrichment; local proposal remains usable |
| Stale confirmation | Reject without canonical or transport write |

## 5. Good / Base / Bad

- Good: a sync cycle imports a new catalog row, the coordinator enqueues one local scrape job, and a proposal is available without reopening the source.
- Base: no sync endpoint is configured; startup still drains local catalog scrape jobs.
- Bad: a periodic scrape calls `remote revision`, `PROPFIND`, `stat`, `ByteSource::read_at` or a comic download to fill a missing field.

## 6. Tests Required

- Scheduler tests cover startup ordering, debounce/coalescing, revision supersession, no-loop behavior, restart recovery, cancellation and independent backoff.
- Integration tests prove sync completion creates exactly one scrape job, no-sync mode still scrapes, and confirmation emits sync-dirty without calling transport.
- RemoteOnly tests assert zero book-source requests, ByteSource reads, HEAD/stat/PROPFIND and downloads during automatic scraping. Provider requests are measured separately.

## 7. Wrong vs Correct

### Wrong

```rust
async fn scrape_on_tick(source: Arc<dyn ByteSource>) {
    let _ = parse_zip(source).await; // hidden Range I/O can contact WebDAV/SFTP.
}
```

### Correct

```rust
async fn scrape_on_catalog_revision(snapshot: CatalogSnapshot) {
    enqueue_catalog_scrape(snapshot).await?; // SQLite snapshot only.
}
```
