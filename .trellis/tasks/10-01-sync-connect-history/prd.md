# 同步建连失败写入历史

## Goal

Ensure a failed WebDAV connection attempt is visible in persistent sync history, consistent with ADR-027.

## Requirements

- Record a durable attempt outcome when `SyncEngine` fails before calling the Rust `sync_now` API, including start/end time, failure phase, and a user-readable error.
- Make the new entry visible through the existing recent sync-history surface; preserve the existing last-error and retry behavior.
- Do not store credentials, full URLs, or other secrets in the history entry.
- Prefer the existing `sync_history` contract; record any required API or schema changes in a design before implementation.

## Acceptance Criteria

- [ ] A WebDAV connection failure creates a new persistent history row with a connect phase and error, visible after app restart.
- [ ] A successful sync still records its existing summary and clears the current last error.
- [ ] Targeted tests cover connection failure, subsequent success, and secret redaction.
- [ ] ADR-027 remains satisfied: every attempted sync, including failures before transport, is represented in recent history.

## Notes

- Keep `prd.md` focused on requirements, constraints, and acceptance criteria.
- Lightweight tasks can remain PRD-only.
- For complex tasks, add `design.md` for technical design and `implement.md` for execution planning before `task.py start`.
