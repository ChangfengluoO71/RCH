# Background Work and Foreground Responsiveness

This contract covers local indexing, automation sync, batch tagging, stale-data
cleanup, and the async state used by cloud-source setup. It extends
`automation-pipeline.md` with source freshness, bounded maintenance slices, and
the Flutter-to-Rust batch-write boundary.

## 1. Scope / Trigger

Use this contract when changing library index refreshes, automatic sync/scrape,
bulk metadata/tag writes, maintenance cleanup, or Quark/115 connect feedback.
The goal is to reduce repeated work and let active user operations reach their
next safe step without losing persisted changes or leaving maintenance starved.

## 2. Signatures

```dart
typedef SourceIndexRefreshResult = ({
  int entries,
  String message,
  bool changed,
  String revision,
  List<String> bookKeys,
});

Future<SourceIndexRefreshResult> LibraryIndexService.refreshSourceIndex({
  required BookSource source,
  bool force = false,
  Future<List<FolderSnapshotEntry>> Function(String path)? listRemote,
});

Future<SyncRunOutcome> SyncEngine.syncNowWithOutcome({
  bool refreshCatalog = true,
  bool background = false,
});

Future<BatchTagResult> LibraryStore.batchTag(
  BookSource source,
  Iterable<BatchTagTarget> targets,
  String tag, {
  void Function(BatchTagProgress progress)? onProgress,
});

Future<(int, int, int, int)> LibraryStore.purgeStaleData({
  bool alignRemote = true,
  bool background = false,
});

Future<T> ForegroundWorkCoordinator.runForeground<T>(
  Future<T> Function() work,
);
Future<void> ForegroundWorkCoordinator.waitForMaintenanceTurn();
```

The Rust batch boundary is
`db_apply_batch_tag_writes(Vec<BatchTagWriteDto>) -> Result<(), String>`;
each write carries `book_key`, `tag_name`, and optional `BookMetaDto`. Rust
applies one bounded list in a single SQLite transaction.

`SyncRunOutcome` carries `status`, `message`, optional remote `revision`,
`changedEntities`, and `changesKnown`. `BatchTagResult` carries `total`,
`succeeded`, `skipped`, and `failed`; `BatchTagProgress` additionally carries
the current phase and `processed` count.

## 3. Contracts

- Index refresh flights are keyed by source ID. Concurrent ordinary refreshes
  share the active result. A forced refresh arriving behind an ordinary pass
  queues one forced pass; callers that request the same queued force share it.
  A failed incremental pass does not cancel the queued force.
- Publish a source index only after the replacement transaction succeeds.
  Reusing a prior unchanged subtree must use a parent/path index; repeatedly
  filtering the full previous-entry list for every subtree grows with both the
  number of directories and indexed entries.
- Local filesystem freshness is independent of remote sync revision. A remote
  no-op may skip transport work, but it does not prove that local files stayed
  unchanged. Scan local sources before scraping, and refresh again after sync
  when applied changes are reported or the sync effect is unknown.
- Only durable library mutations raise the automation dirty generation. If a
  generation changes while a cycle is running, recheck it in `finally` and
  schedule another cycle after the active one; do not discard it or react to
  the coordinator's own broad UI notifications.
- A failed or overlapping sync has `changesKnown == false`. Reload and refresh
  conservatively in that case. A typed successful no-op may avoid a full store
  reload, but it cannot suppress the local filesystem scan.
- Batch tagging deduplicates by normalized path **and entry type**, retains
  file/directory name and parent-path information, and works in chunks of at
  most 100 targets. Index entries and metadata/tag links are persisted before
  the in-memory projection is updated. Each chunk failure is counted and does
  not make a failed chunk appear successful. Tag listeners receive at most one
  merged notification for the operation.
- Serialize batch-tag operations. A batch write must not overlap a full
  snapshot save or repository reload; queued `load()` calls wait for the active
  batch, and a batch waits for already active loads and saves before writing.
