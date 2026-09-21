# Remote Cover, Reader Budget, and Update Handoff Contracts

## 1. Scope / Trigger

This contract applies to remote poster covers, custom-cover editing, reader
page reads, and application update download/install handoff. It was added after
the post-release implementation introduced typed Rust cover outcomes, a shared
blocking-request governor, and injectable installer boundaries. It prevents a
cache miss or a provider limitation from silently turning a visible-card or
custom-cover request into a whole-book download, and prevents concurrent UI
surfaces from duplicating I/O or installer launches.

## 2. Signatures

Rust cover APIs exposed through FRB:

```rust
enum CoverFetchPolicy { CacheOnly, RemotePartialOnly }
enum CoverFetchOutcome { Image, CacheMiss, RangeUnsupported }
struct CoverFetchResult { outcome: CoverFetchOutcome, image: Option<PageImage> }
```

The six provider `*_cover_with_policy` functions accept the existing session,
path, page, dimensions, crop, and `CoverFetchPolicy`. Legacy `*_cover` methods
remain compatibility wrappers and must not weaken the policy.

Rust reader budget:

```rust
blocking_request_governor().acquire(RequestPriority::Foreground | Prefetch | Cover)
```

The permit selects only not-yet-started blocking work. A held permit must not
be documented as cancellation of synchronous provider I/O.

Dart boundaries:

```dart
AppSettings.remoteCoverFetchEnabled // persisted default: true
Future<void> UpdateManager.download();
Future<void> UpdateManager.confirmDialogDownload();
Future<void> UpdateManager.install();
```

The update manager accepts injectable `UpdatePlatform` and
`UpdateDownloadTransport` implementations for tests. `CoverEditorPage` uses
typed `CustomCoverOpenDecision`, `CustomCoverReady`, and
`NeedsWholeBookDownload` decisions.

## 3. Contracts (request / response / environment)

- `CacheOnly` may decode an existing memory, cover-disk, or raw-local cache
  through an already-live provider session. A missing session is a typed cache
  miss; it must not create a session, resolve metadata, request a downlink,
  probe Range, or read a remote body.
- `RemotePartialOnly` may perform a new provider request only after the adapter
  verifies an exact HTTP `206` and a matching `Content-Range` interval. SFTP
  uses its bounded random-access path. Unsupported Range returns
  `RangeUnsupported`; it never calls whole-book raw-cache download.
- Poster cards schedule only while mounted/visible, coalesce equivalent logical
  keys, and release queued work when the final subscriber disposes. Started
  blocking work is allowed to finish.
- Reader request order is `Foreground > Prefetch > Cover`; background work is
  capped so at least one capacity opportunity remains for foreground work.
- Custom-cover editing follows cache/local → safe partial route → typed
  `NeedsWholeBookDownload` → explicit `下载整本` / `取消`. Only confirmation
  may call the explicit download strategy.
- Update downloads publish one in-flight Future before I/O. Dialog confirmation
  is the only source of automatic installer handoff; settings downloads leave a
  verified package for the manual install action. Windows launches an
  executable with a fixed argument list; Android opens the system confirmation
  channel only.

## 4. Validation & Error Matrix

| Condition | Required result |
| --- | --- |
| Cache-only request without live session | Typed `CacheMiss`, no remote/session factory call |
| HTTP `200`, `416`, missing/malformed/mismatched `Content-Range` | Do not consume body as a cover; return `RangeUnsupported` for policy cover path |
| Valid exact `206` partial response | Decode and return `Image` |
| Remote auth/network failure | Placeholder/retryable error; no credentials, downlinks, or full URL in UI |
| Last queued cover subscriber disposes | Remove not-yet-started task; do not claim active cancellation |
| Foreground/read/prefetch/cover contention | Start foreground first; preserve per-priority FIFO and foreground reserve |
| Custom-cover user cancels | Zero whole-book download calls; retain retryable state |
| Custom-cover user confirms | Exactly one explicit download-capable call after typed need decision |
| Update transfer duplicate | Share one Future and one target writer |
| Installer/channel/UAC handoff fails | Preserve verified package and return to retryable downloaded state |

