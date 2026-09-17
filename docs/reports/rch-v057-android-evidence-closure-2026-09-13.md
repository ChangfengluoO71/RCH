# RCH v0.5.7 Android Evidence Closure Report

Date: 2026-09-13 (Asia/Shanghai)

Baseline: `v0.5.7` / `d11243303079b04aed3045c018df223d8e4b6ec8`

Branch: `local-ai/cover-quark-debug`

Evidence directory: `D:\Temp\rch-v057-android-evidence-closure-20260913`

## Executive Status

**YELLOW**

**DO NOT RELEASE**

This round installed the requested Android SDK components and accepted the seven
SDK licenses with the user's explicit authorization. A standard product APK was
built in a one-shot Visual Studio Developer environment (including the Rust
native library), an API 36 emulator was created, and the app installed and
launched without a native-library crash. Runtime Keystore probing exposed two
Android-only defects (caller-supplied GCM IV and an empty sync credential
reference); both received minimal regression-tested fixes. A non-secret seeded
fixture then survived cache-boundary checks, app/emulator restart, and
over-install, with SQLite integrity and all seeded rows retained. A temporary
in-memory-auth-proxy run also exercised the Android Rust/Flutter worker against
the authenticated OpenList service for approximately 20/100/300 MiB downloads,
first-page reads, fixed-buffer RSS sampling, and early/middle/near-complete
cancellation. Migration failure/WAL interruption cases, direct credentialed
Android UI flows, Package Installer user/policy outcomes, real-device/OEM checks, and the M8
truth set are still missing. The release decision therefore remains YELLOW /
DO NOT RELEASE.

## Environment

| Item | Observed value | Status |
| --- | --- | --- |
| Flutter / Dart | Flutter 3.44.8 / Dart 3.12.2 | PASS |
| JDK | Temurin OpenJDK 25.0.2+10 | PASS; Gradle starts |
| Gradle / AGP / Kotlin | 9.1.0 / 9.0.1 / 2.3.20 | pinned; not upgraded |
| Android SDK root | `C:\Users\cfl\AppData\Local\Android\Sdk` | PASS |
| Platform / Build Tools | API 36 / 36.0.0 | installed |
| Platform-tools / Emulator | 37.0.1 / 37.1.11 | installed |
| NDK | 28.2.13676358 | installed |
| System image / AVD | API 36 Google APIs x86_64 / `RCH_API_36` (Pixel 2 profile) | installed; booted |
| Rust Android targets | arm64, armv7, i686, x86_64 | installed |
| SDK licenses | all seven accepted | PASS |
| Native host build environment | VS Developer Command Prompt 17.14.37; Temurin JDK 25 process-local | required for Cargokit host link |

The official command-line-tools archive used for installation is retained in
the evidence directory (SHA-256
`90AE805D20434428BFFCB699C290860F19BB5F66A67E6B330067E3DE801FB04A`). No
CMake installation was needed: the native build scripts use `cc` and the NDK
LLVM toolchain. Android environment variables were process-local only.

The first retry showed TLS handshake failures, but a later endpoint probe
recovered official Google Maven, Maven Central, and Gradle Plugin Portal access
(`phase-c-endpoint-probe-retry2.txt`). No mirror or insecure TLS bypass was
enabled. The default PowerShell process still lacks the Visual Studio linker
environment needed by Cargokit's host build; the successful retry used a
one-shot VS Developer command prompt and did not alter repository configuration.

## Android Build

`flutter clean` and `flutter pub get` completed. The current ignored
`app/android/local.properties` contains `flutter.versionName=0.5.7` and
`flutter.versionCode=100507`, matching `app/pubspec.yaml` (`0.5.7+100507`).

The first APK retries exposed a Kotlin incremental-cache cross-drive failure
(`phase-c-apk-build-retry3.log`, `phase-c-apk-build-retry4.log`). A direct
Gradle retry with `--project-prop=kotlin.incremental=false` returned success,
but archive inspection found no `librust_lib_app.so`; that APK was rejected.
Subsequent Flutter retries also demonstrated that an outer exit code can be a
Cargokit false positive when the Rust sub-build fails (spki/API and host-link
failures in `phase-c-apk-build-retry7.log` and
`phase-c-apk-build-retry8.log`).

