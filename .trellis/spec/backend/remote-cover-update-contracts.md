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
