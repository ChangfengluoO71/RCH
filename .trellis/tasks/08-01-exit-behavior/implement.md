# Conditional Exit Prompt — Implementation Plan

> This task implements only the active-long-work prompt described by `prd.md` and `design.md`. The historical tray/three-mode proposal is deferred.

**Goal:** Warn before closing RCH when local work is active or durable Cloud115 monitoring will pause, while allowing a silent immediate exit when there is nothing to explain.

**Architecture:** A process-local `LongTaskRegistry` reports currently executing work through scoped leases. Existing domain managers remain state authorities; the root app composes their active leases with durable Cloud115 monitor rows and handles the Windows app-exit request.

**Tech Stack:** Flutter/Dart, existing task coordinators and stores, Flutter Windows app-exit lifecycle, existing Rust/SQLite Cloud115 monitor API.

**Spec:** `.trellis/tasks/08-01-exit-behavior/prd.md` and `.trellis/tasks/08-01-exit-behavior/design.md`.

## Global constraints

- Do not add a tray, Windows service, Task Scheduler integration, or background-running mode.
- Never claim local polling continues after RCH exits; accepted remote 115 BT jobs continue independently.
- Reuse existing cancellation/checkpoint/flush behavior and existing persistence; do not create a second task database.
- Preserve current P0/P1 gates and do not issue any live 115 write during tests.
- No commit, push, merge, rebase, tag, release, delete, or cleanup.

## Review focus

- A task that throws or is cancelled must release its registry lease in `finally`; test each outcome.
- Multiple jobs with the same user-facing label must remain distinct; test nested/same-label leases.
- A monitor waiting for its next poll is resumable but not currently executing; show it accurately.
- Exit preparation timeout/failure must not silently close and claim that state was saved; test a failing callback.
- A second close request while a dialog is open must not create duplicate dialogs or double-run callbacks; test repeated requests.

### Task 1: Inventory long-running owners

**Files:** Create `.trellis/tasks/08-01-exit-behavior/research/long-task-inventory.md`; inspect `app/lib/main.dart`, `app/lib/store/`, and relevant manager tests.

- [x] Enumerate operations that can outlive a short interaction: EH scan/Daily, Cloud115 batch/monitor, remote scans/session warm-up, source indexing, sync/automation, AI tasks, and existing backup/cache/update work.
- [x] Record each operation's owner, stop/checkpoint/recovery behavior, registration boundary, and user-facing label in `research/long-task-inventory.md`.
- [x] Remove duplicate nested registration candidates; the highest coordinator owning the operation's full lifetime is the sole registration owner.

### Task 2: Add the in-memory registry

**Files:** Create `app/lib/store/long_task_registry.dart`; create `app/test/long_task_registry_test.dart`.

**Interfaces:** Implement `LongTaskRegistry.register(...) -> LongTaskLease`, `LongTaskRegistry.update(lease, ...)`, `LongTaskRegistry.snapshot()`, and bounded `LongTaskRegistry.prepareForExit(snapshot, timeout)`. `LongTaskLease.dispose()` removes only its own registration. Snapshot fields are display-safe `kind`, `label`, `phase`, `startedAt`, and preparation capability; no task payload or credential fields.

- [x] Write tests for independent leases, same-label/nested leases, phase updates, disposal, error cleanup, stable snapshots, and timeout/failing exit callbacks.
- [ ] Capture a pre-implementation failing test run. (Not recorded; no claim is made for this TDD evidence.)
- [x] Implement the registry with identity-scoped leases and `try/finally` integration contract.
- [x] Re-run focused tests and `flutter analyze` on the new file.

### Task 3: Instrument existing task coordinators

**Files:** Modify the owning existing coordinators identified in Task 1, expected candidates include `app/lib/store/eh_subscription_store.dart`, `app/lib/store/eh_daily_coordinator.dart`, `app/lib/store/cloud115_bt_batch.dart`, `app/lib/store/cloud115_bt_daily_workflow.dart`, `app/lib/store/remote_scan_coordinator.dart`, `app/lib/store/automation_coordinator.dart`, `app/lib/store/ai_upscale_manager.dart`, and other verified long-operation owners only.