The valid retry ran, in one process, from the VS Developer Command Prompt with
JDK 25, SDK 36, NDK `28.2.13676358`, target-specific NDK clang variables, and
`GRADLE_OPTS=-Dorg.gradle.project.kotlin.incremental=false`:

```text
call VsDevCmd.bat -arch=x64 -host_arch=x64
flutter clean && flutter pub get && flutter build apk --debug
```

It completed in about 389.4 seconds. The inspected artifact is
`app/build/app/outputs/flutter-apk/app-debug.apk`, 273,923,961 bytes,
SHA-256 `5E03366878B689DD51410FA906FD340AC3E05E3BA4DF78F7FF24626019AAC974`.
`aapt2 dump badging` reports package `com.rch.reader`, version `0.5.7`, code
`100507`, min/target SDK 24/36, and native ABIs arm64-v8a, armeabi-v7a, and
x86_64. Each packaged ABI contains `librust_lib_app.so`. The build log is
`phase-c-apk-build-retry9-vsdevcmd.log`; artifact inspection is recorded in
`apk-artifact-retry9-vsdevcmd.txt`.

`RCH_API_36` (Pixel 2 / Google APIs / x86_64) was created and booted. `adb`
reported `emulator-5554 device`, SDK 36, ABI x86_64; `flutter devices` listed
the emulator. `adb reverse tcp:5244 tcp:5244` succeeded. Installing the valid
APK with `adb install -r` and launching `com.rch.reader/.MainActivity` worked;
there was no FATAL or `dlopen` error. This deployment is only a test baseline,
not Package Installer acceptance.

The first Android integration probe temporarily replaced `app-debug.apk` with
the Flutter test harness (its VM root was `flutter_test_listener` and it stayed
on the test Splash); that artifact was not counted as a product launch. After
removing the probe, a final standard rebuild (retry31) completed successfully;
the product install/launch was captured on retry32.
The current inspected product artifact is
`app/build/app/outputs/flutter-apk/app-debug.apk`, 317,285,358 bytes,
SHA-256 `C933A6736EDCB77EE1F664A0B8E050343D6E289C3DA64DF1D1679C26485BC879`.
Badging again reports package `com.rch.reader`, version `0.5.7`, code `100507`,
min/target SDK 24/36, and launch activity `com.rch.reader.MainActivity`.
The final build/install/launch evidence is in
`android-standard-final-launch-retry32.log`; `adb install` remains deployment
baseline evidence only and is not Package Installer acceptance.

The later Android integration harness APK was a temporary test artifact and is
not counted as the product build. It was used only on `RCH_API_36` and removed
from the emulator during teardown.

Auxiliary native checks passed after explicitly adding the installed NDK LLVM
directory to the process environment:

```text
cargo check --workspace --locked --target aarch64-linux-android  PASS
cargo check --workspace --locked --target armv7-linux-androideabi PASS
cargo check --workspace --locked --target i686-linux-android   PASS
cargo check --workspace --locked --target x86_64-linux-android PASS
```

These checks do not replace seeded persistence, Keystore, provider, or installer
evidence. The earlier TLS and Cargokit failures remain reproducibility notes,
not reasons to treat the inspected retry9 APK as invalid.

## Persistence P0

The implementation and unit-level contracts are present: Android startup sets
the durable root to `filesDir/RCH/data`; cache contents stay under the cache
root; legacy database candidates have deterministic precedence; migration
handles `database.db`, `-wal`, and `-shm`, validates SQLite integrity/schema,
uses temporary copies and atomic replacement, and preserves a partial marker on
failure. Android startup enters a protected/degraded state instead of opening a
new empty database when an existing candidate cannot be verified.

Static coverage passed in `cargo test` and the focused Flutter regression run,
including storage layout, deterministic candidates, redacted JSON, migration
failure handling, and cache cleanup contracts. **Runtime status: PARTIAL /
SEEDED EMULATOR FIXTURE**. A non-secret fixture created a local source, source
credential reference, settings, metadata (including custom crop parameters),
read history, tags, and page/raw/cover cache files. The seed probe passed with
`files/RCH/data/database.db` and the encrypted vault file in app-private
storage; page/raw cleanup removed only rebuildable cache while the cover stayed.
The evidence is `android-p0-seed-fixture-retry15.log`.

