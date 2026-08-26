# Android 远程 PDF 闪退 — Root Cause Investigation

Date: 2026-08-26

## Current status

Task entered `in_progress`. No product-code fix has been committed.

## Observed application data flow

1. Flutter `ReaderPage._open()` opens remote books through `openWebdavBook` / `openSftpBook` / `openBaiduBook` / `openCloud115BookFor` / `openQuarkBook`.
2. Remote open resolves to `document::open_document(...)` and therefore the same `PdfBook` implementation used by local PDFs.
3. `PdfBook::open()` reads the entire `ByteSource` into one `Vec<u8>` and loads that buffer into Pdfium.
4. `register_book()` wraps the PDF in the generic `Reader` and immediately calls `reader.warm_up()`.
5. Generic `Reader::spawn_prefetch()` launches independent OS threads for neighbor pages. Flutter then also requests the current page and next two pages through `_ensure()`.
6. For a cold multi-page PDF, several `PdfBook::page_bytes()` calls can therefore overlap across threads against the same `PdfDocument`.

## Historical validation gap

Android PDF support was accepted on 2026-08-08 using a one-page `dummy.pdf`. A one-page document does not exercise neighbor-page prefetch, so that acceptance did not cover concurrent rendering of a multi-page PDF.

## H1 — primary root-cause hypothesis

`pdfium-render 0.9.3` is unsound under concurrent use despite its default `thread_safe` feature. RCH uses exactly 0.9.3 with default features enabled. Upstream issue #262 demonstrates that 0.9.3/default-feature safe Rust can concurrently enter non-thread-safe Pdfium state and crash with SIGSEGV/SIGTRAP because the feature provides `Send + Sync` without actual FFI serialization. Upstream 0.9.4 release notes state that memory safety under `thread_safe` was improved.

Why this maps to RCH:

- RCH's `Document` contract requires `Send + Sync` and explicitly permits concurrent `page_bytes()` calls.
- `Reader` actively spawns multiple threads around the current page.
- `PdfBook` stores one shared `PdfDocument` and performs rendering inside `page_bytes()`.
- The original Android acceptance used only one page, so it could not expose this race.
- A real remote multi-page PDF on a cold cache naturally exercises several page renders immediately.

Prediction if H1 is correct:

- Crash evidence should be native (SIGSEGV/SIGTRAP/abort) with frames in `libpdfium.so` / `librust_lib_app.so`, rather than a handled Dart/Rust `Result` error.
- Multi-page cold-cache PDFs should reproduce more readily than a one-page PDF.
- Reopening after page cache has already been populated may reproduce less readily because fewer Pdfium renders are required.
- Local multi-page cold-cache PDFs may also be vulnerable even if the issue was first observed on remote files.

## H2 — secondary hypothesis to exclude

Android memory pressure caused by `PdfBook::open()` reading the entire PDF into a `Vec<u8>` before loading it into Pdfium. This can be amplified for large remote PDFs, especially if the remote path holds additional buffers during download/range access.

Prediction if H2 is correct:

- `logcat` should show LMKD / OOM / allocation failure rather than a Pdfium data-race-style native fault.
- Reproduction should correlate strongly with PDF byte size rather than page concurrency.

## Evidence required before fixing

Capture one failing Android run with:

- source type (WebDAV / SFTP / Baidu / 115 / Quark),
- open strategy (`auto`, `download`, or `stream`),
- PDF size and page count,
- device model / Android version,
- `adb logcat` covering app launch through crash,
- native tombstone/backtrace if emitted.

Then compare with at least one control:

- same PDF opened locally or after full download, OR
- a one-page PDF through the same remote source, OR
- the same multi-page PDF on a warm page cache.

## Decision gate

Do not change `pdfium-render`, disable prefetch, or add a mutex until crash evidence discriminates H1 from H2. If native backtrace is consistent with H1, proceed to a failing concurrency regression test and evaluate the smallest correct remediation. If evidence is OOM/LMKD, return to the ByteSource/PDF-buffer lifetime path instead.
