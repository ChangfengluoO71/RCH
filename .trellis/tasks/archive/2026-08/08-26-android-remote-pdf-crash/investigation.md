# Android 远程 PDF 闪退 — Root Cause Investigation

Date: 2026-08-26 / closed 2026-08-27

> Archived closure note: the original investigation progressed through several distinct failures. Final product-code commit is `0c7e2ed874c5fddf24084a7f12ebd9e9bea2c17e`. Same-device Baidu PDF, local multi-page PDF, and non-PDF Reader regressions all passed before archival. Detailed device evidence is in `device-regression-2026-08-27.md`.

## Original reproduction evidence

User reproduction:

- Platform: Android.
- Reporting device: PGFM10 / OP528F.
- Remote source: Baidu.
- PDF page count: 4.
- Reporting PDF: `001 妖刀浩劫`.

Crash evidence from the supplied logcat/tombstone:

- Process: `com.rch.reader`.
- Native crash: `Fatal signal 11 (SIGSEGV)`, `SEGV_MAPERR`, fault address `0x0`.
- Crashing thread: `tokio-rt-worker`.
- Native backtrace was inside bundled `libpdfium.so` and reached `FPDF_LoadPage+116`.
- Android recorded `APP CRASH(NATIVE)`, status/signal 11.
- RSS around the crash was approximately 367 MB.
- LMKD logging reported sufficient memory and did not kill RCH.

This excluded OOM/LMKD as the cause of the observed incident and directly implicated concurrent Pdfium page access.

## Data-flow analysis

1. Flutter remote-open paths (`openWebdavBook` / `openSftpBook` / `openBaiduBook` / `openCloud115BookFor` / `openQuarkBook`) resolve into the same Rust document layer used for local PDFs.
2. `PdfBook::open()` reads the `ByteSource` into a byte buffer and loads it into Pdfium.
3. The generic Reader performs neighbor-page prefetch while Flutter also requests current/nearby pages.
4. A four-page PDF is sufficient to overlap multiple `PdfBook::page_bytes()` calls.
5. Historical Android PDF acceptance used a one-page fixture, so it could not exercise this race.

## Root cause 1 — concurrent Pdfium FFI access

RCH uses `pdfium-render 0.9.3`. Pdfium itself is not safely re-entrant for these shared-document operations, while the Reader can issue concurrent page requests.

The remediation was a process-wide Pdfium FFI gate in `app/rust/src/document/pdf.rs`, covering:

- Pdfium library/document acquisition and `load_pdf_from_byte_vec()`;
- page-count/page collection access;
- page load → dimensions → render → bitmap → owned image copy;
- `PdfDocument` destruction.

`PdfBook.doc` became an `Option<PdfDocument<'static>>` so destruction could occur under the same gate. WebP encoding remains outside the gate after the pixel data is owned.

### TDD evidence

RED commit: `d0ba3aa2af7a7373aaf9582cf4dd97e8a02f454b`.

The test `pdfium_ffi_gate_serializes_concurrent_calls` required maximum simultaneous occupancy inside the gate to be exactly one. Before implementation it failed because the gate did not exist.

GREEN implementation included the process-wide serialization and passed the targeted regression test.

An early idea to upgrade to `pdfium-render 0.9.4` was discarded after registry verification showed that 0.9.4 was not published; RCH stayed on 0.9.3 and fixed the adapter boundary instead.

## Root cause 2 — Reader L1 mutex self-deadlock introduced during remediation

After the initial crash fix, a transient cross-format regression appeared: non-PDF comics could show many simultaneous spinners and stop progressing.

Real non-PDF diagnostics showed approximately 70 `Reader::get_page()` requests, but only a handful progressed beyond the L1 cache entry point. The key sequence was an L1 hit followed by `spawn_prefetch()`, after which later requests could no longer acquire the same cache mutex.

The remediation branch had changed a safe scoped cache read into an `if let` expression whose temporary mutex guard lived across `spawn_prefetch()`. That caused the same thread to try to lock the cache mutex twice.

The fix restored an explicit local scope so the L1 cache guard is released before prefetch scheduling. A dedicated `reader_l1_hit_deadlock` regression test was added. The user then confirmed the affected non-PDF comic returned to normal loading behavior on-device.

## Root cause 3 — WebP single-dimension limit on ultra-tall PDF pages

With the native crash and Reader deadlock removed, the original four-page PDF still showed persistent spinners.

Diagnostic logging proved the PDF pipeline reached successful Pdfium render and bitmap copy, then failed at WebP encoding with:

`Format error encoding WebP: Invalid dimensions`

Observed rendered heights included:

- 16826 px
- 18864 px
- 20066 px
- 25672 px

All exceeded WebP's 16383-pixel single-dimension limit. The old algorithm always targeted width 1600 and preserved aspect ratio without bounding height.

The final fix keeps normal PDF pages at a 1600-pixel target width, but proportionally scales ultra-tall pages so neither output dimension exceeds 16383 pixels. Regression tests include the real device dimensions and verify aspect-ratio preservation.

## Final product-code commit

`0c7e2ed874c5fddf24084a7f12ebd9e9bea2c17e`

The final product state includes:

- process-wide Pdfium FFI serialization;
- shared Reader in-flight behavior with the L1 cache mutex lifetime corrected;
- removal of the fixed page-0 warm-up so the first real request drives prefetch;
- WebP-safe proportional PDF render dimensions;
- diagnostic hooks used to prove the failure chain;
- regression tests for Pdfium serialization, Reader L1 hit behavior, and ultra-tall PDF dimensions.

## Verification evidence

Final automated verification:

- `rustfmt --edition 2021 --check app/rust/src/document/pdf.rs`: PASS.
- `git diff --check`: PASS.
- final serial Rust suite: PASS — 239 passed, 0 failed, 2 ignored;
- dedicated integration test: PASS — 1 passed, 0 failed;
- `cargo check -p rust_lib_app`: PASS;
- Android arm64 release Rust core build: PASS;
- signed APK verification: PASS, APK Signature Scheme v2/v3, one signer, production-compatible certificate.

Repo-wide `cargo fmt --check` remains noisy due to pre-existing unrelated formatting drift in files such as `scrape_projection.rs` and `scraper.rs`; no unrelated formatting changes were folded into this incident.

## Device closure

- Non-PDF comic that exposed the transient Reader regression: PASS.
- Original Baidu four-page PDF on the reporting device/source: PASS (`PDF正常了`).
- Local multi-page PDF smoke on the same fixed build: PASS (`本地PDF正常`).

All incident-specific acceptance gates are satisfied.

## Explicitly out of scope

`PdfBook::open()` still buffers the whole PDF into a `Vec<u8>`. This remains a separate large-file memory/architecture risk, but it was not the cause of this SIGSEGV/WebP incident and was intentionally not expanded into this P1 bugfix.