The standard product APK was then installed over the seeded package, launched,
and the emulator was rebooted. Lifecycle readback after reboot and over-install
reported `vault=present`, `db=verified`, and retained sources=1, metas=1,
records=1, tags=4, book_tags=4, settings=26; the final over-install readback is
in `android-standard-final-launch-retry32.log`, and the lifecycle probes are
`android-p0-lifecycle-retry17.log` and
`android-p0-lifecycle-after-reboot-retry18.log`. `cmd package trim-caches 1G`
was also executed; the small fixture cache was not evicted, and the same rows
remained intact. This is evidence for the durable-root/cache boundary and
restart/over-install behavior, not for every migration path.

A first synthetic probe used a nonexistent local root (`/evidence`) and the
normal stale-local-file policy removed its associated record/meta/tag rows;
the rerun with an actual isolated fixture file retained them. This is recorded
as a fixture caveat, not a database-corruption finding. First JSON migration,
valid WAL/SHM migration, corruption/interruption recovery, and migration
interruption recovery remain pending. The Android Settings `Clear Cache` UI
path is now covered separately below. The empty sync credential-reference
startup bug was fixed separately and covered by a focused regression test (see
Keystore below).

## Storage Classification

| Data | Android target | Rebuildable | Survives Clear Cache | Sensitive |
| --- | --- | ---: | ---: | ---: |
| SQLite, sources, settings, tags, history, cover parameters | `filesDir/RCH/data` | No | Yes | No |
| Provider credentials, sync password | Keystore + private ciphertext file | No | Yes | Yes |
| `library.json` recovery snapshot | app support; Android redacted | Partial | Yes | No |
| Imported books | app support/books | No | Yes | No |
| page/raw/cover/AI/thumbnails, snapshots, error logs | cache root or temp | Yes | No | logs redacted |
| `.part`, temporary files, update APKs | temp/cache/external-files | Yes | No | No |

Android cache migration is limited to `cache/` and rebuildable files; it does
not copy the database, credentials, or `library.json`. Windows retains its
historical single user-root database/cache contract.

## Keystore

`CredentialVault.kt` uses an Android Keystore AES-256-GCM key, random 12-byte IVs,
app-private ciphertext, a `.part` write, and a rollback backup. The ciphertext
and backup are excluded from Android backup/device transfer. The Dart boundary
is limited to `rch/credentials` `put`, `get`, and `delete`; source JSON keeps a
`credentialRef` and omits sensitive columns after migration.

The full Flutter regression suite passed 113 tests, including the new
empty-reference regression. A repository scan found
no bearer token or non-placeholder JSON secret values; the only URL-auth matches
are parser/test patterns in `app/rust/src/db/mod.rs`. `RCH_TEST_OPENLIST_PASSWORD`
was not present in the current process. A second scan of the text evidence files
found no URL basic-auth, Bearer, Authorization header, or password assignment.

**Runtime status: PASS / EMULATOR CHANNEL SMOKE (sentinel only)**. The first
`rch/credentials` put/get/delete run failed with AndroidKeyStore's
`Caller-provided IV not permitted`. The root cause was the combination of
`setRandomizedEncryptionRequired(true)` with a caller-supplied GCM IV. The
minimal fix lets AndroidKeyStore generate the IV and persists the returned IV
prefix; the rerun passed 1/1 (`android-keystore-runtime-retry11.log`). The
empty-sync-reference startup bug was fixed by normalizing blank refs to null and
covered by `sync_manager_credential_ref_test.dart`; the standard APK then
launched without the prior `missing credential key` error. The runtime probe used
only a non-secret sentinel and deleted it. A second lifecycle probe read the
existing sentinel vault entry after over-install, `cmd package trim-caches`, and
emulator reboot (`android-p0-lifecycle-after-reboot-retry18.log`), while the
seeded DB and vault remained intact. This validates emulator app/restart and
over-install behavior only; real provider credentials and OEM cleaners remain
pending. The Android Settings Clear Cache UI run is covered in the Cache Cleanup
section. The permanent Android-only
round-trip regression (`integration_test/android_credential_vault_test.dart`)
also passed on the emulator (`android-keystore-regression-final-retry30.log`).
No provider secret was added to the emulator, repository, logs, or report.

## OpenList Local WebDAV