## 5. Good / Base / Bad Cases

- Good: a visible WebDAV card with a live session receives a verified partial
  response and caches the decoded image.
- Good: two cards with the same source/path/page/size/crop share one scheduler
  request while either subscriber remains mounted.
- Base: a disabled setting or an unavailable Range leaves the existing
  placeholder and one source-level notice; opening custom cover asks before a
  whole-book download.
- Good: two simultaneous update buttons observe one manager Future; only the
  dialog-confirmed path launches the installer after size verification.
- Bad: calling `*SessionFor` while `CacheOnly` is selected.
- Bad: treating any `200` response or failed Range probe as a poster image or
  falling back to `download_to_raw_cache`.
- Bad: interpolating an installer path into PowerShell or showing the raw
  transport exception (which may contain a mirror URL/token).

## 6. Tests Required (with assertion points)

- Rust safe-cover tests assert exact `206`/`Content-Range` validation before
  body reads, typed outcomes, cache-only no-remote behavior, and no raw-cache
  fallback. Run `cargo test safe_cover_contract` and the serial full suite.
- Rust reader tests assert foreground ordering, the reserved foreground slot,
  same-page in-flight deduplication, cache-hit prefetch, and release of permits.
- Flutter cover tests assert logical-key dedupe, reference-counted disposal,
  disabled no-session factory calls, one notice per source, and placeholder
  behavior for `RangeUnsupported`.
- Flutter custom-cover tests assert zero download calls on cancel, one explicit
  call on confirmation, retry after cancellation, and no prompt for supported
  partial access.
- Flutter update tests assert shared download Future, dialog-backed progress,
  no automatic install for manual download, argument-list Windows launch,
  Android channel failure preservation, redacted error text, and retryable
  downloaded state.

## 7. Wrong vs Correct

### Wrong

```dart
// A poster miss silently creates a session and may download a whole book.
final session = await webdavSessionFor(source);
final cover = await webdavCover(session: session, path: path, ...);
```

### Correct

```dart
final result = await webdavCoverWithPolicy(
  session: liveSession,
  path: path,
  page: page,
  width: width,
  height: height,
  policy: remoteCoverFetchEnabled
      ? CoverFetchPolicy.remotePartialOnly
      : CoverFetchPolicy.cacheOnly,
);
switch (result.outcome) {
  case CoverFetchOutcome.image:
    showImage(result.image!);
  case CoverFetchOutcome.cacheMiss:
    showPlaceholder();
  case CoverFetchOutcome.rangeUnsupported:
    showPlaceholderAndSourceNotice();
}
```

For updates, the equivalent wrong pattern is building a shell command from a
path or calling `install()` from every download button. The correct pattern is
to call `confirmDialogDownload()` only after dialog confirmation and pass the
executable plus fixed arguments through `UpdatePlatform`.

## Superseding Addendum — Remote Cloud Scan Task 6 (2026-09-14)

This additive section records the finalized remote-scan invariants. Existing
cover/update guidance above remains historical context; this section takes
precedence where the remote-cloud-scan adapter, generation, tombstone, or
retention rules are more specific.

### Adapter and range invariants

- WebDAV, SFTP, Baidu, 115, and Quark adapters expose the same logical
  boundary: normalized paths, paged directory entries, optional `size` and
  `mtime`, bounded file reads, and typed capability/error outcomes. Provider
  credentials, authorization headers, cookies, tokens, and full private URLs
  never cross the test, status, or report boundary.
- A directory is `listing_complete` only after every page is read successfully
  and the pagination cursor is consumed without a cycle. A partial page,
  authentication failure, 403, 404, 429, cancellation, or transport error
  cannot replace the previous manifest or authorize deletion.
