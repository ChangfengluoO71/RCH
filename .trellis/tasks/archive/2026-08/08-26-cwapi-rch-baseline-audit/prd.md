# CWapi RCH baseline audit

## Goal

Audit the RCH v0.5.5 exact-commit baseline without changing product code.

## Baseline

- Repository: `ChangfengluoO71/RCH`
- Exact commit: `6d33ef11d7b8d4af492dcafab62befc6cd0587dc`
- App version: `0.5.5+505`
- CWapi workflow: v1.6.1 / MCP v2 / SAFE

## Scope

- Inventory unarchived Trellis task state.
- Validate the CWapi execution environment.
- Run or diagnose Flutter baseline checks.
- Run Rust `cargo check` and `cargo test`.
- Compare stale Trellis states with implementation/release commits.

## Acceptance Criteria

- [x] CWapi repository worktree creation succeeds at the exact commit.
- [x] Python, Git, Flutter and Cargo executables are discovered rather than assumed.
- [x] Trellis active-task state and unarchived task inventory are inspected.
- [x] Flutter execution behavior under CWapi is diagnosed without claiming an unverifiable pass.
- [x] Rust baseline is executed with the required MSVC environment and worktree-local temporary directory.
- [x] Historical task-state drift is demonstrated with commit evidence.
- [x] No product source is modified and no production mutation occurs.

## Out of Scope

- Product feature implementation.
- Automatic archival of historical tasks without per-task verification.
- Changes to CWapi itself.
