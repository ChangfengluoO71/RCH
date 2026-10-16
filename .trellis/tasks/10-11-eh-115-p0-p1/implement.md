# Implementation Plan — P0 Only Until Gate Passes

## Ordered work

1. Reconfirm baseline, current worktree, and remote HEAD; complete code/docs and proxy behavior audit.
2. Remove all writes, add-task candidate probing, and task-clear calls from the legacy 115 probe. Keep only documented read-only network/auth/folder checks, or make unsupported probes clearly unavailable.
3. Implement the minimum EH network policy/config validation and bounded main-site/Tracker diagnostics. Keep existing filter behavior and default settings compatible.
4. Add read-only 115 direct diagnostics only if they can reuse an already-held session access token and existing gate without changing the reader client's route or rotating tokens. Otherwise record the exact blocker.
5. Run focused Rust and Flutter checks plus fault-injection tests for proxy validation, route isolation, challenge/status classification, credential redaction, and probe write absence.
6. Review official 115 documentation/SDK and local authorization/resources. Execute at most one actual write chain and only after the official API contract, active session, target folder, and authorized test Torrent are verified. Never try candidate write endpoints.
7. Classify P0 with evidence. If P0_REAL_PASS is not fully met, stop. Do not implement P1 in this task.

## Validation commands

- `cargo tree --locked --offline -e features -i reqwest`
- Focused Rust tests for changed EH/network and probe code, followed by repository-required Rust checks if P0 remains eligible.
- Focused Flutter tests/analyzer for any settings UI change.
- Static audit: `rg -n '\.post\(|offline/clear|add_task' app/rust/examples/cloud115_offline_probe.rs`; the safe probe must contain no task-creation or cleanup request.
- `git status --short` and diff review; no commit or destructive cleanup.

## Stop conditions

- Any requirement to send an undocumented or guessed 115 write request.
- No current authorized session, no authorized test Torrent, or no route to safely identify the target folder.
- WAF/challenge, unknown write result, token-rotation race, or impact to the existing reader path.
- Any manifest/history loss or test regression.

## Current execution record

- Baseline and remote state were rechecked at task start; the branch was clean before this task and already contained three commits beyond the requested `origin/master` base.
- EH proxy modes, validation, SOCKS support, host/Tracker-root probe metadata, and a settings panel were implemented. Tracker root checks are explicitly transport-only; this does not validate Torrent bytes. Existing 115 reader clients were not changed.
- The unsafe 115 probe was retired without network activity. Official public 115 materials found no published BT upload/submission contract. The local Torrent folder was inspected for names/sizes only; no specific authorized test sample was identified.
- No live RCH process or existing in-memory 115 session was available. No token refresh, authenticated account probe, or write request was made.
- P0 is `P0_BLOCKED`; P1 remains unstarted. See `research/p0-gate.md` for evidence and constraints.

## Approved follow-up: Daily EH-to-115 workflow and settings layout (2026-10-13)

The user approved implementing the complete Windows Daily orchestration while
preserving the P0 real-write gate. This follow-up does not change the historical
P0 result and does not authorize assistant-issued real 115 writes.

### Scope

1. Keep the persisted Daily switch as the single explicit opt-in. A Daily run
   performs EH collection, refreshes the checked local manifest, then invokes
   the existing sequential 115 batch processor with the saved Cookie source,
   target folder, and cleanup preference. It must reuse the currently cached
   RCH Cookie session and never prompt for a second login.
2. Keep automatic 115 writes behind a release-controlled `P0_REAL_PASS`
   capability gate. The gate remains closed while the evidence in
   `research/p0-gate.md` is blocked. Do not silently downgrade an enabled
   full-workflow request to EH-only automation: the UI/status must expose the
   gate as a partial/blocked cloud stage, and the EH scan must not trigger any
   115 request while the gate is closed.
