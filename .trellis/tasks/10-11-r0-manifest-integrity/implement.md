# R0 Manifest Integrity Implementation Plan

1. Add tests first for strict collection preflight: missing, empty, damaged JSON, unreadable file, and no network/file writes when preflight fails. Preserve legacy-reader tests and H0 checked-reader status tests.
2. Implement a strict collector-only loader and move it before client creation/network work. Keep the legacy reader unchanged.
3. Add atomic writer tests with injectable write/replace failure points, then implement unique same-directory temp creation, flush/sync, round-trip validation, safe replacement, and best-effort/current-platform parent sync.
4. Add short commit-lock coverage and load-merge-write under the lock. Verify duplicate infohash handling and concurrent update preservation.
5. Run focused Rust tests, formatter, and relevant quality checks. Run the independent Windows Flutter/Rust FFI smoke only if the current workspace can produce and load its own DLL without normal app startup; report it separately from R0.
6. Review diff and Git status. Do not modify SPEC, the main reading chain, Daily/P1 features, or 115 write paths. Do not commit or clean files.

## Acceptance Evidence

- Fault-injection test output for all manifest read/write cases in the task request.
- Cross-platform behavior described from implementation plus official platform contracts; runtime claims limited to platforms actually tested.
- Separate `H0_FFI_PASS` or `H0_FFI_BLOCKED` result, with source/DLL identity evidence.
- Final repository status confirms all pre-existing dirty paths remain present and no commit was made.

## Execution Results — 2026-10-11

- Ruling: use a Windows named mutex and Unix output-directory advisory lock instead of a sidecar lock file. SPEC §10.2 B2 limits persistent plugin outputs to Torrent files and `manifest.json`; OS locks preserve that boundary. Cost if wrong: a platform/filesystem that rejects directory advisory locks will fail the manifest commit safely and needs a platform-specific lock review.
- `cargo test --locked --lib eh_subscription::tests -- --test-threads=1`: 36 passed, 0 failed. This includes Windows replacement of an existing manifest, injected temp-write and pre-replace errors, case-insensitive deduplication, unknown-field preservation, and concurrent merge preservation.
- `RUSTFLAGS=-D warnings cargo check --locked --all-targets`: passed. `rustfmt --edition 2021 --check src/eh_subscription.rs`: passed.
- `cargo build --locked --lib`: passed on Windows; MSVC emitted its existing linker stdout notice while creating the import library and object files.
- `flutter test test/eh_subscription_manifest_ffi_smoke_test.dart --reporter expanded`: 1 passed through the generated FRB binding and current Debug DLL. Valid fixture returned `ready/valid`; damaged fixture returned `damaged/manifest_json_invalid`. DLL SHA-256: `2B8B6DC8396BE414768EA0C8F0BACF8BF7196C88F316FE0D4A536B6690C307AD`; binding SHA-256: `91CD6442181E1CEBCBFBDF58BBE5D8320FFCF17D7FF883AD80BB4264B982D690`.
- `flutter analyze --no-pub`: exit 1 due to 10 existing informational color-channel deprecation notices in `lib/theme/app_theme.dart` and `test/app_theme_mica_test.dart`. The new FFI test analyzed independently with no issues.
- Android cross-check was attempted with `cargo check --locked --lib --target aarch64-linux-android`; it stopped in `ring` because `aarch64-linux-android-clang`/Android NDK is unavailable. Linux and macOS Rust targets are not installed. No platform tools were installed.
- The broad filtered `cargo test --locked manifest_` invocation attempted to compile every integration target and failed from Windows page-file exhaustion (OS error 1455). The focused library test command above completed successfully. Historical 12 parallel Rust failures remain `BASELINE_UNRESOLVED` and were not rerun or changed.
- Power-loss durability was not tested. The stale-temp test uses an orphan-file fixture; it does not simulate forcibly terminating a writer process.
- SPEC unchanged. P0 remains `P0_WEB_WRITE_AUTH_BLOCKED`; P1 remains `NOT_STARTED`. No commit or other Git operation was performed.
