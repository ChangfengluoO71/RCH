# 云端书源连接等待反馈

## Goal

为夸克、115 等扫码/Cookie 验证与连接流程增加即时 loading、阶段文案和重复提交防护。

## Requirements

- Show immediate loading feedback for Quark and 115 QR scanning, Cookie validation/connect, session creation, and required first-directory load.
- Use actual async operation state and real phases when exposed; do not display a fabricated percent or add UI polling.
- Disable duplicate submit while an operation is in progress.
- Guard loading state by request identity so completion of an older Future cannot clear a newer operation.
- On success continue the existing flow; on cancel/failure/timeout clear loading and preserve actionable error/retry behavior.

## Acceptance Criteria

- [ ] Loading appears when the action starts, stays visible while its operation is pending, and clears on every terminal path.
- [ ] Duplicate submissions are prevented and stale completions cannot change current state.
- [ ] Quark and both 115 Cookie/QR and official mode flows retain existing auth and data behavior.
- [ ] Foreground connect work is not deferred behind routine maintenance.

## Notes

- Keep `prd.md` focused on requirements, constraints, and acceptance criteria.
- Lightweight tasks can remain PRD-only.
- For complex tasks, add `design.md` for technical design and `implement.md` for execution planning before `task.py start`.
