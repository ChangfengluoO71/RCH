# M8-A0 自动化调度与同步集成技术设计

## 1. Existing sync behavior to preserve

当前 `SyncEngine` 已提供以下产品语义：启动时尝试同步；`LibraryStore` 变化后 2 秒防抖；60 秒轻量轮询；同步中跳过重复触发；429/503 进入 15 分钟冷却；普通失败按 30 秒、1 分钟、2 分钟、4 分钟、8 分钟、15 分钟退避。它还会在同步前刷新本地索引或从已保存 snapshot 建立远程源的离线索引。

M8 不复制这些策略，也不把同步实现改造成 scraper。新增协调器只负责把它们编排为任务；实际 WebDAV transport 仍留在 `SyncEngine`/Rust sync 层。接入时必须消除双重调度：协调器成为唯一的 Timer/防抖/队列触发所有者，`SyncEngine` 保留 `syncNow()`、transport 状态和可复用的冷却/退避执行逻辑；现有 UI API 通过协调器转发。

## 2. Runtime model

```text
AutomationCoordinator
 ├─ SyncTransportLane       → existing SyncEngine / WebDAV sync state
 ├─ CatalogScrapeLane       → SQLite CatalogSnapshot → local proposal
 └─ ProviderEnrichmentLane  → AniList/Bangumi + provider_cache (optional)
```

Planned contracts:

```rust
enum AutomationJobKind { SyncTransport, CatalogScrape, ProviderEnrichment }
enum AutomationJobState { Queued, Running, Succeeded, Degraded, RetryWait, Failed, Cancelled }
enum AutomationTrigger { Startup, CatalogRevision, SyncCompleted, PeriodicTick, Manual, Retry }

struct AutomationJob {
    job_id: String,
    kind: AutomationJobKind,
    scope_key: String,       // catalog book_key or global sync scope
    input_revision: String,  // catalog revision + rule/provider version
    trigger: AutomationTrigger,
    state: AutomationJobState,
    attempt: u32,
    next_run_at: i64,
    last_error: Option<String>,
}
```

The catalog lane consumes only `CatalogSnapshot { book_key, filename, ancestor_dirs, persisted catalog fields }`. It must not accept a generic `ByteSource` or source adapter. The provider lane receives normalized text/query fields, never a comic file handle.

## 3. Cycle algorithm

```text
startup after DB/catalog load
  → enqueue sync_transport if enabled/configured
  → on sync completion, read local catalog revision delta
  → enqueue catalog_scrape for each changed book_key
  → optionally enqueue provider_enrichment after local proposal

catalog/index mutation
  → persist revision
  → debounce 2s and coalesce by book_key + revision + rule_version
  → run catalog_scrape without source refresh

periodic tick
  → ask sync lane whether its existing 60s/remote-revision policy is due
  → independently drain local scrape jobs
  → never perform remote checks for the scrape lane

confirm_proposal
  → SQLite canonical transaction
  → mark sync-dirty and emit canonical-changed
  → return; existing sync automation observes the change later
```

The coordinator should serialize the first startup cycle as `sync → scrape` so imported catalog changes are covered. Once the sync cycle is complete, local scraping may run while a later provider job is pending. If sync is disabled or unconfigured, the scrape lane starts after local initialization without waiting.

## 4. Queue, dedupe and persistence

- Reuse `scrape_jobs` for per-book scrape/proposal work and add the minimal orchestration fields (`trigger`, `input_revision`, `next_run_at`, `attempt`, `last_error`) in its ordered migration; canonical M8-M2 migration remains separate from working-state migration.
- Use a global sync job key and a per-book scrape key. A newer catalog revision supersedes an older queued job; a running job finishes against its captured snapshot and the newer revision is queued once.
- Provider jobs are keyed by proposal revision + provider + query hash. Provider cache hits may complete without a network call.
- Persist `degraded` as a terminal result for missing/ambiguous local fields; only operational failures enter retry.

## 5. Error and isolation matrix

| Lane | Failure | Action | Other lanes |
|---|---|---|---|
| Sync | 429/503 | existing 15-minute cooldown | scrape/provider continue from local state |
| Sync | network/merge error | existing exponential backoff | scrape remains runnable |
| Catalog scrape | SQLite busy/transient | bounded local retry | sync unaffected |
| Catalog scrape | missing ancestor/role conflict | degraded proposal with evidence | no remote fallback |
| Provider | offline/timeout/rate limit | typed degraded enrichment + cache/backoff | local proposal and sync unaffected |
| Confirmation | stale proposal | reject and require refresh | no canonical/transport write |

## 6. Lifecycle and UI

- Initialize coordinator after `LibraryStore`, `FolderSnapshotStore`, `LibraryCatalogStore`, and `SyncManager` are ready; this matches current `main.dart` ordering.
- Reuse the existing application lifecycle flush hook to persist queued jobs. On resume, a catch-up tick drains due local jobs; no OS background execution promise is added.
- Add one automation status surface later in M8-M5: pending scrape count, last scrape result/time, Provider degraded count, and the existing sync status remain separately visible.

## 7. Verification plan

- Scheduler unit tests: startup ordering, 2-second coalescing, revision supersession, no-loop after apply, retry classification, cancellation and restart recovery.
- Integration tests: sync pull/apply creates a catalog delta and exactly one scrape job; no sync configuration still runs catalog-only; confirmation emits dirty but does not call sync transport.
- Local-first spies: a RemoteOnly job asserts zero book-source requests, ByteSource reads, stat/HEAD/PROPFIND and downloads; Provider requests are counted separately.
- Cross-platform checks: Rust DB/API tests, Dart scheduler tests, `flutter analyze`, Windows end-to-end preview; Android must not block startup on optional Provider or sync failure.

## 8. Explicit non-goals

The coordinator is not a universal network client, not a remote crawler and not an auto-confirm engine. Any implementation that passes a remote source session or `ByteSource` into `CatalogScrapeLane` violates the architecture.