3. Use the existing credential-free batch ledger for per-infohash idempotency.
   An unknown write result remains unknown, is not replayed, and does not stop
   independent later Torrents. Account/session identity mismatch, WAF, auth,
   and explicit rate limiting remain batch-wide stop conditions. Local Torrent
   cleanup remains opt-in and requires positive remote task proof.
4. Restructure Settings: top-level `EH 订阅与自动化`; first content is the
   Daily full-workflow switch and latest-run summary; local saved-seed handoff
   follows; EH filters/output and 115 source/folder/cleanup controls are in a
   collapsed Advanced Options group. EH proxy and EH diagnostics move to their
   own top-level `网络` category.
5. Update SPEC §10 minimally to distinguish the local-only EH collector from
   the independent, P0-gated Cloud115 adapter. Do not modify the normal 115
   reader path or the saved-seed manifest schema.

### Implementation sequence

1. Add failing tests for Daily run summary round-tripping, the closed P0 gate
   causing zero 115 callbacks, and successful EH collection invoking exactly
   one injected cloud stage with the refreshed checked manifest.
2. Add the Daily stage result fields and injected runner to the EH store; run
   the cloud stage only after successful EH collection and local manifest
   refresh, while retaining the existing single-flight/manual-vs-Daily lock.
3. Add the production Daily runner over the existing ledger, selected Cookie
   source, cached session, H0-checked manifest, and batch processor. Keep its
   write capability gate false until P0 evidence is independently changed.
4. Add UI regressions for the top-level labels, Daily-first ordering, advanced
   collapse, and the separate Network category; then reorganize the widgets.
5. Add the minimal SPEC §10 clarification and review all changed files and the
   pre-existing dirty paths without staging or committing anything.
6. Run the focused Flutter tests, analyzer, relevant Rust checks only if Rust
   changes are needed, and produce a standard Windows Debug build if available.

### Acceptance limits

- P0 remains `P0_WEB_WRITE_AUTH_BLOCKED`; `P0_REAL_PASS` is not inferred from
  mocks or this integration.
- Daily 115 writes issue zero requests while the P0 gate is closed.
- No user Cookie/Token enters settings, the batch ledger, logs, or Daily state.
- No commit, push, merge, rebase, tag, release, delete, or cleanup.

## Follow-up TODO

- [ ] Fix the 115 source status indicator: the user confirms the existing session is connected, while the UI says `需连接`. Do not use this label as evidence that authentication failed. Derive the displayed state from a successful existing-session request and add regression coverage for connected, disconnected, and expired sessions. This is tracked for follow-up and is not implemented as part of the current P0-B retry.

## Daily EH-to-115 workflow integration checkpoint — 2026-10-13

- The EH settings category is `EH 订阅与自动化`. Its first card contains the opt-in `自动每日完整工作流` switch, local-date scheduling state, last-run summary, and explicit P0 gate. Saved local Torrents remain the second section; EH rules/output and 115 Cookie source/target/cleanup/manual batch remain under collapsed `高级选项`. EH proxy controls and diagnostics are in the independent top-level `网络` category.
- After a successful Daily EH scan, the store refreshes the checked local manifest and passes only H0-valid entries to the existing sequential Cloud115 batch processor. Manual EH scans do not auto-write. The release capability gate remains `false` because P0 is still blocked; with the gate closed, neither the EH Daily request nor any 115 callback runs.
- Per-item unknown upload/submit outcomes remain in the credential-free account + Infohash + target-folder ledger, are never blindly replayed, and do not prevent processing independent later items. Account/session mismatch, auth, WAF, and explicit rate limiting remain whole-batch stops. Cleanup remains opt-in and requires a positive same-account remote task check.
- Daily `提交已处理` means only that this round's BT submission handling ended; it explicitly does not claim cloud download completion or RCH source discovery. No task-status polling or reader scan was added to the Daily submission stage.
- Verification: focused Flutter suite passed 58 tests; changed Dart files passed targeted `dart analyze`; focused Rust SSP encrypted-body and same-account local-cleanup tests passed 1/1 each. No real 115 request or Cookie access occurred.
- P0 remains `P0_WEB_WRITE_AUTH_BLOCKED`; `P0_REAL_PASS=NO`; P1 production auto-upload remains P0-gated. The 12 historical parallel Rust failures remain `BASELINE_UNRESOLVED`.
- Standard Windows Debug PID 15036 is still open beside the older preview PID 32292. Both were left untouched. The Debug asset bundle predates the final copy/label text edits; the build output was not overwritten while that process ran.

