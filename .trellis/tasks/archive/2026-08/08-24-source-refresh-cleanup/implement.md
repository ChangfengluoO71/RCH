# Implementation plan

1. Add the task-specific source/root and cleanup regression tests first; run
   them to capture the current failures.
2. Implement `BookSource.effectiveRootPath`, update the 115 edit path and use
   the helper in browser and remote crawler entry points.
3. Make source add/update persistence awaitable and refresh
   `LibraryCatalogStore` only after the save completes; update UI source add,
   edit and import flows to await the boundary.
4. Add orphan-tag pruning to `TagRepository` and switch stale-data cleanup to
   it. Reload the source tree after cleanup, including the no-removed-key path.
5. Harden remote alignment result handling so failed refreshes cannot authorize
   deletion.
6. Run focused tests, then full Flutter analysis/tests and relevant Rust tests.
   Review the diff for local-first violations and concurrent mutation hazards.
7. Update the backend/frontend specs if the new source-persistence or cleanup
   contract is durable knowledge, then prepare a single fix commit.

## Validation commands

```text
flutter test --no-pub
flutter analyze --no-pub
cargo test --manifest-path app/rust/Cargo.toml
```

## Risk points

- Awaiting a debounced save must not deadlock the existing save queue.
- Ignored futures in credential-refresh call sites must not become unhandled
  errors; use the existing error/logging conventions.
- 115 root IDs and logical `path` values must stay compatible with old DB rows.
- Tag pruning must not delete a shared or manually retained tag.