- [ ] Add a registration lifecycle assertion for every instrumented manager. Current focused coverage verifies the registry and selected remote scan/automation paths; per-manager coverage remains incomplete.
- [x] Instrument inventoried long operations at their lifetime-owning coordinator; nested automation/sync indexing does not create duplicate child leases.
- [x] Connect Cloud115 resumable-monitor summaries as resumable exit information, not as a fake active poll lease.
- [ ] Run focused tests for every modified coordinator and prove scheduling/cancellation/retry behavior is unchanged. Selected coordinator suites passed; exhaustive manager-by-manager proof remains pending.

### Task 4: Handle the app exit request

**Files:** Create `app/lib/ui/exit_work_dialog.dart`; modify `app/lib/main.dart` at the `RchApp` lifecycle boundary; add root lifecycle/widget tests.

**Interfaces:** Add a typed `ExitWorkSnapshot` composer that combines registry snapshots with existing persisted resumable-monitor summaries. The root app handles `WidgetsBindingObserver.didRequestAppExit`, serializes close requests, and returns `AppExitResponse.cancel` or `AppExitResponse.exit`.

- [ ] Exercise the root lifecycle's silent exit with no active or resumable work in a lifecycle test.
- [x] Test dialog task summaries; “Continue using RCH” cancels exit and leaves work running.
- [x] Test exit preparation callback success/failure and duplicate request gating; the remote continuation message is covered by widget tests.
- [x] Implement the single Windows lifecycle hook and dialog; non-Windows lifecycle remains unchanged.
- [x] Code-path review confirms the exit flow has no remote 115 stop/delete/write operation. No live endpoint was called.

### Task 5: Windows GUI and cross-layer verification

**Files:** Add/extend isolated Windows GUI test entry only if the repository's current test harness cannot render the root lifecycle safely; otherwise reuse the existing Windows GUI test facilities. Do not change production startup to support testing.

- [ ] Run a complete unit/widget suite for every instrumented manager and root app-exit lifecycle. Focused subsets passed; this full coverage remains pending.
- [x] Run `flutter analyze` on changed files and the relevant focused Flutter tests.
- [x] Build the standard Windows Debug target from the current workspace.
- [ ] In an isolated profile with fake active tasks and a fake resumable monitor, click the real window close button; verify both dialog choices, no duplicate dialog, and normal/narrow layout.
- [x] Focused tests use injected/mock state and perform no real EH or 115 network requests or production Cookie reads.
- [ ] Report GUI, test harness, and untested platform results separately. Do not claim app-closed monitoring or global shutdown durability.

## Execution record — 2026-10-14

- Flutter focused suite: 85 tests passed across the exit registry/dialog, Cloud115 monitor/batch, remote scan, automation, source index, update, sync credential, and cache-target suites. This includes the added “monitor state unreadable” fail-closed prompt regression.
- `flutter analyze` on 14 changed Dart files: no issues.
- Rust: `cargo fmt --check`, `cargo check --lib`, Cloud115 monitor tests (4), and bounded task-snapshot tests (2) passed.
- Windows Debug: `flutter build windows --debug --no-pub` passed; output `app/build/windows/x64/runner/Debug/RCH.exe`. The app was not launched.
- `git diff --check`: clean after removing one generated-binding trailing space. Git still reports line-ending conversion warnings for existing modified files.
- Real Windows close-button GUI flow, isolated production-entry launch, Android/macOS/Linux lifecycle behavior, and power-loss durability were not tested. Overall implementation remains partial until isolated GUI validation and full manager coverage are complete.
- P0 remains `P0_WEB_WRITE_AUTH_BLOCKED`; P0 real pass is `NO`. P1 remains `NOT_STARTED`. The historical 12 Rust parallel failures remain `BASELINE_UNRESOLVED`.