- Partial HTTP reads are accepted only for `206` responses whose
  `Content-Range` unit, start, end, total, and body length match the requested
  interval. `200`, `416`, missing, malformed, or mismatched `Content-Range`
  responses are the typed `rangeUnavailable` outcome. They must not be
  interpreted as a cover and must not silently trigger a whole-book download.
  `Accept-Ranges` is an optimization hint, not proof of capability.
- Direct image children classify a directory as `ImageFolder`; archive
  children classify the parent as `ContainerDir`. Hidden/system entries are
  excluded before fingerprinting and classification. Missing `mtime` remains
  unknown metadata and is not converted to zero.

### Generation and tombstone invariants

- Every staged generation is bound to the source fingerprint, effective root,
  and non-empty session epoch. Listing rows, cover tasks, and status writes
  must carry the same generation/epoch; an older session cannot publish or
  finish work after a newer session is bound.
- New listings and cover dependencies are staged first and published in one
  transaction. A partial, failed, paused, cancelled, or stale generation is
  discarded while the last successful generation remains visible.
- A remote tombstone requires a complete listing for the exact parent, the
  current generation and session proof, and a succeeded terminal state. Only
  missing direct children of that proven listing are tombstoned. One-file 404,
  incomplete pagination, auth expiry, 403, 429, cancellation, or a source/root
  mismatch is never deletion evidence.

### Cover-retention invariants

- Reader completion purges page/raw content only. Covers, cover aliases,
  metadata, tags, reading history, completion state, and custom-cover page or
  crop parameters remain available for a live book.
- Verified remote deletion may remove the deleted logical cover, aliases,
  image-folder dependencies, and page/raw content selected by the dependency
  graph. It must not touch an unrelated source/path or a still-live book.
- Turning off `remoteCoverFetchEnabled` or the background scan gate stops new
  network work but preserves existing covers and index metadata. The UI shows a
  source-scoped paused/degraded state with a redacted message and does not
  expose provider secrets.

The provider-labelled fake matrix and the Flutter coordinator/status contract
tests are the executable boundary for these rules; release evidence remains
pending until real provider/device validation is recorded separately.

## Superseding Addendum — v0.6.0 Opening Speed & Cover Pipeline (2026-09-21)

### Cover document-open read budget

- The 64 MiB `COVER_READ_BUDGET_BYTES` cap must **not** reject a *deliberate
  whole-file read*: `offset == 0` and a single request for the entire file,
  bounded by `COVER_WHOLE_FILE_MAX_BYTES` (512 MiB, same order as the PDF cap).
  The read-count cap (384) and the wall-clock cap (30 s) still apply, so
  pathological archives (a tail EOCD sweep = many small reads) stay rejected.
  Predicate: `is_deliberate_whole_file(offset, requested, length)`.
- This is a **document read**, not a download strategy: the cover path must
  still never call the provider's whole-book raw-cache download. The original
  "a cover request never becomes a whole-book download" invariant is preserved.
- Rationale (real device, 2026-09-21): an 82.6 MB MOBI whose lazy open was
  declined fell back to the whole-file read; that single 82.6 MB request was
  rejected by the byte cap, so the cover failed forever
  (`cover_read_budget_exceeded`) even though the same book's cover needs one
  page. After the exemption the same book opens its cover in 719 ms and reads
  page 0 in 198 ms.

### MOBI lazy-open leniency

- A record whose offset is >= file length (a legal zero-length EOF/padding
  entry) and a candidate range that is empty or inverted must be **skipped**,
  never a reason to decline the whole lazy path. Declining sends the book into
  the whole-file fallback, which is exactly the failure mode above.
- Every decline path must log `mobi_lazy_declined reason=<code>` (too_small,
  header_read, record_count_zero, record_table_overflow, record_table_read,
  no_mobi_magic, first_image_index, probe_failed, no_decodable_image) plus
  `mobi_lazy_clamped_offsets` / `mobi_lazy_skipped_ranges` counters, so the
  reason is read from a log instead of guessed.

