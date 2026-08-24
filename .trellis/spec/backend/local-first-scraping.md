# Local-First Scraping Boundary

> M8-M1 baseline is Catalog-Only: filename, persisted ancestor directory names and already-indexed sibling names are the complete input. Content evidence is a later capability.

## Scenario: M8 must never read remote book sources

### 1. Scope / Trigger

- Trigger: M8 introduces cross-layer scraping, metadata extraction, Provider HTTP, cache, sync-dirty and FRB APIs.
- Existing `ByteSource` deliberately hides local and remote Range I/O; passing it to scraper code could silently contact WebDAV, SFTP, 115, Quark, Baidu or a future book source.
- This spec applies to every M8 scraper, extractor, fingerprint, provider and confirmation implementation.
- It also applies when scraping is launched automatically by the M8 coordinator after startup, a catalog revision, a sync completion or a periodic tick; automation does not grant extra source I/O permissions.

### 2. Signatures

```rust
enum ContentAvailability { LocalFile, FullyCached, RemoteOnly }

struct CatalogSnapshot {
    book_key: String,
    filename: String,
    ancestor_dirs: Vec<String>,
    parent_siblings: Vec<String>,
    user_title: Option<String>,
    user_author: Option<String>,
}

struct ScrapeAsset {
    catalog_snapshot: CatalogSnapshot,
    local_content: Option<LocalContentHandle>, // later content-evidence stages only
}

fn start_scrape(asset: ScrapeAsset) -> Result<ScrapeJob, ScrapeError>;
fn confirm_proposal(request: ConfirmProposalRequest) -> Result<ConfirmResult, ConfirmError>;
```

- `LocalContentHandle` opens only a local filesystem file or a completed `raw/` cache entry.
- M8-M1 scraper APIs accept only `CatalogSnapshot` and must not accept `ByteSource`, file handles, `BookSource`, remote source sessions, Downloader handles, remote URLs, or sync transport handles.
- Later content extractors may accept `ScrapeAsset`, but `local_content` must be absent for `RemoteOnly`.
- `confirm_proposal` returns after local commit; it may mark records sync-dirty but must not invoke sync transport.
- A `sync_transport` job may run before or after a scrape job according to coordinator policy, but a `catalog_scrape` job never receives the sync job's transport/session capability.

### 3. Contracts

| Availability | Allowed evidence | Prohibited |
|---|---|---|
| `LocalFile` | filename, embedded metadata, page count, selected hashes, lazy SHA-256 | — |
| `FullyCached` | same, reading only the local raw cache | any source fallback |
| `RemoteOnly` | persisted SQLite filename/path/size/mtime/etag and user metadata | `ByteSource`, Range, download, cover, embedded metadata, page hash, stat/HEAD/PROPFIND, refresh |

- Tier 0 `source_fingerprint + normalized_path` and Tier 1 `filename + persisted size/mtime/etag` read only SQLite.
- Tier 2 content evidence and Tier 3 SHA-256 require `LocalFile` or `FullyCached`.
- Provider calls may use only normalized local text evidence and query fields. `ProviderError::Offline`, `Timeout`, `RateLimited`, `Unauthorized`, `InvalidResponse`, and `Unavailable` must preserve local usability; jobs may end `local_evidence_only` or `provider_unavailable`.

### 4. Validation & Error Matrix

| Condition | Required behavior |
|---|---|
| `RemoteOnly` content extractor requested | skip with `content_unavailable`; no network fallback |
| catalog field missing | leave evidence absent; no remote stat/list request |
| Provider unavailable | preserve local evidence and return typed degraded state |
| proposal stale | reject with conflict; no canonical or transport write |
| confirmation succeeds | atomically write canonical data + sync-dirty only |
| sync endpoint unavailable during confirmation | confirmation still succeeds locally |

### 5. Good / Base / Bad Cases

- Good: a WebDAV entry without raw cache creates a query from persisted filename and user title, then calls AniList; book-source request count remains zero.
- Base: a cached SFTP CBZ extracts ComicInfo from `raw/` and never reopens SFTP.
- Bad: an extractor accepts `Arc<dyn ByteSource>` and reads the ZIP central directory; this can issue a remote Range request and is forbidden.

### 6. Tests Required

- RemoteOnly integration test: assert remote book-source requests, `ByteSource::read_at`, remote HEAD/stat/PROPFIND and comic downloads are all zero after `start_scrape`.
- FullyCached integration test: assert evidence reads local raw cache and remote request count is zero.
- Provider failure test: assert `local_evidence_only` / `provider_unavailable` without book-source fallback.
- Confirmation transport-spy test: assert canonical transaction and sync-dirty marker complete while `sync_now`, sync actor and remote transport call counts remain zero.

### 7. Wrong vs Correct

#### Wrong

```rust
fn extract(source: Arc<dyn ByteSource>) -> Evidence {
    parse_zip(source) // WebDavFile::read_at may perform an HTTP Range request.
}
```

#### Correct

```rust
fn extract(asset: &ScrapeAsset) -> Result<Vec<MetadataEvidence>, ScrapeError> {
    let Some(local) = &asset.local_content else {
        return Ok(vec![MetadataEvidence::content_unavailable()]);
    };
    parse_local_content(local)
}
```
