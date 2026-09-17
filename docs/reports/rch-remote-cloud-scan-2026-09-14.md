# Executive Status

**YELLOW / DO NOT RELEASE**

Task 6 adds deterministic provider-labelled contract coverage for WebDAV,
SFTP, Baidu, 115, and Quark, plus a focused Flutter coordinator/status suite.
The fake boundary passes without credentials or live URLs. The scoped Android
closure report provides useful WebDAV/OpenList transport and worker evidence,
but it does not establish cross-provider scan correctness, direct provider UI
performance, or release readiness.

# Environment

| Item | Checked evidence | Status |
|---|---|---|
| Host | Windows worktree on `local-ai/cover-quark-debug`, current commit `2a97c35` | AVAILABLE |
| Flutter / Dart | Flutter 3.44.8 / Dart 3.12.2, as recorded by the scoped 2026-09-13 Android closure | SCOPED |
| Rust | `rust_lib_app` remote-scan integration fixture, serial test execution | AVAILABLE |
| Credentials and provider URLs | Fixtures contain labels, logical paths, and typed outcomes only | SAFE / NO SECRETS |
| Full workspace gates | Rust tests, startup contract, and Windows build completed; Clippy and full Flutter analysis/test remain blocked by existing workspace debt | MIXED / RELEASE BLOCKED |
| Android build environment | Retry after ignored-dir cleanup: `flutter clean` exit 0 and `flutter pub get` exit 0; the canonical APK build failed exit 1 in 20.4s at `:file_selector_android:compileDebugKotlin` because the Kotlin daemon could not close incremental caches (`Storage ... is already registered`) under `D:\Projects\RCH-source\app\build\file_selector_android\kotlin...`. The earlier pre-clean attempt failed after 149.4s on the Pub-cache `C:`/project `D:` drive-root mismatch. An environment-only workaround produced a diagnostic APK, but did not clear the canonical gate | CANONICAL BLOCKED / DIAGNOSTIC ONLY |
| External runtime | Current `RCH_API_36` emulator smoke completed on `emulator-5554`; OpenList loopback route was enabled with `adb reverse tcp:5244 tcp:5244`; emulator was stopped and temporary logs removed | BASELINE SMOKE / INSTALLER PENDING |

# Android Smoke

The controller performed a bounded API 36 emulator smoke using the fresh APK
produced by the environment-only Kotlin incremental-cache workaround. This is
deployment-baseline evidence only and is not Android system Package Installer
handoff evidence.

| Check | Evidence | Status |
|---|---|---|
| AVD startup | `RCH_API_36` started successfully; `adb devices` reported `emulator-5554 device` | PASS |
| Boot/runtime | `sys.boot_completed=1`; Android 16 / API 36 / x86_64; `flutter devices` recognized the emulator | PASS |
| APK identity | `com.rch.reader`; versionName `0.5.7`; versionCode `100507`; 273,756,657 bytes; SHA-256 `DA5B263E96E5A4877C2D4EA061A7AAA4AA1A9C2C75B0F74628CA3072FFDA4A01` | PASS |
| Baseline deployment | `adb install -r` exit 0, `Success`, approximately 4483 ms | PASS / BASELINE ONLY |
| Baseline launch | `com.rch.reader/.MainActivity` launch exit 0, approximately 1019 ms | PASS / BASELINE ONLY |
| Local service route | `adb reverse tcp:5244 tcp:5244` succeeded | PASS |
| Teardown | Emulator stopped; temporary logs removed | PASS |

`adb install -r`/direct activity launch only establish that the debug APK can
be deployed and opened on the emulator. They do not validate Package Installer
handoff, user cancellation/retry, unknown-source policy, same-version or
downgrade behavior.

# Provider Matrix

| Provider | Fake contract coverage | Real-provider evidence in scope |
|---|---|---|
| WebDAV | PASS: labelled pagination, nullable mtime, nested image folder, typed failures, strict range, and verified tombstone | SCOPED WebDAV/OpenList transport and Android worker evidence exists in the 2026-09-13 closure; direct scan/UI/performance evidence is PENDING |
| SFTP | PASS: same adapter boundary and typed outcomes | PENDING; no authorized SFTP sample was available |
| Baidu | PASS: same adapter boundary and typed outcomes | PENDING; no authorized Baidu sample was available |
| 115 | PASS: same adapter boundary and typed outcomes | PENDING; no authorized 115 sample was available |
| Quark | PASS: same adapter boundary and typed outcomes | PENDING; no authorized Quark sample was available |