Commit: NO / Push: NO / Merge: NO / Rebase: NO / Tag: NO / Release: NO / Delete: NO / Cleanup: NO.

## v0.6.6 filter policy update — 2026-10-16

- The EH filter now enforces a minimum rating of `4.4` and rejects galleries posted within or exactly three days before the scan time. Missing/invalid posting time is skipped because the age threshold cannot be proven.
- The trial download ladder is applied to fresh default configurations. The one-time legacy policy migration preserves an existing user's `age_tiers` while raising an older rating below `4.4`; it only uses defaults if no valid tier list was saved.
- Regression coverage: `app/test/eh_subscription_filter_policy_test.dart` verifies that an existing ladder survives the policy migration.
- No real EH request or 115 write was performed for this update. `P0_WEB_WRITE_AUTH_BLOCKED`, `P0_REAL_PASS=NO`, and `P1=NOT_STARTED` remain unchanged.

## Approved follow-up plan: persistent Cloud115 task monitoring

**Goal:** Resume bounded, read-only monitoring of RCH-attributed 115 BT tasks across app restarts and show trustworthy batch/task counts without authorizing any new write.

**Architecture:** Keep the append-only batch JSONL ledger authoritative for upload/submit idempotency. Add operational SQLite projections and state-transition history; poll one bounded task-list snapshot per account through the existing Cookie session and request gates. The exit-prompt task owns process-lifetime task aggregation and exit confirmation.

**Tech stack:** Existing Rust SQLite/FRB APIs, Dart stores and Flutter UI, existing Cloud115 Cookie session, existing JSONL ledger, Flutter Windows app-exit lifecycle.

**Spec:** `prd.md` and `design.md` in this task; `prd.md`, `design.md`, and `implement.md` in `.trellis/tasks/08-01-exit-behavior/` for exit behavior.

### Global constraints

- P0 remains `P0_WEB_WRITE_AUTH_BLOCKED`; this follow-up does not authorize real writes or unlock automatic 115 submission.
- No second login/session, no separate refresh-token owner, no credential/raw-response persistence, and no live API write tests.
- Poll only RCH-attributed `(account, Infohash, target folder)` ledger entries; one bounded snapshot per account per cycle.
- Unknown pagination/status/folder evidence never means success, duplicate eligible for cleanup, or failure.
- App close pauses local work and polling; remote 115 downloads continue. No tray, service, or background process.
- Keep all unrelated dirty work. No commit, push, merge, rebase, tag, release, delete, or cleanup.
- The proposed SPEC §10.5 operational-SQLite boundary is not yet applied. Do not add the migration/table until that exact storage-boundary change is authorized.

### Review focus

- Unstable/over-budget task-list pagination: absent tasks remain unknown; test incomplete and changing snapshots.
- Unrecognized or localized task status: map to unknown; test arbitrary numeric/text values.
- Hash exists under an unverified target folder: retain local seed and do not count target completion; test mismatched/unknown folder evidence.
- Crash between ledger append and DB projection: startup reconciliation rebuilds monitor rows without writes; test restart and duplicate upsert.
- Two RCH processes poll the same account: one short DB lease wins; losing process performs no request; test with independent processes against one temp DB.

### Task 1: Operational monitor storage

**Files:** `app/rust/src/db/mod.rs`, `app/rust/src/api/db.rs`, generated `app/lib/src/rust/api/db.dart` and FRB files, plus a focused Rust DB test.

