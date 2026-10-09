# Design: Mobile wide-page split reading

## Boundaries and data flow

The reader remains the owner of visual navigation. A new typed `DisplayPage` identifies a source page plus its whole/left/right region and reading-order part number. `ReaderPaging` maps physical page indices to this display sequence using only currently resolved split decisions; unresolved pages count as whole. When a page is loaded, a bounded preview resolves it and the mapping is updated while preserving the current source page and part. Only visible/prefetched neighbors are inspected, so remote sources remain lazy.

The `WidePageMode` app setting is `smart` by default, with `split` and `whole` alternatives. Smart mode splits in portrait only, for effective aspect ratio >= 1.3 and a confident center seam. Seam analysis uses a <=512px preview, searches x=45%-55%, and accepts a 1%-4%-wide low-texture band spanning >=85% of rows whose edge density is <= half the adjacent bands. Uncertain pages remain whole. Forced and per-page splits use the best seam or x=50%. Wide-page decisions are suppressed only by forced dual-page stitching. Manga orders right then left; comic orders left then right. In webtoon mode, the halves are stacked vertically in the page's existing list item, using the reading direction active when entering webtoon mode; progress remains tied to the source page.

Per-page overrides are stored in a local-only SQLite table keyed by canonical book key and source page. They are exposed through the Store/API boundary and excluded from sync DTOs and snapshot/app-settings payloads. Progress continues to store source page indices; the displayed label shows the part. Opening a saved physical page starts at its first reading-order part. Completion observes the final visual part, not merely the final source index.

## Rendering and compatibility

Flutter decodes raster source images to the selected single-page target width, doubled for a split spread. PDFs expose optional page dimensions so the reader can request a 512px preview only for wide candidates, then render the final page at the appropriate width. Existing render-width presets remain; standard keeps its legacy disk-cache directory when width is omitted. Width becomes request-scoped throughout Rust `Reader`; memory, disk, in-flight, and prefetch keys all include `(page index, optional target width)`. Existing `w<width>` disk paths remain readable.

The image is clipped to the selected source rectangle in the reader viewport; the original archive/source bytes remain unchanged. Rotation is applied consistently to detection and crop mapping. Automatic border cropping is explicitly deferred.

## Main interfaces

- Flutter: `WidePageMode`, `DisplayPage`, split override type, `WidePageDetector`, and extended `ReaderPaging`.
- Rust document: optional `page_dimensions(index)`; PDF returns dimensions under the existing Pdfium synchronization gate.
- Rust reader/API: page reads accept request-scoped target width; page-dimension query; local-only per-page split override CRUD.
- Store: typed override load/update methods and canonical local book key construction.

## Risks and mitigations

- A seam detector can confuse an intentional spread with two scans. Smart mode therefore requires a continuous gutter signal and leaves uncertain pages whole; user overrides correct individual pages.
- Resolving a split changes virtual indices. Rebuild/reposition by stable `DisplayPage`, not by stale integer index.
- PDF preview/final rendering can race or reuse the wrong cache entry. Include target width in every cache and in-flight identity, and test concurrent requests.
- Large double-width images can increase memory use. Bound the requested source width at 8192px, decode lazily for the active viewport, and retain the existing image cache lifecycle.

## Verification

Run focused Dart model/widget tests, then full `flutter analyze` and `flutter test`; run Rust reader/document/db tests, `RUSTFLAGS="-D warnings" cargo check --locked --all-targets`, and serial `cargo test --locked -- --test-threads=1` from `app/rust`.
