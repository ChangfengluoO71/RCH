# Implementation plan

## Order and contracts

1. Add pure Flutter models for split regions, reading-order sequence mapping, center-gutter detection, and split render-width calculation. Add failing Dart tests first for RTL/LTR ordering, mixed pages, cover/boundary behavior, seam acceptance/rejection, rotation, and width policy; then implement until green.
2. Add local-only SQLite override storage and FRB APIs. Test CRUD, missing-row fallback, and the local-only table boundary. Regenerate FRB bindings after Rust API changes; do not hand-edit generated bindings.
3. Convert Rust page reads to request-scoped widths and width-keyed memory/disk/in-flight/prefetch state. Add failing concurrency/cache tests before changing implementation. Keep the `None` cache path and `w<width>` layout compatible. Add PDF page dimensions under the Pdfium gate and tests for geometry and multi-page request behavior.
4. Integrate preview analysis, dynamic virtual-page mapping, region rendering, source-page progress/resume/completion, per-page correction, and global/session settings into Flutter. Add widget tests for modes, labels, overrides, forced dual-page exclusions, and completion. Preserve image rotation, AI page selection, random navigation, and load generation guards.
5. Run targeted tests after each change, then full `flutter analyze`, `flutter test`, Rust `cargo check --locked --all-targets` with warnings denied, and serial Rust `cargo test --locked -- --test-threads=1`. Review the complete diff and report any environment-only failures.

## Cross-layer invariants

- Source files and archive members are never rewritten; split regions exist only in reader presentation.
- A `DisplayPage` is the only unit of paged visual navigation; history, bookmarks, and remote APIs continue to use physical source indices.
- No whole-book pre-scan is introduced. Only requested/neighbor pages are previewed.
- Local page overrides do not enter `BookMeta`, `app_settings`, sync snapshots, or sync DTOs.
- Every width-sensitive cache and in-flight lookup uses the same request-scoped width identity.
- `DualPageMode.force` retains its existing page semantics. Webtoon keeps source-page navigation and stacks split halves vertically inside each source-page item.