The matrix is intentionally provider-neutral at the transport boundary. It
does not claim that a fake response proves a provider's current API behavior.

# Scan Evidence

The Rust contract fixture exercises all five provider labels serially. It
asserts that a complete multi-page listing follows its cursor and commits one
merged directory, while an interrupted later page returns a typed transient
error and commits nothing. Unknown `mtime` remains `None`; hidden entries are
filtered; direct image children classify a nested directory as `ImageFolder`
and produce one folder cover task.

The same matrix asserts typed `Unauthorized`, `Forbidden` (403), `NotFound`
(404), `RateLimited` (429), and `RangeUnavailable` outcomes without a commit.
A test-only range parser accepts only a matching `206` and `Content-Range`;
`200`, `416`, missing, malformed, and mismatched headers are rejected as
`RangeUnavailable`.

The proof fixture binds generation to source identity, effective root, and
session epoch. A complete successful listing marks only the missing direct
child as a verified tombstone and returns its cover dependency; a partial
listing leaves the previous row live. No fake outcome authorizes whole-book
fallback or destructive cleanup.

The Flutter suite covers stable provider labels, one initial full scan followed
by incremental and explicit manual modes, duplicate-job joining, automatic
pause while manual scan remains available, the background cover gate, retained
cover state, and redacted status text.

# Reader/Cache Evidence

The scoped 2026-09-13 Android closure records WebDAV/OpenList first-page reads,
20/100/300 MiB worker measurements, cancellation probes, and a page/raw cleanup
boundary that retained live covers and metadata. Those observations remain
scoped to that provider/runtime path and are not generalized to SFTP, Baidu,
115, or Quark.

The current fake gate test confirms that disabling cover work does not invoke
the operation or clear an existing cover ledger. End-to-end remote image-folder
Reader behavior, verified-deletion cache removal on a real provider, direct UI
provider performance, and a 50+ page long-strip run are not complete.

# Automated Verification

| Exact command | Result | Scope / note |
|---|---|---|
| `cargo fmt --all -- --check` | PASS | No formatting differences reported |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | FAIL (exit 101) | 69 library + 74 library-test existing lint diagnostics; no diagnostics from the new `remote_scan_contract.rs` |
| `cargo test --workspace --locked -j 1 -- --test-threads=1` | PASS (279 passed, 0 failed, 2 ignored; integration tests 1 and 18 passed) | Full Rust workspace regression gate |
| `cd app/rust; cargo test --test remote_scan_contract -- --test-threads=1` | PASS (18 tests) | Provider matrix, range fixture, pagination, lifecycle, and tombstone contracts |
| `cd app; flutter test --no-pub test/remote_scan_provider_contract_test.dart test/remote_scan_ui_regression_test.dart test/remote_scan_coordinator_test.dart test/remote_scan_status_test.dart` | PASS (24 tests) | Focused provider/coordinator/status/UI suite |
| `cd app; dart format test/remote_scan_provider_contract_test.dart` | PASS | Owned Dart test is formatted |
| `cd app; dart analyze test/remote_scan_provider_contract_test.dart` | PASS | No issues found |
| `cd app; flutter analyze --no-pub test/remote_scan_provider_contract_test.dart` | PASS | No issues found |
| `flutter analyze --no-pub` | FAIL (61 issues) | Existing unrelated workspace issues; no diagnostics were reported in the remote-scan files |
| `flutter test --no-pub` | FAIL (132 passed; 7 existing test files failed to compile) | Existing API-mismatch failures in the dirty workspace; focused remote-scan suite remains PASS |
| `dart run tool/startup_contract_check.dart` | PASS | Startup contract gate |
| `flutter clean` | PASS (exit 0) | Ignored build outputs were cleaned before the APK retry |
| `flutter pub get` | PASS (exit 0) | Dependencies resolved before the APK retry |
| `flutter build apk --debug` | FAIL (exit 1; 20.4s) | Clean retry failed at `:file_selector_android:compileDebugKotlin`: Kotlin daemon `Could not close incremental caches` / `Storage ... is already registered` under `D:\Projects\RCH-source\app\build\file_selector_android\kotlin...`. The earlier pre-clean attempt failed after 149.4s on the Pub-cache `C:`/project `D:` drive-root mismatch. No fresh APK was produced by the canonical command; the stale 2026-09-14 APK is not release evidence |
| `flutter build apk --debug --android-project-arg=kotlin.incremental=false` | PASS (exit 0; 364.1s) | Environment-only workaround; no production configuration changes. APK `app/build/app/outputs/flutter-apk/app-debug.apk`, 273,756,657 bytes, SHA-256 `DA5B263E96E5A4877C2D4EA061A7AAA4AA1A9C2C75B0F74628CA3072FFDA4A01`; aapt package `com.rch.reader`, versionName `0.5.7`, versionCode `100507`, compileSdk 36. Diagnostic evidence only; it does not make the canonical APK gate pass |
| `flutter build windows` | PASS (62.6s) | `build\\windows\\x64\\runner\\Release\\RCH.exe`; version `0.5.7+100507`; SHA-256 `E36751EEEC80B3058B0DDE0124C9F9FB8876E2F22F65CEBA09CEFE56072EE702` |
| `adb devices` / API 36 smoke | PASS | `RCH_API_36` / `emulator-5554` online; boot completed, Android 16 / API 36 / x86_64; `flutter devices` recognized it |
| `adb install -r` + direct `MainActivity` launch | PASS | Deployment baseline only: install `Success` in approximately 4483 ms; launch exit 0 in approximately 1019 ms. Not Package Installer handoff evidence |
| `adb reverse tcp:5244 tcp:5244` | PASS | Emulator-to-host OpenList loopback route established; emulator stopped and temporary logs removed after smoke |
| `git diff --check` | PASS | No whitespace errors reported; unrelated dirty files remain untouched |