The service is reachable on `127.0.0.1:5244`. The unauthenticated challenge
remains `401 WWW-Authenticate: Basic realm="openlist"`; a hidden interactive
credential was then used for the evidence run (the password is not present in
any command, log, fixture, screenshot, or report). Authenticated WebDAV
returned `207 Multi-Status` for `/dav/`. The bounded traversal recorded 500
PROPFIND requests, 4,934 entries, and 4,375 files (the traversal limit was hit;
the counts are not a complete library inventory).

For one real CBZ in each size bucket, HEAD returned `200` with an ETag; single
Range returned `206` with the expected `Content-Range`; two disjoint ranges
returned `206 multipart/byteranges`; and an out-of-bounds Range returned `416`
with `Content-Range: bytes */<size>`. The selected sizes were 20,564,803,
106,466,694, and 312,003,874 bytes. Only status, range, length, path hash,
timing, and final SHA-256 were written. Evidence: `openlist-auth-range-
benchmark-retry4.txt` (the earlier unauthenticated probe is retained in
`openlist-unauth-status-retry2.log`).

## Download Benchmark

Authenticated local OpenList transport benchmarking is now available. Real CBZ
samples were streamed into an isolated temporary fixture and deleted after each
run; user originals and the OpenList library were not modified. Serial and
two-segment downloads completed with matching hashes:

| Bucket | Bytes | Serial | 2 segments | Peak RSS delta |
| --- | ---: | ---: | ---: | ---: |
| ~20 MiB | 20,564,803 | 0.244 s / 80.355 MiB/s | 0.261 s / 75.220 MiB/s | 1.758 / 0 MiB |
| ~100 MiB | 106,466,694 | 1.101 s / 92.232 MiB | 1.064 s / 95.425 MiB/s | 5.941 / 1.664 MiB |
| ~300 MiB | 312,003,874 | 3.244 s / 91.719 MiB/s | 3.028 s / 98.270 MiB/s | 2.453 / 0.699 MiB |

Every run used a fixed 1 MiB stream buffer, atomically promoted `.part` only
after completion, and reported `part_after=False`, `final_after=True`. Serial
and segmented SHA-256 values matched for each bucket. The measured host-process
RSS deltas are below 128 MiB and the 100→300 MiB delta did not grow 2×. This is
transport evidence, not the Android Rust/Flutter worker's peak RSS;
the Android app-process worker was measured separately below.

An additional host Flutter/Rust smoke used the real `webdavConnect`,
`openWebdavBook(strategy: download)`, `bookPage`, and cache APIs against the
same authenticated provider. It downloaded 20,000,978, 102,131,070, and
323,401,152-byte CBZs, opened the first page successfully, and reached progress
`1.0`. The recorded wall times were 0.103 s, 0.203 s, and 0.406 s with
`ProcessInfo.currentRss` deltas of 0.609, 0.113, and 0 MiB. The fixture cache
root was outside the repository and deleted in teardown. This closes a useful
desktop app-worker smoke. The passing test output is `openlist-app-runtime-retry6.log`; the
earlier cache-hit cancellation attempt is retained as
`openlist-app-runtime-retry5.log` and is not counted as a pass.

The Android app-process worker was then exercised through a temporary
localhost-only proxy that held the OpenList password in memory and injected
the upstream Basic header; no credential was placed in the test APK, command
line, logs, or report. The emulator connected to the proxy through `adb
reverse`, while the proxy forwarded the real WebDAV requests. Discovery made
three PROPFIND calls and found 240 candidate files. The worker downloaded and
opened a real CBZ in each bucket, reached `raw=true`, and produced matching
SHA-256 values. Wall times were 1.078 s / 5.312 s / 15.496 s, with sampled
peak RSS deltas of 1.625 / 0.5 / 1.25 MiB for ~20 / ~100 / ~300 MiB. The
complete output is `android-openlist-app-worker-retry36.log`. This closes the
Android emulator worker/performance smoke; it does not replace direct
credential-vault hydration through the product UI or real-device RSS evidence.

## Cancellation / Recovery