**Interfaces:** Add idempotent migrations for `cloud115_bt_monitor_run`, `cloud115_bt_task_monitor`, and `cloud115_bt_task_event`. Expose typed DTOs and CRUD operations to upsert ledger-derived entries, list due entries, acquire/release an account lease, record normalized poll observations, and reload current counts. Keep each transaction local to SQLite; never pass a connection/transaction across an await or HTTP call.

- [ ] Confirm explicit approval of the proposed SPEC §10.5 additive wording in `design.md`; if not approved, stop before the migration and keep the implementation unstarted.
- [ ] Write tests for migration idempotence, unique account/hash/folder identity, ordered meaningful state events, current state reload, lease exclusion/expiry, and no library schema-marker bump.
- [ ] Run the focused Rust DB tests against an isolated temporary database and confirm they fail for missing migration/API behavior.
- [ ] Implement the migration and typed DB/FRB operations using the project's current DB initialization path.
- [ ] Re-run the focused tests; verify a simulated crash after ledger append is repaired by idempotent upsert and never creates a batch write intent.

### Task 2: Bounded account-level task snapshot

**Files:** `app/rust/src/source/cloud115_offline.rs`, `app/rust/src/api/source.rs`, `app/lib/src/rust/api/source.dart`, generated FRB files, and focused Cloud115 offline-module tests.

**Interfaces:** Add a typed snapshot result that carries page completeness/instability and normalized task rows (Infohash, safely available task ID, folder evidence, raw status code only if contract-validated, progress). Implement an account-level bounded listing API; retain the existing per-Infohash UI query until callers migrate. Unknown status/folder fields remain nullable/unknown.

- [ ] Add mocked page tests for stable complete listing, empty list, 300-row budget exhaustion, page-count mutation, malformed rows, unknown status, and multiple tracked hashes matched from one snapshot.
- [ ] Implement a single bounded page walk under the current request gate, preserving existing page and retry limits; do not increase the existing 10 × 30 budget.
- [ ] Add conservative status normalization: only verified mappings become queued/downloading/completed/failed; every other value is unknown.
- [ ] Run the focused Rust offline module tests and `cargo check --lib`; confirm no write endpoint is reachable from the snapshot API.

### Task 3: Dart monitor coordinator and restart recovery

**Files:** Create `app/lib/store/cloud115_bt_monitor.dart`; modify `app/lib/store/cloud115_bt_batch.dart`, `app/lib/store/cloud115_bt_daily_workflow.dart`, and `app/lib/store/cloud115_session.dart`; add `app/test/cloud115_bt_monitor_test.dart`.

**Interfaces:** `Cloud115BtMonitor` reconciles JSONL ledger entries into SQLite, schedules due account snapshots while the app is active, persists `lastCheckedAt`/`nextCheckAt`/classification, and emits derived batch/task summaries. Inject clock, DB bridge, account-session resolver, and read-only snapshot function for tests. Session resolution must return the existing source session or WAIT_AUTH; it must not create a login or refresh owner.

- [ ] Test reconciliation after ledger-only crash, restart with due and future checks, 1/5/15/30-minute capped backoff, Retry-After cap, no rows means no timer, no per-hash request fan-out, and status transitions append events only when meaningful.
- [ ] Test auth expiry, 405/WAF, challenge, 429, offline, partial pagination, and unknown status as separate persisted outcomes; verify no upload/submit function is invoked by any monitor path.
- [ ] Integrate the monitor after manual batch completion and on app startup after DB/source initialization; only attach to gated Daily execution where the existing `P0_REAL_PASS` gate already allows the batch.
- [ ] Test app close/relaunch, unknown submit reconciliation, account/source change, and two independent processes contending for an account lease.

### Task 4: Batch summary UI

**Files:** `app/lib/ui/eh_subscription_panel.dart`, focused widget tests under `app/test/`.

