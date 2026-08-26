# Android 远程 PDF 闪退 — Root Cause Investigation

Date: 2026-08-26

## Current status

Task is `in_progress`. Root-cause gate has been crossed from crash evidence. No product-code fix has been committed yet. A RED regression guard has been staged first at `app/rust/tests/pdfium_dependency_safety.rs`; execution is still pending because the CWapi Slack transport stopped returning claim/response messages after the evidence review.

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
5. Generic `Reader::spawn_prefetch()` launches independent OS threads for neighbor pages. Flutter then also requests the current page and next two pages through `_ensure()`.
6. For a cold multi-page PDF, several `PdfBook::page_bytes()` calls can therefore overlap across threads against the same `PdfDocument`.
7. A 4-page PDF is sufficient for `warm_up()` at page 0 to schedule pages 1, 2, and 3 concurrently.

## Historical validation gap

Android PDF support was accepted on 2026-08-08 using a one-page `dummy.pdf`. A one-page document does not exercise neighbor-page prefetch, so that acceptance did not cover concurrent rendering of a multi-page PDF.

## H1 — confirmed primary root cause

`pdfium-render 0.9.3` is unsound under concurrent use despite its default `thread_safe` feature. RCH uses exactly 0.9.3 with default features enabled. Upstream issue #262 demonstrates that the affected implementation allows safe Rust to concurrently enter non-thread-safe Pdfium state and crash with SIGSEGV/SIGTRAP because `Send + Sync` is exposed without actual FFI serialization.

Why this maps to the observed crash:

- RCH's `Document` contract requires `Send + Sync` and explicitly permits concurrent `page_bytes()` calls.
- `Reader` actively spawns multiple page-prefetch threads.
- `PdfBook` stores one shared `PdfDocument` and performs Pdfium page loading/rendering inside `page_bytes()`.
- The user's real failing input is a four-page Baidu PDF, enough to trigger neighbor prefetch immediately.
- The crash is on `tokio-rt-worker`, not a Dart exception path.
- The native stack reaches `libpdfium.so` `FPDF_LoadPage`, exactly at the page-load boundary exercised concurrently.
- The historical one-page Android acceptance could not exercise this race.

The hypothesis is therefore accepted as the root cause for this defect.

## H2 — excluded for this crash

Android memory pressure from `PdfBook::open()` reading the entire PDF into a `Vec<u8>` remains an architectural risk for large PDFs, but it is not supported as the cause of this incident:

- the observed termination is SIGSEGV/null dereference in Pdfium rather than OOM/allocation failure;
- Android records `APP CRASH(NATIVE)` signal 11;
- LMKD reports sufficient memory around the crash and does not kill RCH.

Whole-PDF buffering should be tracked separately if large-file memory behavior becomes a problem; it should not be mixed into this minimal crash fix.

## Remediation decision

Preferred minimal remediation: upgrade `pdfium-render` from `0.9.3` to exact `0.9.4`.

Reasons:

- upstream 0.9.4 explicitly improves memory safety for `thread_safe` by sequencing Pdfium access behind a mutex;
- 0.9.4 still maps `pdfium_latest` to `pdfium_7881`, matching RCH's existing Android `libpdfium.so` binary, so no native Pdfium ABI migration is required;
- this fixes the unsafe FFI synchronization at the binding layer instead of weakening RCH's generic Reader prefetch or adding a partial application-level mutex;
- no unrelated Reader or remote-source refactor is required.

Fallback only if 0.9.4 fails compatibility/build verification: serialize all Pdfium operations at the RCH PDF adapter boundary and prove that the lock covers complete logical operations. Do not disable global Reader prefetch as the first fix.

## TDD / verification state

RED guard staged first:

- `app/rust/tests/pdfium_dependency_safety.rs`
- expected to fail while `Cargo.lock` resolves `pdfium-render 0.9.3`;
- expected to pass only after the dependency and lock file resolve `0.9.4`.

Required before claiming the fix complete:

1. Observe the regression guard fail on 0.9.3.
2. Change `Cargo.toml` to exact `=0.9.4` and refresh `Cargo.lock`.
3. Observe the same guard pass.
4. Run Rust full tests / checks.
5. Build Android successfully.
6. Re-test the same Baidu four-page PDF on the same device/source and confirm no native crash.
7. Run a local PDF regression and the relevant Baidu open strategy regression.

The task remains `in_progress` until the same-device Baidu regression is confirmed.