Static tests cover strict 206/`Content-Range` validation, malformed/short
segment fallback, cancellation markers, `.part` removal without deleting a
formal cache, version mismatch protection, update single-flight behavior,
hash mismatch rejection, and preservation of a verified package after failed
handoff. The Rust test suite and focused Flutter tests pass. Authenticated
transport cancellation probes against the ~300 MiB CBZ completed three early,
three middle, and three near-complete range-abort rounds; each stopped after a
small read and left `part_after=False`. A host Flutter/Rust app smoke then
started a real 300 MiB `openWebdavBook(strategy: download)`, called
`webdavCancelDownload` at progress about 0.186, observed the download future
fail, found no raw cache and no `.part` file, and passed. The Android emulator
worker run repeated cancellation at approximately 10%, 50%, and 90%; all three
futures failed as cancelled, `raw=false`, and `part_files=0`, with peak RSS
deltas of 1.0 / 1.078 / 4.035 MiB. Android UI state, connection/handle release
under OEM conditions, single-flight state, N1-N6 recovery, and a truncated-
response fixture still need validation.
A truncated provider response was not forced
(`TRUNCATED_RESPONSE=NOT_FORCED`) because doing so against the real library
would be unsafe.

## Cache Cleanup

The code-level cleanup boundary removes only page/raw content cache and keeps
covers, metadata, sources, credentials, tags, history, completion state, and
custom cover parameters. The focused cleanup tests pass, including reader
lease/refcount behavior and the non-download/local no-op path. The seeded
Android fixture exercised page/raw deletion while retaining its cover, and the
subsequent over-install/reboot readback retained the seeded DB rows and vault.
`cmd package trim-caches 1G` was executed on API 36; because the fixture was
small the command evicted nothing and the readback stayed at sources=1,
metas=1, records=1, tags=4, book_tags=4, settings=26
(`android-cache-trim-final-retry25.log`). This is evidence that the boundary
remained intact, not a proof that the platform Settings UI actually evicts this
app.
The Android Settings **Clear cache** flow was then exercised on API 36 with a
durable marker under `files/RCH/data` and a disposable marker under
`cache/RCH/cache/page`. Before the tap, Settings showed an enabled button and
`Cache = 61.44 kB`; after the tap it showed a disabled button and `Cache = 0
byte`. The durable marker test returned exit 0 and the cache marker test returned
exit 1. UI dumps and the concise result are retained in
`android-clear-cache-ui-before-retry40.xml`,
`android-clear-cache-ui-after-retry40.xml`, and
`android-clear-cache-retry40.log` / `android-clear-cache-retry40-summary.txt`.
This closes the emulator Settings UI path;
OEM cleaner behavior is separately pending. The debug package and its seeded
markers were uninstalled after capture; no test package or staged update APK
remains on the emulator. The earlier retry39 UI dumps are superseded and are not
used as evidence.

## Long-strip

Flutter webtoon navigation and gesture regression tests pass. The valid APK
launches on `RCH_API_36`, but the discovered 90-image CBZ is not a high-
difference strip fixture and was not copied to the emulator. The required 50+
page Android smoke (ten aggressive paging/rotation/background/last-page cycles)
is therefore **NOT RUN**; OEM/real-device evidence remains pending.

## Package Installer

The Android `rch/updater` channel, FileProvider declaration, unknown-source
permission branch, and retry-preserving Dart state are present. The fake
platform update tests pass: concurrent downloads share one future, verified
packages are reused, SHA-256 mismatch is rejected, failed Windows/Android
handoffs preserve the package, and the progress view remains visible.

`flutter build windows` passed and produced
`app/build/windows/x64/runner/Release/RCH.exe` (84,480 bytes,
SHA-256 `66C46E015A001E5B7104155B1823BC31496E37CED861B060E224396583BA8D98`).
The valid APK was deployed with `adb install -r` only as a test baseline; this
is explicitly not Package Installer acceptance. A separate emulator handoff
below passed; remaining Windows UAC and Android Package Installer user/policy
flows—including user cancel, retry, unknown-source denial, bad/corrupt hash,
same-version, and downgrade handling—were not run.

A temporary Android integration harness copied a debug APK into the app's
external-files path, invoked `rch/updater`, and returned `launched=true`. An
`adb dumpsys activity` snapshot showed
`com.google.android.packageinstaller/.PackageInstallerActivity` as the resumed
activity. The harness, staged APK, and app package were removed after capture;
the handoff evidence is `android-package-installer-handoff-retry38.log` and
`android-package-installer-dumpsys-retry38.txt`. This proves the emulator
FileProvider-to-system-installer launch only; user cancel/retry, bad hash,
same-version/downgrade policy, and real Windows UAC remain pending.

## M8

