# Android 远程 PDF 闪退 — Root Cause Investigation

Date: 2026-08-26 / updated 2026-08-27

## Current status

Task remains `in_progress`. Root cause is confirmed, a minimal product fix has been committed, the RED→GREEN regression cycle is complete, Windows Rust regression passes, and the Rust core cross-compiles successfully for Android arm64. Final closure still requires installing a patched Android build and re-testing the same Baidu four-page PDF on the reporting device/source.

Current verified code head: `cbaf6134101688945b5c6d84c3e03ba83b191dea` on `audit/08-26-trellis-state-reconciliation`.

## Reproduction evidence

User reproduction:

- Platform: Android.
- Remote source: Baidu.
- PDF page count: 4.
- Failing run captured by `adb logcat` on 2026-08-26.

Crash evidence from the supplied logcat:

- Process: `com.rch.reader`.
- Native crash: `Fatal signal 11 (SIGSEGV)`, `SEGV_MAPERR`, fault address `0x0`.
- Crashing thread: `tokio-rt-worker`.
- Native backtrace is inside bundled `libpdfium.so` and reaches `FPDF_LoadPage+116`.
- Android records the exit as `APP CRASH(NATIVE)`, status/signal 11.
- RSS around the crash is approximately 367 MB.
- Low-memory killer logging around the same interval reports sufficient memory and does not select RCH for killing.

This evidence rejects an OOM/LMKD explanation for the observed crash and directly matches concurrent Pdfium page access.

## Observed application data flow

1. Flutter `ReaderPage._open()` opens remote books through `openWebdavBook` / `openSftpBook` / `openBaiduBook` / `openCloud115BookFor` / `openQuarkBook`.
2. Remote open resolves to `document::open_document(...)` and therefore the same `PdfBook` implementation used by local PDFs.
3. `PdfBook::open()` reads the entire `ByteSource` into one `Vec<u8>` and loads that buffer into Pdfium.
4. `register_book()` wraps the PDF in the generic `Reader` and immediately calls `reader.warm_up()`.
5. Generic `Reader::spawn_prefetch()` launches independent OS threads for neighbor pages. Flutter also requests the current page and next pages through `_ensure()`.
6. For a cold multi-page PDF, several `PdfBook::page_bytes()` calls can overlap across threads against Pdfium state.
7. A four-page PDF is sufficient for `warm_up()` at page 0 to schedule pages 1, 2, and 3 concurrently.

## Historical validation gap

Android PDF support was accepted on 2026-08-08 using a one-page `dummy.pdf`. A one-page document does not exercise neighbor-page prefetch, so that acceptance did not cover concurrent rendering of a multi-page PDF.

## H1 — confirmed primary root cause

RCH uses `pdfium-render 0.9.3`. The affected binding surface permits safe Rust callers to enter Pdfium concurrently even though Pdfium itself is not re-entrant. RCH's `Document` contract is `Send + Sync`, and the generic Reader deliberately performs neighbor-page prefetch concurrently.

Why this maps to the observed crash:

- `Reader` actively spawns multiple page-prefetch calls.
- `PdfBook` shares one Pdfium document and performs page loading/rendering in `page_bytes()`.
- the real failing input is a four-page Baidu PDF, enough to trigger neighbor prefetch immediately;
- the crash is native on `tokio-rt-worker`, not a Dart exception path;
- the native stack reaches `libpdfium.so` `FPDF_LoadPage`, exactly at the page-load boundary exercised concurrently;
- the historical one-page Android acceptance could not exercise this race.

The hypothesis is therefore accepted as the root cause for this defect.

## H2 — excluded for this crash

Android memory pressure from `PdfBook::open()` reading the entire PDF into a `Vec<u8>` remains an architectural risk for large PDFs, but it is not supported as the cause of this incident:

- the observed termination is SIGSEGV/null dereference in Pdfium rather than OOM/allocation failure;
- Android records `APP CRASH(NATIVE)` signal 11;
- LMKD reports sufficient memory around the crash and does not kill RCH.

Whole-PDF buffering should be tracked separately if large-file memory behavior becomes a problem; it is not part of this minimal crash fix.

## Upstream-version correction

An initial remediation candidate was `pdfium-render 0.9.4`, based on upstream source documentation describing improved `thread_safe` memory safety. Registry verification on 2026-08-26/27 showed that this version is **not published** to the crates.io registry: the rsproxy Sparse Index contains 88 `pdfium-render` versions and currently ends at `0.9.3`. Requests for a `0.9.4` crate do not resolve to a published package.

