# Android PDF same-device regression — 2026-08-27

## Device and reproduction

- Reporting device: PGFM10 / OP528F.
- Package: `com.rch.reader`.
- Remote source: Baidu.
- Original reproduction: four-page PDF `001 妖刀浩劫`.
- Final product-code commit under test: `0c7e2ed874c5fddf24084a7f12ebd9e9bea2c17e`.
- Test APK was rebuilt from the device's current v0.5.5 base APK, with the final arm64 Rust core injected and re-signed with the production-compatible certificate.
- APK signing verification: v2=true, v3=true, one signer, certificate SHA-256 `ae58956ba4ee51d9d32bfc4eee26e7598b376085bbbd3df4aab918024e503fc3`.

## Root-cause chain confirmed during remediation

1. The original native crash was a Pdfium concurrency failure: Android logcat/tombstone showed SIGSEGV in `libpdfium.so` reaching `FPDF_LoadPage` while multiple page renders overlapped.
2. A process-wide Pdfium FFI gate removed the native crash.
3. During Reader in-flight dedup work, a temporary cross-format regression was introduced by holding the L1 cache mutex across `spawn_prefetch()`. Non-PDF logcat proved requests were blocking before L1 hit/miss. Restoring an explicit cache-guard scope fixed the self-deadlock.
4. After the crash was removed, the reporting PDF still failed because all four rendered pages exceeded WebP's 16383-pixel single-dimension limit. Real device render heights included 16826, 18864, 20066 and 25672 pixels. PDFium render and bitmap copy succeeded; WebP encoding failed with `Invalid dimensions`.
5. The final PDF fix keeps normal pages at a 1600-pixel target width but proportionally scales ultra-tall pages so neither output dimension exceeds 16383 pixels.

## Automated verification for final product commit

For `0c7e2ed874c5fddf24084a7f12ebd9e9bea2c17e`:

- `rustfmt --edition 2021 --check app/rust/src/document/pdf.rs`: PASS.
- `git diff --check`: PASS.
- Full serial Rust test suite: PASS, exit code 0.
- `cargo check -p rust_lib_app`: PASS, exit code 0.
- Android arm64 release core build: PASS.
- Signed APK verification: PASS.

Repo-wide `cargo fmt --check` remains noisy because of pre-existing unrelated rustfmt drift in files such as `scrape_projection.rs` and `scraper.rs`; no unrelated formatting was applied in this P1 fix.

## Device regression result

### Non-PDF regression

User re-tested the non-PDF comic that had shown dozens of simultaneous spinners after the transient Reader regression.

Result: **PASS**. Loading behavior returned to normal after the L1 cache mutex lifetime fix.

### Original Baidu four-page PDF

User installed the final signed APK and re-tested the original reporting PDF on the same Android device/source.

Result reported by user: **PASS — PDF正常了**.

This closes the original same-device remote-PDF failure surface: no persistent spinner was reported and the PDF was readable again after the WebP dimension cap.

## Remaining Trellis closure gate

The task remains `in_progress` only because the PRD explicitly requires a local multi-page PDF smoke regression before archival. Once that local PDF smoke is confirmed, the task can be marked complete/archived.

Whole-PDF buffering into a `Vec<u8>` remains a separate architecture/memory-risk item and is not part of this incident's root cause or minimal fix.