The implementation remains Catalog-only/local-only. The Rust golden seed test
passes for 52 records, including the remote-only boundary that must not touch a
provider or downloader. No local, manually reviewed 100-book truth set was
available; therefore `M8_TRUTH = NOT COMPLETE` and exact accuracy, FP, FN,
unresolved counts, and representative failure cases are absent. Online smart
scraping remains a future plugin and is not a release dependency.

## Rust / Flutter Regression

| Exact command | Result | Evidence |
| --- | --- | --- |
| `cargo fmt --all -- --check` | FAIL (exit 1) | Existing mixed dirty/historical formatting diffs; final output captured in `cargo-fmt-final-retry26.log`; no Category A source changes this round. |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | FAIL (exit 101) | 64 lib and 69 lib-test errors (`ptr_arg`, `too_many_arguments`, `type_complexity`, `io_other_error`, etc.); final output captured in `cargo-clippy-final-retry27.log`; existing debt. |
| `cargo test --workspace --locked -j 1 -- --test-threads=1` | PASS | 275 lib tests passed, 0 failed, 2 ignored; `reader_l1_hit_deadlock` 1 passed; doc tests 0 (`cargo-test-final-retry23.log`). |
| `flutter analyze --no-pub` | PASS | No issues found (`flutter-analyze-final-retry34.log`). |
| `flutter test --no-pub` | PASS | 113 tests passed (`flutter-test-final-retry20.log`). |
| `dart run tool/startup_contract_check.dart` | PASS | `startup contract satisfied` (`startup-contract-final-retry21.log`). |
| `flutter build apk --debug` | PASS WITH TOOLCHAIN CONDITION (retry31) | VS Developer environment + JDK25 + process-local Kotlin incremental workaround; standard APK inspected, Rust `.so` present, hash recorded above. Default-shell retries exposed cross-drive/Cargokit failures and false-positive outer exits. |
| `RCH_API_36` boot/reboot/install/launch | PASS (emulator smoke) | API36 x86_64 booted and rebooted; standard APK installed with `adb install -r`; launch had no FATAL/`dlopen`, and seeded P0 readback remained intact. |
| Android authenticated OpenList worker (temporary proxy) | PASS (emulator smoke) | `android-openlist-app-worker-retry36.log`: real ~20/100/300 MiB CBZ download, page 0 decode, raw/hash verification, three cancellation points, sampled RSS peak deltas; proxy credential stayed in process memory. |
| Android Settings Clear cache | PASS (emulator UI smoke) | `android-clear-cache-retry40-summary.txt` plus before/after UI dumps: enabled 61.44 kB cache became disabled `0 byte`; durable marker retained and cache marker removed. OEM cleaner remains pending. |
| Android FileProvider to Package Installer handoff | PASS (emulator launch only) | `android-package-installer-handoff-retry38.log` returned `launched=true`; `android-package-installer-dumpsys-retry38.txt` shows `PackageInstallerActivity` resumed. User/policy outcomes remain pending. |
| `flutter build windows` | PASS | Release executable produced with current Dart changes; hash recorded above (`flutter-build-windows-final-retry22.log`). |
| `git diff --check` | FAIL (exit 2) | Trailing whitespace is confined to the user-modified `AGENTS.md`; line-ending warnings are also present (`git-diff-check-final-2-retry29.log`). |
| `git diff --check -- . ':(exclude)AGENTS.md'` | PASS | No whitespace errors outside `AGENTS.md` (`git-diff-check-code-only-final-2-retry29.log`). |

The full Flutter regression command passed all 113 tests, including update,
storage, vault, cleanup, webtoon, cover scheduling, cover-consent, and the new
empty-reference regression. No Category A fmt/Clippy debt was introduced in
this evidence-only round; Category B/C debt was not mass-formatted or
suppressed.

## Historical Debt

The worktree already contained broad Rust/Flutter/Android changes and Trellis
task/spec edits before this round. Strict fmt and Clippy failures span those
existing files and historical modules; resolving them would be a separate
change set and would pollute the current evidence boundary. The default shell
also lacks the Visual Studio host-link environment expected by Cargokit, and
Gradle/Kotlin has a cross-drive incremental-cache edge case; the successful
retry used process-local environment settings only. The full `git diff --check`
failure comes from trailing spaces in the newly supplied `AGENTS.md`, which is
preserved verbatim per the worktree-preservation rule.

## Pending External Validation

The following labels remain open and must not be converted to PASS without
fresh evidence:

```text
ANDROID_APK_BUILD=PASS_WITH_VSDEV_ENV (final retry31/32; standard APK inspected and launched)
ANDROID_EMULATOR=PASS_BOOT_REBOOT_INSTALL_LAUNCH (RCH_API_36)
P0_PERSISTENCE=PARTIAL_SEEDED_EMULATOR (fixture/over-install/reboot PASS; migration matrix pending)
KEYSTORE_RUNTIME=PASS_EMULATOR_SENTINEL_LIFECYCLE (real provider/OEM pending)
SYSTEM_CLEAR_CACHE=PASS_EMULATOR_SETTINGS_UI (durable marker retained; cache marker removed; OEM cleaner pending)
REAL_PROVIDER_AUTH=PASS (authenticated PROPFIND 207; credential not retained)
REAL_PROVIDER_RANGE=PASS (HEAD/single/multi/invalid on ~20/~100/~300 MiB)
REAL_PROVIDER_TRANSPORT_DOWNLOAD=PASS (serial + 2-segment hashes match)
REAL_PROVIDER_TRANSPORT_CANCEL=PASS (9 range-abort probes; `.part` removed)
DESKTOP_APP_WEBDAV_DOWNLOAD=PASS (real Rust/Flutter worker; 20/100/300 MiB)
DESKTOP_APP_WEBDAV_CANCEL=PASS (real Rust/Flutter worker; raw/.part cleanup)
ANDROID_PROVIDER_WORKER=PASS_EMULATOR_AUTH_PROXY (20/100/300 MiB; page read; RSS sample; 10/50/90% cancel)
REAL_PROVIDER_PERF_PENDING (direct credentialed product UI and real-device/OEM RSS)
REAL_PROVIDER_TRUNCATED_RESPONSE_PENDING
ANDROID_INSTALLER_HANDOFF=PASS_EMULATOR_FILEPROVIDER_TO_PACKAGE_INSTALLER (launch only; user confirmation/policy pending)
OEM_CLEANER_REAL_DEVICE_PENDING
REAL_DEVICE_LONG_STRIP_PENDING
REAL_DEVICE_INSTALLER_PENDING
M8_TRUTH=NOT COMPLETE
```

The next run should cover the remaining migration matrix (first JSON import,
valid WAL/SHM, corruption and interruption recovery), direct credential-vault
hydration through the product UI, and Package Installer user/policy outcomes. It
must also execute long-strip/OEM and Windows UAC checks and
complete the local 100-book M8 truth set. No third-party mirror or insecure TLS
bypass was enabled in this run.

## Git Boundary

Final state remains on `local-ai/cover-quark-debug` at `d112433` (`v0.5.7`).
The SDK/NDK and system image are installed outside the repository. The
temporary comic fixture, seeded emulator package/data (including the sentinel
vault), and all temporary integration probes were removed after evidence
capture (`android-fixture-cleanup-final-retry28.log`); the package used for the
later Settings Clear Cache capture was also uninstalled after retry40. The user
original and the external evidence logs are retained. The temporary auth proxy was also
stopped after the Android worker run and its script is outside the repository.
This round includes the minimal
`CredentialVault.kt` IV fix, the blank sync credential-reference normalization,
and its regression test; all remain uncommitted alongside the pre-existing
worktree edits. Existing Trellis artifacts, reports, the Feishu authorization
image, and `graphify-out/` were left untouched. This round created no commit,
tag, release, version bump, or push.

The closing `git status --short` check reports 63 tracked modified files and 29
untracked entries (this report, the Dart regression test, and the Android
integration regression are included). These are
the pre-existing dirty changes plus evidence artifacts and the two minimal
uncommitted fixes, not a clean-release boundary, and must be reviewed before any
future commit.

## Final Recommendation

**YELLOW**

**DO NOT RELEASE**

The valid APK, booted/rebooted emulator, seeded P0 readback, Keystore sentinel
lifecycle, authenticated OpenList transport matrix, Android emulator worker
smoke, Settings Clear Cache UI, and the FileProvider-to-Package-Installer launch move this round
materially forward, but the release-candidate gate is still incomplete. Keep
v0.5.7 at the current baseline until migration fault cases, direct
credential-vault/UI behavior, OEM cache cleaners, Package Installer user
outcomes, long-strip/OEM and Windows UAC checks, and a manually reviewed
100-book M8 truth set all have reproducible evidence.