- Foreground leases cover the user action, not a UI polling timer. Routine
  maintenance checks at source/proposal boundaries and cleanup checks between
  bounded groups of database/cache work. After five minutes of continuous
  foreground activity, one safe maintenance slice may proceed; the next slice
  checks again. Never defer a transaction halfway through or drop a user task.
- `kProfileMode` logs expose phase names, elapsed milliseconds, and aggregate
  counts only. They never include source names, paths, comic titles, cookies,
  or tokens. An FFI phase measures bridge wait plus Rust work; it does not
  distinguish SQLite mutex wait from transaction duration.
- Cloud QR, Cookie validation, and connect actions set a visible busy label
  immediately, disable duplicate submission, and clear only the matching
  request ID on completion/cancel/error. Do not show a fabricated percentage.
- EH Daily and 115 automation scheduling, process deduplication, and persisted
  state remain separate from foreground maintenance coordination.

## 4. Validation and Error Matrix

| Condition | Required behavior |
|---|---|
| Normal refresh overlaps normal refresh for one source | Both callers receive the same active result |
| Force arrives during normal refresh | One forced refresh runs after the normal pass, even if that pass fails |
| Sync reports unknown effects | Reload and refresh conservatively before scraping |
| Sync reports no applied entities | Do not infer that local paths are unchanged |
| One batch chunk fails | Count that chunk as failed; keep committed chunks and report partial completion |
| Full save or reload is already active | Batch waits before acquiring the write gate |
| Foreground work is active | Maintenance waits until idle or its five-minute aging point, then serves one bounded slice |
| A newer busy request owns the UI | Completion of an older request cannot clear the newer loading state |
| QR start/exchange times out or is cancelled | Close the spinner state and preserve the actionable error/retry flow |

## 5. Good / Base / Bad Cases

- Good: a normal local refresh is in progress when the user requests a forced
  rebuild; the caller waits for the forced pass and receives its final result.
- Base: a timer cycle finds no remote changes; it still checks local source
  snapshots once before scraping and skips an unnecessary second scan.
- Good: a 250-target batch persists three bounded chunks, emits phase progress,
  and reports any failed chunk in the final summary.
- Base: a cloud QR start takes a few seconds; the add-source action shows a
  spinner and stage label while the request is pending.
- Bad: infer local freshness from an unchanged remote revision, let `load()`
  replace repository maps while a batch is applying chunks, or keep deleting
  stale rows through a long unbroken cleanup loop while foreground work waits.
- Bad: log a Cookie, title, complete path, or source account as part of a
  performance marker.

## 6. Tests Required

- Index tests: same-source ordinary single-flight; force behind ordinary;
  forced pass after an incremental error; no partial index publication.
- Automation tests: durable dirty mutation during an active cycle is replayed
  once; coordinator notifications do not create a loop; unknown sync effects
  trigger conservative reload/refresh; remote no-op still permits local scan.
- Batch tests: file/directory identity and parent paths survive; 100/101-target
  boundaries are correct; metadata and links persist together; partial failure
  counts are accurate; concurrent save/load/batch operations serialize.
- Coordination tests: foreground release wakes maintenance; aging grants one
  slice; further slices recheck; explicit user operations do not get dropped.
- UI tests: loading appears immediately and clears on success, cancel, timeout,
  or failure; duplicate taps are ignored; a stale request ID cannot clear a
  newer request.
- Profile review: record build/raster frame p95 and frame-budget misses with
  DevTools. Add dedicated SQLite lock-wait/transaction instrumentation only
  when those measurements justify it.

## 7. Wrong vs Correct

### Wrong

```dart
if (remoteRevisionDidNotChange) return; // local files may have changed.
for (final entry in everyStaleEntry) {
  await deleteStaleRow(entry); // no foreground boundary for a large cleanup.
}
```

### Correct

```dart
await refreshLocalSnapshots(); // independent of the remote revision result.
for (var start = 0; start < staleEntries.length; start += 50) {
  final end = (start + 50).clamp(0, staleEntries.length).toInt();
  await deleteChunk(staleEntries.sublist(start, end));
  await waitForMaintenanceTurn();
}
```