- [ ] Add injected-state widget tests for parsed, accepted, duplicate, local cleanup, queued/downloading, completed, failed, and unknown counts, plus long names/error text.
- [ ] Render ledger and monitoring states separately. Label “accepted”, “download completed”, and “RCH library discovered” separately; unknown/missing destination evidence never renders completed.
- [ ] Verify polling produces no EH scan or 115 write and does not change existing cleanup opt-in semantics.

### Task 5: Exit prompt and task inventory

**Files:** See `.trellis/tasks/08-01-exit-behavior/implement.md`. Keep the registry API and dialog there; register Cloud115 monitoring as a resumable local observer, not as a remote worker.

- [ ] Complete the exit-task acceptance criteria before wiring every domain manager; no active tasks must retain the existing immediate exit behavior.

### Task 6: Cross-layer verification and gate report

**Files:** `docs/project/SPEC.md` only if the accepted design needs a precise additive clarification; task research/gate records for evidence.

- [ ] Run focused Rust tests, relevant Flutter tests, Dart analysis for changed files, Rust formatting/checks, FRB generation/check, and standard Windows Debug build.
- [ ] If Rust API signatures changed, regenerate bindings with the repository script `pwsh -File app/codegen.ps1` while RCH is closed, then review generated diffs and rebuild the standard Windows Debug target.
- [ ] Inspect generated FRB changes and prove no unrelated API churn.
- [ ] Keep Mock, FFI, GUI, and real network results distinct. This plan has no live 115 writes; real status/folder mappings stay unverified until separately authorized evidence exists.
- [ ] Report the exact remaining P0/P1 gates and any unknown status/task-folder semantics. Do not mark P0/P1 passed from local mocks.

## EH live progress and compact workflow UI — 2026-10-13

- Root cause of the apparent stall: Rust published progress mainly after search pages and successful Torrent saves. Metadata batches, filter-only candidates, and in-flight Tracker requests left the last counters unchanged; Flutter polls every 600 ms but could only render that stale snapshot. On scan error/cancel, the store also replaced the snapshot with a zero-valued failed state.
- Rust progress now reports the active search page, metadata batch, candidate index/title, Torrent probe/download/reuse stage, and activity start time. The UI renders stage counters and elapsed time from the existing poll loop.
- A scan error now fetches the final Rust snapshot where available and preserves counters. Valid Torrent bytes are Infohash-checked, an existing matching file is reused, and each validated item is added to the manifest before the next request. Cancellation and later network failures therefore keep earlier results deduplicated and visible.
- The first EH automation card is condensed around the Daily opt-in/status, one manual full-workflow button, configurable scan pages, stop action, and live progress. The foreground card also mirrors the current 115 batch item/stage after EH completes. The local seed handoff follows it. Detailed EH filters/output and saved 115 source/folder/cleanup/batch controls remain in Advanced Options; Network remains separate.
- Manual runs are not limited by the automatic local-date claim. Automatic runs remain at most once per local date. The Daily opt-in can be saved while P0 is blocked, but the coordinator remains fail-closed and starts neither EH automation nor 115 writes until the gate is open.
- No real EH scan or 115 write was started. P0 remains `P0_WEB_WRITE_AUTH_BLOCKED`, `P0_REAL_PASS=NO`; P1 automatic cloud writes remain gated. Verification used Rust `cargo check`, targeted Dart analysis, and a standard Windows Debug build; no tests were run in this follow-up.
- The final standard Debug app is running as PID 33952 from `app/build/windows/x64/runner/Debug/RCH.exe`; the Flutter kernel bundle timestamp is 2026-10-13 13:04:47 and the Rust library artifact timestamp is 2026-10-13 13:01:53.

Commit: NO / Push: NO / Merge: NO / Rebase: NO / Tag: NO / Release: NO / Delete: NO / Cleanup: NO.

## Daily EH-to-115 workflow integration checkpoint — 2026-10-13

