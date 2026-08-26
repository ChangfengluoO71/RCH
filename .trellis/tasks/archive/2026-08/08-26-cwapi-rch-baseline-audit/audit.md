# RCH CWapi v1.6.1 Baseline Audit

Date: 2026-08-26

## Executive result

The RCH v0.5.5 Rust core is healthy at exact commit `6d33ef11d7b8d4af492dcafab62befc6cd0587dc` when CWapi SAFE execution is given the actual MSVC developer environment and a worktree-local `TEMP/TMP` directory. The final Rust run completed with `235 passed; 0 failed; 2 ignored` after a successful `cargo check`.

Flutter cannot yet be treated as verified through this CWapi path. Direct execution of `D:/flutter/flutter/bin/flutter.bat` remains running with no output. Wrapping it with `cmd.exe /d /c call ...` returns exit 0 immediately but emits no Flutter output; BEFORE/AFTER sentinels confirm the wrapper executes while Flutter output is absent. Therefore no Flutter analyze/test pass is claimed by this audit.

## CWapi execution findings

1. Moving CWapi to a shorter portable path resolved the original Windows worktree filename-length failure.
2. The configured Slack transport channel works and MCP v2 repository requests execute against request-unique detached worktrees.
3. CWapi sees Python, Git, Flutter and Cargo on the machine.
4. Cargo initially failed because `link.exe` was not in the frozen CWapi PATH.
5. Visual Studio Build Tools 2022 is installed at `C:/Program Files (x86)/Microsoft Visual Studio/2022/BuildTools`; `vcvars64.bat` and x64 `link.exe` are present.
6. After loading `vcvars64.bat`, MSVC failed with LNK1104 because the inherited `D:/Temp` directory was not writable under SAFE execution.
7. Redirecting `TEMP` and `TMP` to `app/rust/target/tmp` inside the request worktree allowed the complete Rust baseline to pass.

## Trellis state findings

The repository reported 26 unarchived tasks before this audit and no active task for the CWapi session. The task tree contains historical `planning` / `in_progress` states that do not reliably describe implemented functionality.

Clear state-drift examples found in repository history include:

- AI upscaler functionality was implemented and released in v0.3.0, with later fixes and queue work, while old AI-upscaler tasks remain unarchived.
- Task drag reorder has a dedicated implementation commit (`72d3a574...`) while its task remains unarchived.
- Folder-cover work was implemented and released in v0.4.5 (`b4d5179d...`, `ff1b867d...`) while `08-08-sync-and-folder-cover` remains unarchived.
- Android P0/P5 were implemented in `42b30b11...`; P1 in `a8e5a57f...`; P3 plus release workflow in `6773e714...`; v0.4.1 shipped Android signing/update/touch fixes in `bdf28066...`. The M6 task tree still reports 0/6 done and several children as `in_progress` / `planning`.
- M8 catalog-rules-v3 was explicitly frozen and archived (`6ccc696b...`, `1084835b...`), while the older M8 parent/subtask tree remains open.

These examples justify a dedicated task-state reconciliation pass before choosing the next implementation task.

## Recommendations

1. Treat commit/release evidence as authoritative over stale Trellis status labels.
2. Create a dedicated `trellis-state-reconciliation` task and classify each of the 26 historical tasks as: completed/archive, superseded/archive, still actionable, or parent-state repair.
3. Prioritize M6 Android and M8 smart-scraping parent/child reconciliation because their status counters are visibly inconsistent with shipped work.
4. Preserve the working Rust CWapi recipe: load VS Build Tools `vcvars64.bat`, set `TEMP/TMP` inside the detached worktree, then invoke Cargo.
5. Before using CWapi as the Flutter verification authority, fix or document the `.bat` execution/output incompatibility and prove it with a command that produces normal Flutter version/analyze/test output.

## Product mutations

None. This audit branch contains only Trellis audit records.
