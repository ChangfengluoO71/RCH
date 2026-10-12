# 前后台任务协调

## Goal

使常规后台维护让路并合并，增加老化服务与安全边界抢占规则。

## Requirements

- Give foreground current-content queries, explicit scans/sync/batch actions, committed persistence, and required consistency recovery priority over routine maintenance.
- Coalesce and defer routine source refresh, scraping, cleanup, and unrelated prewarm work while foreground tasks are active.
- Resume deferred work after foreground activity, with a maximum wait/aging rule and bounded service slices so work cannot starve indefinitely.
- Yield to necessary foreground work only at safe source/batch/transaction boundaries.
- Do not add UI polling. Reduce remote scan polling only when measurements prove it causes database pressure, preserving timely visible progress.
- Do not absorb or change EH Daily or 115 automation.

## Acceptance Criteria

- [ ] Routine work is merged, not lost, and eventually advances under sustained foreground use.
- [ ] Necessary foreground operations start at the next safe boundary and are not queued behind maintenance.
- [ ] No task is interrupted inside a SQLite transaction and no new timer polling is introduced.
- [ ] EH Daily/115 automation contracts are unchanged.

## Notes

- Keep `prd.md` focused on requirements, constraints, and acceptance criteria.
- Lightweight tasks can remain PRD-only.
- For complex tasks, add `design.md` for technical design and `implement.md` for execution planning before `task.py start`.
