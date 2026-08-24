# M8-M1 — Catalog-Only Name & Role Extraction — Implementation Plan

## Red-Green-Refactor

1. Add failing parser fixtures for title/chapter separation, author/provider separation, ancestor relation, missing fields and RemoteOnly catalog input.
2. Implement token normalization and structural marker extraction.
3. Implement provider/platform and author rules with mutually exclusive role validation.
4. Implement title/ancestor scoring and `NameRoleProposal` evidence/conflict output.
5. Add stable rerun and zero-network boundary tests; then refactor only while tests remain green.

## Files / Boundaries

- Add the M8 catalog parser under the Rust metadata/scraper module selected by the existing directory conventions.
- Keep parser inputs as plain catalog DTOs; do not import `source::ByteSource` or Downloader.
- Do not touch Flutter, canonical DDL, Provider adapters or sync transport in this child.
- Expose a narrow adapter for M8-A0 that accepts a captured `CatalogSnapshot` and returns a working proposal; the adapter must not accept scheduler network capabilities.

## Required Tests

- filename-only and three-level ancestor fixtures;
- bracketed provider/group plus explicit author markers;
- title containing numeric tokens without misclassifying chapter/volume;
- absent author/title and ambiguous role output;
- LocalFile/FullyCached/RemoteOnly equivalent catalog snapshots;
- deterministic output for the same rule version;
- coordinator-triggered startup/catalog-revision/sync-completion jobs are deduplicated by input revision;
- instrumentation proving no book-source request, Range read, stat/HEAD/PROPFIND or download.

## Validation

```powershell
cd app\rust
cargo test scraper::
cargo test
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
```

M8-M1 is complete only when the parser can run as a standalone local catalog capability and its outputs are explainable even when author/title are absent.
