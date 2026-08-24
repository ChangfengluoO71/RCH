# Sync / Scrape Automation Review — 2026-08-22

## Existing automatic sync behavior

The current Dart `SyncEngine` is initialized from `app/lib/main.dart` after the library and catalog stores are loaded. It:

- loads `sync_auto` (default enabled), registers a `LibraryStore` listener, and starts a 60-second foreground timer;
- coalesces local changes with a 2-second debounce and skips while `_syncing` or inside rate-limit/backoff cooldown;
- performs a startup sync, uses a lightweight remote revision check during periodic polling, and retains 429/503 15-minute cooldown plus 30-second-to-15-minute exponential retry;
- calls the existing sync API and refreshes in-memory stores after a successful apply;
- runs local index refresh / snapshot-based index building before sync transport. The latter is not a scraping permission and must not be called by the scraper.

`SyncManager` remains configuration, device and backup state. The existing archived sync design also describes the same pull-first intent, but the current `SyncEngine.syncNow()` is the active transport entry and should be wrapped by an adapter rather than duplicated.

## Required integration decision

Add a planned `AutomationCoordinator` / `M8-A0` layer above the existing engine. It owns job lifecycle, deduplication, revision cursors, cancellation, restart recovery and status. It does not absorb WebDAV transport or turn `SyncEngine` into a generic network client.

There must be one scheduler owner. Because the current `SyncEngine` already owns a startup call, a 60-second timer and a 2-second listener debounce, the implementation must migrate those triggers behind the coordinator (or expose an explicit coordinator mode) before adding scrape triggers. Keeping both schedulers would cause duplicate syncs, race the scrape ordering and invalidate backoff accounting. Public UI methods remain compatibility facades routed through the coordinator.

The lanes are:

```text
sync_transport       → existing SyncEngine / Rust sync state
catalog_scrape       → persisted SQLite CatalogSnapshot only
provider_enrichment  → AniList/Bangumi metadata API + provider_cache only
```

Default cycle: `sync_transport` when enabled/configured → read local catalog delta → `catalog_scrape` → optional Provider enrichment. With no sync endpoint, local catalog scraping starts after database initialization. A confirmation writes canonical data and sync-dirty locally; a later normal sync observes it.

## Hard boundaries

- Scrape jobs cannot call remote book-source list/stat/HEAD/PROPFIND, `ByteSource`, Downloader, source sessions, cover reads or comic downloads.
- A missing catalog field is a degraded local result, not a reason to refresh the source.
- Working proposals, evidence, candidates and Provider cache do not enter sync snapshots.
- Sync and Provider failures are independent from local scraping; automatic scraping never silently confirms canonical metadata.

## Acceptance tests

1. Startup, catalog revision, sync completion, periodic tick and manual triggers produce deduplicated jobs.
2. Sync completion imports a catalog delta and creates exactly one scrape job per input revision.
3. No-sync configuration still runs catalog-only scraping.
4. RemoteOnly automatic scraping has zero book-source requests, ByteSource reads, stat/HEAD/PROPFIND and downloads; Provider calls are counted separately.
5. Confirmation succeeds while transport is offline and only marks sync-dirty.
6. Repeated ticks, restart recovery and independent failure/backoff do not create loops or block reading.

## Open implementation gate

Before production code, decide whether the first adapter can call the existing `syncNow()` as one pull/apply/push cycle or whether the sync API must expose explicit pull-complete and push-due events. Either choice must preserve the externally visible `sync → local scrape` ordering and no inline sync on confirmation.
