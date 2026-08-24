# M8 Local-First Boundary Audit — 2026-08-20

## Decision

`Scraping Never Reads Remote Book Sources` is a P0 M8 architecture invariant. Scrape jobs may call AniList/Bangumi metadata APIs, but may never initiate WebDAV, SFTP, 115, Quark, Baidu or any other remote book-source I/O.

## Repository Evidence

- `app/rust/src/source/mod.rs:65-148` defines `ByteSource` and `SourceReader`; format parsers receive a source abstraction that can hide its locality.
- `app/rust/src/source/webdav.rs:658-696` documents and implements `WebDavFile::read_at` as HTTP Range unless a local cache file is present.
- `app/rust/src/source/sftp.rs:261-280`, `cloud115.rs:648-689`, `quark.rs:509-545`, and `baidu.rs:551-587` expose analogous remote `ByteSource` implementations.
- `app/rust/src/api/sync.rs:61-79` exposes explicit sync transport through `sync_now`; confirmation must not call it inline.

## Required Boundary

```text
Remote Book Source ─X→ M8 Scraper
Local SQLite ─────────→ catalog evidence
Local file/raw cache ─→ byte-level evidence
AniList/Bangumi ─────→ metadata candidate API only
```

Scraper input is `ScrapeAsset { catalog_snapshot, local_content: Option<LocalContentHandle> }`, not a generic `ByteSource`. `RemoteOnly` assets lack local content and can only produce catalog-derived evidence.

## Test Invariants

1. A RemoteOnly asset produces zero remote book-source requests, `ByteSource::read_at` calls, remote stat/HEAD/PROPFIND calls and comic downloads during `start_scrape`.
2. A FullyCached remote asset reads only the local raw cache during evidence extraction.
3. `confirm_proposal` writes canonical SQLite data and a sync-dirty marker, but invokes no sync transport inline.