### MOBI page-table cache

- Key: `stable_hash(path|file_len)` under `cache/mobi_table/<hash>.table`.
  Self-validation: the file stores `sha256(PalmDB header ‖ full record table)`
  and is only reused when version, length, and digest all match (a replaced file
  with an identical length therefore invalidates it). Ranges are additionally
  checked for non-empty, monotonic, in-file intervals.
- Writes are atomic (`.tmp` then rename) and best-effort. The cover-only entry
  point must **not** write the table: it probes only until the first image and
  would persist a truncated page list.

### Card wake and local-fallback invariants

- Every remote card subscribes to its source's cover revision, **including
  cards without a stable asset id** (container-folder comics). A wake re-reads
  durable state only; it never re-issues `requestCover` on its own.
- If the unified cache holds no bytes for the requested selection/profile, the
  card must fall back to the **legacy pure-local cover cache** before rendering
  a failure placeholder. That legacy read is local-only by contract: no session,
  no provider request, no job, no wake, no durable write.
- Rationale: the detail page resolves covers through the legacy path while wall
  cards use the unified path; the two caches are independent, so a card that
  only consults the unified side can show "获取失败" for a book whose bytes are
  on disk and visible on the detail page.

### Manual cover retry

- `remote_cover_retry_failed(source_id, limit)` requeues terminal `failed`
  rows of that source's **current profile only**, resets attempt/backoff/long
  retry, bumps the source revision, and never purges caches or touches other
  profiles, states, or sources. `limit` is clamped to 1..500.
- A wake requires a live session binding (`wake_cover_worker_for_source`
  returns silently without one and must never fabricate a session), therefore
  callers must invoke the retry **after** the source session exists (Dart:
  after `_relist()`), otherwise rows sit in `pending` and nothing fetches them.

### Failure-code accuracy

- `cover_native_lib_missing` may only be produced from the shared loader marker
  `PDFIUM_LOAD_FAILURE_MARKER`. Any other pdfium message — pdfium-render emits
  "pdfium" in every library error — maps to `cover_document_open_failed`
  (terminal, still manually retryable). Mislabeling library errors as a
  deployment problem previously sent investigation after a missing DLL while
  the same process rendered PDF pages normally.

### Cache governance

- `cache/raw` (whole-book packages) has a 2 GiB cap: over the cap, whole
  packages are evicted oldest-first by the newest mtime inside the package, and
  the newest package is always kept. Enforcement is best-effort and runs at the
  single open choke point (`register_book`). The cover whole-file read above is
  in-memory and therefore leaves no package to clean.
- The page cache is partitioned by render width: width 0 keeps the historical
  `page/<ns>/<index>.bin` layout, any explicit width uses
  `page/<ns>/w<width>/<index>.bin`, so switching width neither reuses the wrong
  size nor invalidates the standard-tier cache.

### Reader request-merge invariant (G1)

- The file-head window, once fetched naturally (a window fetched at offset 0),
  is **pinned** and never evicted; serving a head read from it must not issue a
  new remote request. Metadata window slots are 4. Both changes only retain
  already-fetched data — they must never increase request count or bytes.

Executable boundary for this addendum: `document::mobi::tests::page_table_cache_*`,
`lazy_open_tolerates_zero_length_and_out_of_range_records`,
`full_open_probes_concurrently_while_cover_open_stays_serial`,
`source_reader_head_pin_contract`, `adapter_byte_source_stops_at_the_read_budget`,
`deliberate_whole_file_read_bypasses_byte_budget_up_to_the_absolute_cap`,
`archive_open_failures_separate_missing_native_lib_from_document_errors`,
`requeue_failed_for_source_only_touches_current_profile_failures`,
`raw_cache_limit_evicts_oldest_packages_and_keeps_the_newest`,
`display_width_is_forwarded_and_partitions_the_page_cache`, and the Dart
`comic_cover_legacy_fallback_test`.