Therefore the temporary version-only regression guard was based on an invalid distribution assumption and was retired. RCH remains on the published `0.9.3`; the fix is implemented at the RCH PDF adapter boundary.

## Selected remediation

Use one process-wide PDFium FFI gate in `app/rust/src/document/pdf.rs`.

The gate covers the full Pdfium object lifetime and logical operations:

- library/document acquisition and `load_pdf_from_byte_vec()`;
- `page_count()` / page collection access;
- `load page → width/height → render → bitmap → owned DynamicImage`;
- `PdfDocument` destruction in `Drop`.

`PdfBook.doc` is stored as `Option<PdfDocument<'static>>` so its destructor can be explicitly executed while the same global gate is held. WebP encoding happens after `DynamicImage` owns its pixels and therefore remains outside the gate, avoiding unnecessary serialization of pure CPU encoding.

No Reader prefetch behavior, Baidu source behavior, remote-open strategy, Cargo dependency, or Pdfium native ABI was changed.

## TDD evidence

### RED

Commit: `d0ba3aa2af7a7373aaf9582cf4dd97e8a02f454b` — `test(pdf): reproduce missing global Pdfium serialization`

Behavior test: `document::pdf::tests::pdfium_ffi_gate_serializes_concurrent_calls` starts two worker threads and requires the maximum simultaneous occupancy inside the global Pdfium gate to be exactly one.

Before production implementation, CWapi ran:

`cargo test --lib pdfium_ffi_gate_serializes_concurrent_calls`

Expected RED was observed at compile time:

- `error[E0425]: cannot find function with_pdfium_lock in this scope`
- location: `src/document/pdf.rs:146`

This was the intended failure, not an environment/build failure.

### GREEN

Commit: `4980f090a7b69ad7d68ee3e39596f5d10f595e9e` — `fix(pdf): serialize Pdfium access process-wide`

The same targeted test passed:

- `1 passed`
- `0 failed`
- `237 filtered out`
- test time approximately `0.08s`.

The obsolete unpublished-version guard was then removed in commit `cbaf6134101688945b5c6d84c3e03ba83b191dea` — `test(pdf): retire unpublished-version guard`.

## Verification evidence

### Formatting

- `rustfmt --check src/document/pdf.rs`: PASS.
- repo-wide `cargo fmt --check`: FAIL on pre-existing unrelated formatting drift in files such as `scrape_projection.rs` and `scraper.rs`; no unrelated formatting was applied as part of this P1 bugfix.

### Windows / host Rust

On `cbaf6134101688945b5c6d84c3e03ba83b191dea`:

- `cargo test`: PASS — `236 passed, 0 failed, 2 ignored`.
- `cargo check`: PASS, exit code 0.

### Android arm64 Rust core

The project Cargokit configuration was inspected before reproducing its Android environment. Verified local values:

- Android SDK: `C:/Users/cfl/AppData/Local/Android/Sdk`
- NDK: `28.2.13676358`
- Flutter minSdk: `24`
- Rust target: `aarch64-linux-android` already installed.

A direct Cargokit-equivalent offline/locked build was executed with NDK clang/clang++, llvm-ar, llvm-ranlib, API 24 linker target, and the NDK 23+ libgcc→libunwind workaround:

`rustup run stable cargo build --offline --locked ... --target aarch64-linux-android`

Result:

- exit code `0`;
- `librust_lib_app.so` generated successfully;
- Debug artifact size: `339469552` bytes.

This proves the PDF fix compiles and links for the actual Android arm64 Rust target.

### Full Flutter APK build

A full `flutter pub get --offline` + arm64 Debug APK build was also started in a CWapi detached worktree. It produced no terminal error but remained running beyond the CWapi continuous-polling budget, so it was stopped after roughly three minutes. That run is **inconclusive**, not a pass or a failure. The shorter Android Rust target gate above was used to isolate and verify the native-code change itself.

## Remaining closure gates

1. Produce/install a patched Android APK containing `cbaf613...` or a descendant with the same PDF fix.
2. Re-test the same Baidu four-page PDF on the reporting device/source and confirm there is no native crash.
3. Perform a local multi-page PDF smoke regression.
4. If the Baidu reproduction is clean, update Trellis evidence and close/archive this bug task.

The task remains `in_progress` until the same-device Baidu regression is confirmed.