The full gate results do not upgrade the release status: the Clippy failure,
full Flutter analysis/test failures, and canonical Android APK build failure
are recorded as current blockers. The canonical APK failure remains after a
successful clean and dependency resolution: the first attempt exposed a
cross-drive cache mismatch, and the clean retry exposed a Kotlin
incremental-cache registration failure. An environment-only incremental-cache
workaround produced a fresh diagnostic APK and matching manifest/hash evidence,
without production configuration changes, but it does not replace the
canonical gate. The diagnostics are attributable to existing workspace debt or
the build environment where stated; the new remote-scan contract files have
independent focused coverage and no reported diagnostics.

# Pending External Validation

The following items remain **PENDING** or **NOT COMPLETE** and must not be
converted to release evidence without fresh, reproducible runs:

- Real SFTP, Baidu, 115, and Quark authentication, listing, pagination,
  range/fallback, scan recovery, and deletion samples.
- Direct credentialed provider UI behavior and provider-specific performance,
  including WebDAV direct scan/UI measurements.
- Migration cases: first JSON import, valid WAL/SHM, corruption, interruption,
  and recovery/idempotency.
- OEM and real-device behavior, including background lifecycle and memory
  measurements.
- Installer user outcomes and policy cases, including Windows UAC and Android
  Package Installer cancel/retry/unknown-source/same-version/downgrade paths;
  the `adb install -r` emulator smoke above is deployment-baseline evidence only.
- 50+ page long-strip/Reader stress and forced truncated-response handling.
- The manually reviewed M8 truth set, which is **NOT COMPLETE**.

Release-gate labels (machine-readable):

- `REAL_PROVIDER_PERF_PENDING`
- `REAL_PROVIDER_TRUNCATED_RESPONSE_PENDING`
- `OEM_CLEANER_REAL_DEVICE_PENDING`
- `REAL_DEVICE_LONG_STRIP_PENDING`
- `REAL_DEVICE_INSTALLER_PENDING`
- `M8_TRUTH = NOT COMPLETE`

# Git Boundary

No version bump, tag, release, push, or commit was made. Existing tracked and
untracked work from other tasks was preserved. The contract additions are
limited to the Rust/Dart tests, this report, additive spec/task references, and
the parent context-manifest references; live credentials, private URLs, and
user originals were not copied into the repository.

# Final Recommendation

**YELLOW / DO NOT RELEASE**

The deterministic contract boundary is ready for review, but the failed
Clippy/full Flutter gates and blocked Android APK build must be resolved or
reproduced with clean, attributable evidence before release consideration.
Cross-provider real evidence, direct UI/performance validation, migration fault
cases, device/OEM coverage, installer user outcomes, long-strip/truncation
checks, and the M8 truth set also remain open.
