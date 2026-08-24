# M8-M3 — Optional Provider Enrichment — Technical Design

Provider runtime consumes a normalized `title` plus optional `authors` from M8-M1 and returns candidates. It has no dependency on source adapters, `ByteSource`, Downloader, catalog refresh, or sync transport. A missing/failed Provider result leaves the local proposal as the primary result and records a typed enrichment state.
