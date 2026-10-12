# 索引刷新与同步变更正确性

## Goal

实现同源索引 single-flight、运行期间 dirty 保留和类型化同步变化结果。

## Requirements

- Same-source index refreshes share in-flight work; a forced refresh queued behind incremental work still executes afterward.
- Do not publish partial indexes on scan/commit failure.
- Replace broad automation dirty triggers with catalog-specific invalidation while preserving source-level changes raised during a running cycle.
- A dirty generation raised during a cycle is processed after that cycle, including on exceptions.
- Introduce structured sync outcome data for status and catalog invalidation scope; preserve existing status text consumers.
- Keep local filesystem freshness separate from remote revision. Unknown snapshot invalidation must refresh conservatively.
- EH Daily and 115 automation are out of scope.

## Acceptance Criteria

- [ ] Same-source refreshes do not overlap; force semantics are preserved.
- [ ] In-cycle dirty changes are not lost or replayed forever due to coordinator-owned notifications.
- [ ] Local dirty, remote applied changes, and per-source snapshot freshness are independently represented.
- [ ] Failed/unknown sync is never interpreted as a safe no-op.
- [ ] Existing full-index behavior remains atomic to readers.

## Notes

- Keep `prd.md` focused on requirements, constraints, and acceptance criteria.
- Lightweight tasks can remain PRD-only.
- For complex tasks, add `design.md` for technical design and `implement.md` for execution planning before `task.py start`.
