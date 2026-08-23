# M8-A0 Automation Coordinator & Sync Integration

## Implementation checkpoint

Implemented the first usable vertical slice and paused for manual sample validation.

- Catalog-only parser reads filename plus persisted ancestor/sibling names; it never
  receives a `ByteSource`, downloader or source session.
- `catalog-rules-v3` separates work title, title aliases, circle/artist, provider/
  release group, source series, event, edition, chapter/issue/volume relations,
  external IDs and resource-level tags with evidence.
- Chapter-only files use parent folders as work context; continuation/side-story/
  front/following parts receive stable relation and sort projections.
- SQLite now persists `scrape_jobs` and `scrape_proposals`.
- The full semantic proposal is persisted as versioned `semantic_json`, while the
  compatibility columns remain available to older callers.
- FRB API exposes pure parsing, batch scraping, proposal listing, and job listing.
- `AutomationCoordinator` is the single owner of startup, local-change debounce, and periodic triggers.
- SyncEngine keeps WebDAV transport, cooldown, and retry behavior but can run without its own timers in coordinator mode.
- The automatic cycle is sync/revision check -> catalog-only scrape; no inline canonical materialization.
- ScrapePanel exposes result states and the filename/path evidence needed for human review.

## Verification completed

- Rust `cargo test --lib`: 171 tests, 169 passed, 2 ignored.
- Golden corpus: 33/33 semantic projection fixtures passed.
- Flutter `flutter analyze --no-pub`: no issues found.
- Flutter `flutter test --no-pub`: all tests passed.

## Current stop condition

Do not add provider calls, canonical writes, or more parsing heuristics until real local and RemoteOnly catalog samples are reviewed. Manual validation should check:

1. title vs chapter separation, including `10续` and chapter-only folders;
2. circle/artist/author vs provider/release-group separation;
3. ancestor depth and nested folder semantics;
4. missing-author, title-alias and ambiguous cases;
5. resource completeness/edition/source tags are not promoted to work state;
6. RemoteOnly entries produce proposals without source I/O.
