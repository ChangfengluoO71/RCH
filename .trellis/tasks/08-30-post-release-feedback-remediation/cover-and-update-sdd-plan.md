# Cover and Update Combined Execution Plan

> **For agentic workers:** Execute one task at a time. Use test-first development, preserve unrelated working-tree changes, and never commit or revert another task's files.

**Goal:** Deliver bounded remote-cover behavior and a continuous, single-flight update download/install flow in one implementation batch while retaining independent rollback boundaries.

**Architecture:** The cover work introduces an explicit cache-only/remote-partial-only contract at the Rust/Flutter boundary, then layers a setting-aware, deduplicated visible-card scheduler on top. Reader priority and custom-cover full-download consent use the same safe-read boundary. The update work remains a separate Dart state-machine/UI path with an injectable installation handoff.

**Tech Stack:** Flutter/Dart, Flutter Rust Bridge, Rust, Tokio/reqwest provider clients, Flutter unit/widget tests, Rust unit tests.

**Specs:**
- `.trellis/tasks/08-02-cover-loading-perf/prd.md`
- `.trellis/tasks/08-02-cover-loading-perf/design.md`
- `.trellis/tasks/08-02-cover-loading-perf/implement.md`
- `.trellis/tasks/08-30-update-download-install-handoff/prd.md`
- `.trellis/tasks/08-30-update-download-install-handoff/design.md`
- `.trellis/tasks/08-30-update-download-install-handoff/implement.md`

## Global Constraints

- No production code before a focused failing regression test exists and has been observed to fail for the intended missing behavior.
- Never consume a `200`, `416`, missing, or mismatched `Content-Range` response body for a remote poster cover, and never invoke whole-book raw download from poster-wall flow.
- Existing local/memory/cover-disk/raw-cache hits remain usable when remote cover fetch is disabled.
- Do not log or commit credentials, full download URLs, or real user paths.
- Keep cover and update code/test changes separate; do not create a shared state machine or a combined irreversible commit.
- Rust API changes require RCH to be closed before `app/codegen.ps1`; generated bindings must be reviewed with the source change.
- The existing parallel Rust baseline has a known cache-root test race; use a serial `cargo test -- --test-threads=1` as the trusted full-suite baseline until that unrelated test isolation issue is separately fixed.

## Task 1: Settings Contract and Cover Test Harness

**Ownership:** `app/lib/store/models.dart`, `app/lib/ui/home_page.dart`, and `app/test/app_settings_parse_test.dart` only.

**Inputs:** Current `AppSettings` JSON conventions and the existing `ComicCover` cache-first behavior.

**Produces:** Persisted `remoteCoverFetchEnabled` defaulting to `true`, a global remote-source setting control, and regression tests proving old JSON defaults to enabled and round-trips without affecting existing settings. Task 3 owns the runtime “disabled means cache-only” behavior after Task 2 makes remote fetch safe.

- [x] Write a failing settings round-trip/default regression with literal expected values.
- [x] Run that test and confirm failure is caused by the missing field/behavior.
- [x] Add the minimal model and setting UI code without enabling unsafe remote access yet.
- [x] Run the focused tests and `flutter analyze --no-pub`.

## Task 2: Rust Safe Cover Fetch Contract

**Ownership:** `app/rust/src/api/source.rs`, directly affected source adapters under `app/rust/src/source/`, focused Rust tests, and generated FRB bindings only after code generation.

**Inputs:** Task 1 setting intent and the cover policy in the cover task design.

**Produces:** Typed cache-only / remote-partial-only cover outcomes that accept only verified partial reads, report `rangeUnsupported` without a whole-book fallback, and retain cache hits.

- [x] Add focused failing Rust tests for `206 + valid Content-Range`, `200`, `416`, and invalid/missing `Content-Range`; assert each non-206 branch neither consumes a full body nor calls raw-cache download.
- [x] Run the focused tests and confirm they fail because the safe policy/result is absent.
- [x] Implement the smallest shared policy/result boundary and adapt provider cover paths without changing user-initiated book-open strategy semantics.
- [x] Re-run focused Rust tests, then the serial Rust suite. Before API generation, verify the RCH process is not running; run `app/codegen.ps1` only if it is safe.

## Task 3: Visible Cover Scheduler and Degradation UX

**Ownership:** `app/lib/ui/comic_cover.dart`, the source-browser/container UI that owns aggregated notices, focused Flutter tests, and only the Task 2 FRB APIs.

**Inputs:** Task 1 setting and Task 2 typed results.

**Produces:** Stable logical cover cache identity, in-flight deduplication, cache-only behavior while disabled, cancellation of not-yet-started off-screen work, bounded retries, and one non-blocking range-unavailable notice per source browsing session.

- [x] Write focused failing tests for duplicate-card coalescing, disabled cache-only behavior, and one-per-source degradation reporting.
- [x] Confirm they fail against the current FIFO queue/remote guard.
- [x] Implement the minimal scheduler and UI wiring; preserve local cache-first rendering and existing placeholders.
- [x] Run focused Flutter tests, `flutter analyze --no-pub`, and Task 2 focused Rust tests.

## Task 4: Reader Priority and Custom-Cover Consent

**Ownership:** `app/rust/src/reader.rs`, related safe-cover API code from Task 2 only if needed, `app/lib/ui/cover_editor_page.dart` (or its actual owner), and focused Rust/Flutter tests.

**Inputs:** Task 2 safe cover policy and Task 3 scheduler behavior.

**Produces:** Request budget ordering current page > prefetch > cover without claiming started blocking I/O is cancellable; custom-cover flow only offers whole-book download after a user confirmation when partial reads are unavailable.

- [x] Write focused failing tests for foreground priority and for a rejected custom-cover whole-book download producing zero download work.
- [x] Confirm failure against current unbounded prefetch and `auto` open-path behavior.
- [x] Implement the smallest priority gate and typed `NeedsWholeBookDownload`/confirmation flow; do not rewrite providers to async cancellation.
- [x] Run focused Rust/Flutter tests, serial Rust suite, Flutter analysis, and required code generation checks.

## Task 5: Update Download and Install Handoff

**Ownership:** `app/lib/store/update_manager.dart`, `app/lib/ui/update_panel.dart`, any narrowly scoped new progress view/platform-launcher abstraction, and `app/test/update_manager_test.dart` plus focused UI tests.

**Inputs:** Existing `UpdateManager` state and platform channel behavior; no cover-task APIs or files.

**Produces:** A single active download per update, immediate visible progress from the update dialog, one user-authorized automatic handoff to Windows installer or Android system installer, and retryable states after handoff failure.

- [x] Write failing tests for duplicate download coalescing and exactly-once post-download handoff, using a fake only at the Windows/Android boundary.
- [x] Confirm failure against current concurrent `download()` and dialog-close behavior.
- [x] Implement the minimal state/launcher abstraction and continuous progress UI without changing “稍后”, mirror fallback, or Android system-confirmation semantics.
- [x] Run focused update tests, full Flutter tests, and `flutter analyze --no-pub`; perform Windows/Android handoff smoke checks when the relevant platform is available.

## Final Batch Gate

- [x] Run `cargo test -- --test-threads=1`, `flutter test --no-pub`, and `flutter analyze --no-pub`.
- [x] Review cover and update diffs separately for scope, privacy, rollback, and generated-binding consistency.
- [x] Record manual source and platform smoke-test availability without claiming unperformed real-account/device validation.