- The EH settings category is `EH 订阅与自动化`. Its first card contains the opt-in `自动每日完整工作流` switch, local-date scheduling state, last-run summary, and explicit P0 gate. Saved local Torrents remain the second section; EH rules/output and 115 Cookie source/target/cleanup/manual batch remain under collapsed `高级选项`. EH proxy controls and diagnostics are in the independent top-level `网络` category.
- After a successful Daily EH scan, the store refreshes the checked local manifest and passes only H0-valid entries to the existing sequential Cloud115 batch processor. Manual EH scans do not auto-write. The release capability gate remains `false` because P0 is still blocked; with the gate closed, neither the EH Daily request nor any 115 callback runs.
- Per-item unknown upload/submit outcomes remain in the credential-free account + Infohash + target-folder ledger, are never blindly replayed, and do not prevent processing independent later items. Account/session mismatch, auth, WAF, and explicit rate limiting remain whole-batch stops. Cleanup remains opt-in and requires a positive same-account remote task check.
- Daily `提交已处理` means only that this round's BT submission handling ended; it explicitly does not claim cloud download completion or RCH source discovery. No task-status polling or reader scan was added to the Daily submission stage.
- Verification: focused Flutter suite passed 58 tests; changed Dart files passed targeted `dart analyze`; focused Rust SSP encrypted-body and same-account local-cleanup tests passed 1/1 each. No real 115 request or Cookie access occurred.
- P0 remains `P0_WEB_WRITE_AUTH_BLOCKED`; `P0_REAL_PASS=NO`; P1 production auto-upload remains P0-gated. The 12 historical parallel Rust failures remain `BASELINE_UNRESOLVED`.
- Standard Windows Debug PID 15036 is still open beside the older preview PID 32292. Both were left untouched. The Debug asset bundle predates the final copy/label text edits; the build output was not overwritten while that process ran.

Commit: NO / Push: NO / Merge: NO / Rebase: NO / Tag: NO / Release: NO / Delete: NO / Cleanup: NO.

## Batch continuation, P0 toggle gate, and Windows Debug verification — 2026-10-13

- The manual full-workflow button now exposes the current phase (`扫描中…` while EH is active, then the foreground workflow/batch progress). Manual runs remain independent of the automatic once-per-local-date claim.
- The Daily full-workflow switch is disabled while P0 is closed, and `EhSubscriptionStore.setDailyEnabled(true)` independently rejects the same transition. The UI explains that manual EH checks remain available. Existing enabled state remains fail-closed in the coordinator; disabling remains possible.
- The local seed handoff stays limited to the newest saved Torrent. Individual safe/unknown batch outcomes continue to later independent items; WAF, authentication/access, account mismatch, and explicit rate limiting remain batch-wide stops. A confirmed remote duplicate whose local cleanup failed remains recoverable through the ledger.
- Verification: full Flutter suite 328 passed. `flutter analyze --no-pub` reported no errors or warnings, but exited 1 for 10 existing color deprecation infos in `app_theme.dart` and `app_theme_mica_test.dart`. Rust BT offline module: 29 passed; `cargo check --lib` and scoped `rustfmt --check` passed. Full default-parallel `cargo test --lib`: 519 passed, 18 failed, 4 ignored. The 12 previously recorded shared-state/concurrency failures remain `BASELINE_UNRESOLVED`; six further cache/MOBI failures appeared in this parallel run, all in modules untouched by this change, and are not claimed as baseline or fixed.
- Standard Windows Debug build succeeded at `app/build/windows/x64/runner/Debug/RCH.exe`. The app was not launched: production startup warms existing remote-source sessions, which would perform real network work outside this verification boundary. No real EH request or 115 endpoint was called.
- Gate remains `P0_WEB_WRITE_AUTH_BLOCKED`; `P0_REAL_PASS=NO`; automatic 115 submission remains gated. No commit or remote Git operation was performed.

Commit: NO / Push: NO / Merge: NO / Rebase: NO / Tag: NO / Release: NO / Delete: NO / Cleanup: NO.
