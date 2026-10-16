# Conditional Exit Prompt for Long-Running Work — Design

## Goal and boundary

Prompt only when RCH has work that is currently executing or a durable local monitor that will resume later. The prompt is a lifecycle guard and status summary, not a task manager. It does not add tray behavior, a service, new scheduling, or task persistence outside the existing domain stores.

The implementation covers the Windows desktop lifecycle first. It must preserve the existing exit path on platforms without the Windows behavior, and it must not alter the EH/115 P0 gate.

## Sources of truth

- `LongTaskRegistry` is process-local and contains leases for work currently executing in this RCH process. It has no disk writes and no status retry policy.
- Each manager remains the authority for its own durable/in-memory task state: `EhSubscriptionStore`/`EhDailyCoordinator`, `Cloud115BtBatchProcessor`/`Cloud115BtMonitor`, `RemoteScanCoordinator`, `AutomationCoordinator`/sync, `AiUpscaleManager`, and any additional long-running managers found by the implementation inventory.
- Cloud115 monitor rows are read from the monitor database as resumable local polling work. They do not claim that the remote download itself is executing in RCH.
- The dialog combines snapshots from those sources at close request time. It must not duplicate credentials, task payloads, or mutable domain state.

## Proposed interfaces

`LongTaskRegistry` in `app/lib/store/long_task_registry.dart`:

- `LongTaskLease register({required String kind, required String label, required String phase, bool resumable = false, Future<void> Function()? prepareForExit})`
- `void update(LongTaskLease lease, {String? phase, String? detail})`
- `List<LongTaskSnapshot> snapshot()`
- `Future<void> prepareForExit(List<LongTaskSnapshot> tasks, {required Duration timeout})`

Each registration returns an identity-scoped disposable lease. Updating or disposing a lease affects only that registration, so nested operations and repeated task labels do not erase one another. The registry stores only display-safe summaries, start time, phase, and the optional existing pause/flush callback. It never stores secrets or full work inputs.

`ExitWorkSnapshot` is composed at the root app boundary from registry snapshots and a typed query of resumable monitor counts. It includes display text, whether local work is active, and whether remote 115 jobs continue. It does not expose raw DB rows or credentials to the UI.

## Flow

1. A long operation registers a lease at its owning coordinator's start boundary, updates the phase when meaningful, and disposes it in `finally`.
2. The root Windows app state handles `WidgetsBindingObserver.didRequestAppExit`. If another exit dialog is already open, it does not open a second one.
3. If no active lease and no resumable Cloud115 monitor row exists, return `AppExitResponse.exit` without a dialog.
4. Otherwise show one dialog with task names/phases and the fixed distinction: local work and status polling stop when RCH closes; already accepted 115 cloud jobs continue remotely and are reconciled after the next launch.
5. “Continue using RCH” returns `AppExitResponse.cancel`; no task state changes.
6. “Exit and pause local work” invokes only the registered existing cancellation/checkpoint/flush callbacks, waits for them with a bounded timeout, preserves monitor and batch-ledger records, then returns exit. It never cancels/deletes a remote 115 job. If a callback fails or times out, keep the prompt open with that task identified and let the user choose to stay or retry the exit preparation; do not claim a clean pause.

The Windows Flutter engine's app-exit request hook is the only close interception point. Do not add a second native window-close protocol or intercept unrelated process shutdown. Sudden power loss remains outside the guarantee.

## Task inventory and registration ownership

Before adding instrumentation, enumerate all existing operations that can materially outlive a short UI action. At minimum inspect:

- EH foreground scan and Daily collection
- Cloud115 batch submit and the new read-only monitor cycle/resumable monitor set
- remote-source scans and cover work
- sync/automation cycles
- AI upscale queue/running tasks
- long imports/exports, cache moves, and update operations if they already run asynchronously

Register at the highest existing coordinator that owns the operation's full lifetime; do not register every nested HTTP request. Use a stable task kind and short UI label. If a domain already exposes a durable resumable state, reuse it; do not add new persistence just to make the prompt aware of it. A monitor waiting for its next due time counts as resumable, not as a currently executing poll.

## Failure and compatibility behavior

- Registry cleanup is in `finally` so normal errors/cancellation do not leave phantom active tasks.
- If a source cannot provide a safe pause/flush callback, display that limitation and keep “Continue using RCH” available; do not report that work is resumable unless its existing store proves it.
- A Cloud115 status poll is local work; the remote BT task is independent. Exiting stops only the poller and leaves SQLite due state for restart.
- No active/resumable tasks means no extra prompt or delay.
- On non-Windows targets, preserve current platform lifecycle unless the relevant Flutter platform already supports the same app-exit callback and is separately tested.

## Test strategy

- Unit tests: independent lease lifecycle, nested/same-label leases, phase updates, exception cleanup, registry snapshot, resumable monitor aggregation, and prepare-for-exit timeout/failure.
- Widget/lifecycle tests: no-work exits silently; active work shows one prompt; stay cancels exit and leaves leases intact; exit runs pause/flush callbacks; failed preparation remains visible; remote-running copy is accurate; a second close request does not duplicate the dialog.
- Domain tests: every inventoried manager registers exactly one lease around its owned operation and releases it on success/error/cancel.
- Windows GUI acceptance: isolated app profile, controlled fake tasks, actual close-button interaction, both dialog choices, and normal/narrow layout. No production user profile or real EH/115 network.

## Out of scope

Tray icon, keep-running mode, OS service/task scheduler, background work while RCH is closed, generic durable task database, forced termination of other processes, automatic 115 writes, and changing P0/P1 gate status.
