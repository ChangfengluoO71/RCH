# M8-M1 — Catalog-Only Name & Role Extraction — Technical Design

## Input and Output

```rust
struct CatalogSnapshot {
    book_key: String,
    filename: String,
    ancestor_dirs: Vec<String>, // nearest first; SQLite snapshot only
    user_title: Option<String>,
    user_author: Option<String>,
}

struct NameRoleProposal {
    title: Option<RoleValue>,
    authors: Vec<RoleValue>,
    provider: Option<RoleValue>,
    volume: Option<RoleValue>,
    chapter: Option<RoleValue>,
    evidence: Vec<RoleEvidence>,
    conflicts: Vec<RoleConflict>,
    state: ParseState,
    rule_version: String,
}
```

The parser accepts plain catalog data, never a `ByteSource`. `ancestor_dirs` is bounded by a configurable depth (default 3) and missing levels are ordinary input, not a reason to enumerate a source.

M8-A0 invokes this parser from the `catalog_scrape` lane after startup, a persisted catalog revision, or sync completion. The parser itself remains scheduler-agnostic: it returns a deterministic proposal for the captured snapshot and never calls the sync lane or a source adapter.

## Rule Pipeline

1. Normalize separators, whitespace, brackets and comparison case while retaining original spans.
2. Split filename stem and ancestors into tokens.
3. Detect volume/chapter/edition markers (`Vol`, `Ch`, `第…卷`, `第…话`, `OVA`, `特典`, numeric markers) first and remove them from title candidates.
4. Apply explicit provider/platform markers and a versioned provider lexicon (`汉化组`, `出版社`, `raw`, `scan`, known platform/source labels) before author inference.
5. Apply explicit author markers (`作者`, `原作`, `作`, `画`, `著`, `by`, `author`) and person-name patterns. Provider candidates receive negative author weight; explicit author markers can create a visible conflict rather than silently reclassifying.
6. Compare filename and nearest non-provider ancestors. Repeated/stable work-like tokens become title evidence; deeper ancestors are treated as collection/category context unless explicit markers say otherwise.
7. Emit optional fields and role conflicts. Never fabricate missing author/title/provider.

## Persistence and Safety

- Save proposals under a local working-state key `(book_key, rule_version)` or return them through a stable Rust API; do not write `works`, `work_links`, `book_metas`, or sync payloads.
- No call path may import `ByteSource`, Downloader, remote source sessions, Provider clients or sync transport.
- The first version is useful without network access and without a raw cache.
- Automatic execution is proposal-producing only; canonical confirmation and the later sync-dirty transport remain outside this parser module.
