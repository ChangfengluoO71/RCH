# Long-running work inventory — exit prompt

This inventory records the existing lifetime owner and the exit behavior added
for the Windows close prompt. The registry is an in-memory display aid; each
domain store remains authoritative for persistence and recovery.

| Work | Lifetime owner | Existing stop/checkpoint/recovery behavior | Exit registration |
| --- | --- | --- | --- |
| EH manual and Daily collection | `EhSubscriptionStore.run()` | Existing Rust cancellation API; collection writes the manifest/checkpoint in its `finally` path. Daily state records interrupted/failed runs. | One lease for the EH scan. Exit requests cancellation and waits up to 10 seconds for the run to leave `busy`. The lease is released before a daily Cloud115 batch starts. |
| Cloud115 batch writes | `Cloud115BtBatchProcessor.run()` | Private JSONL intent/result ledger is flushed around writes. A later user run can reconcile unknown outcomes; a batch is not automatically restarted. | One lease per processor run, with ledger-safe phase labels and no cancellation callback. The dialog warns it may be interrupted; the remote 115 job is never cancelled. |
| Cloud115 status monitoring | `Cloud115BtMonitor` | SQLite is only a projection reconciled from JSONL. Account-scoped leases recover after expiry; next due time is durable. | Shown as resumable monitoring work in the exit dialog, not as a perpetually active registry lease. Local polling stops on exit and starts again with RCH. |
| Remote session warm-up | `RemoteScanCoordinator.warmUpSessions()` | Best-effort, serial setup from already saved source credentials; per-source errors do not abort other sources. Startup repeats the attempt from the existing saved source configuration. | One lease around the whole warm-up pass, with a source-count phase. It is not advertised as resumable while the process is closed. |
| Remote source directory and related cover progress | `RemoteScanCoordinator._startShared()` / its progress monitor | Existing pause/resume/cancel calls and persisted scan status/checkpoint are retained. A source has one progress monitor. | One source-scoped lease. Exit uses the existing cancel endpoint and releases the lease when cancelled or terminal. Unknown/failed status keeps the prompt visible until resolved/disposed. |
| Source catalogue index | `LibraryIndexService.refreshSourceIndex()` / `_SourceRefreshFlight` | Existing local incremental scan or caller-provided remote listing and SQLite replacement. Single-flight coalescing remains the concurrency owner; there is no general index-job resume queue. | A standalone source refresh gets one lease. Calls nested under automation/sync suppress the child lease because the parent operation already owns the lifetime. |
| Saved folder snapshot index | `LibraryIndexService.buildIndexFromSnapshots()` | Rebuilds the searchable index from persisted folder snapshots; it does not fetch remote listings. The existing SQLite index remains authoritative. | One lease for direct/standalone indexing. Snapshot indexing nested in catalog refresh suppresses a duplicate lease. |
| Automatic sync/catalog cycle | `AutomationCoordinator.runCycle()` | Existing sync state, catalog SQLite transactions, and coordinator coalescing remain authoritative. There is no whole-cycle cancellation callback. | One lease around the complete coordinator cycle. Nested background SyncEngine requests do not create a second lease. It is not represented as resumable. |
| Manual catalog scrape | `AutomationCoordinator.runScrapeNow()` | Existing foreground-work gate and Rust catalog transaction; no separate durable run queue. | One lease around the manual coordinator entry point. |
| Manual WebDAV sync | `SyncEngine.syncNowWithOutcome(background: false)` | Existing WebDAV Sync State, cooldown, and backoff; no new cancellation protocol. Background calls made inside automation rely on the outer automation lease. | One lease only for standalone foreground sync, avoiding a duplicate nested registration. |
| AI upscale queue | `AiUpscaleManager._runTask()` | Per-page progress and running state are persisted in the existing AI task table; startup requeues stale running tasks. | One lease per executing task. Exit callback persists the latest task row; task is marked resumable. |
| Backup restore/export | `SyncManager.restoreFrom()`, `restoreFromWithCredentials()`, `exportToFile()` | Existing Rust package import/export operation; no cancellation/resume protocol exposed. | A lease for each active operation, marked non-resumable so the dialog warns before exit. |
| Cache clear operations | `CacheManagerPanel._clear()` and `_clearAllWithChoices()` | User-confirmed local cache/read-history deletion. There is no rollback/resume protocol. | A lease begins only after confirmation and covers the full clear plus refresh. |
| Cache root migration | `CacheManagerPanel._changeCacheDir()` / `_restoreDefaultCacheDir()` and startup recovery in `_offerResumeMigration()` | Existing migration marker and explicit startup continuation prompt recover a copied root. The in-process operation flushes the LibraryStore, copies, switches/reopens the root, then removes the old copy. | One lease spans flush through old-copy cleanup, rather than only the file-copy helper. It has no cancellation callback; existing startup recovery remains unchanged. |
| Update package download | `UpdateManager.download()` | Existing single-flight download, `.part` staging, package verification, and mirror retry. No promise is made that transfer continues after process exit. | One lease for the active download; no cancellation callback. The external installer handoff is intentionally not registered as RCH work. |

## Ownership and exclusions

- The EH store releases its lease before delegating the Cloud115 stage to the
  batch processor, so one Daily workflow is not shown as two owners at once.
- The monitor's scheduled wait is durable resumable state, not an active task.
- The monitor uses a single bounded read-only task-list snapshot per due account; absence is not inferred from an incomplete listing.
- Remote cover progress uses the existing remote scan progress lane; no second
  cover lease or new cover poller was added.
- Short settings, file-picker, status-refresh, and one-shot cache-size reads do
  not receive leases.
- Standalone catalogue indexing and cache operations receive leases only for
  their long operation boundary; nested automation/sync indexing is covered by
  its owning parent lease.
- Update installation is an operating-system handoff. Intercepting the
  installer-owned application shutdown would obstruct the normal update flow.
- None of these leases records account identifiers, cookies, access tokens,
  request payloads, full paths, or task results.
