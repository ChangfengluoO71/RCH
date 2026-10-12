# 性能基线与诊断

## Goal

Profile/Release 下测量扫描、同步、批量与云端连接路径的耗时和前台影响。

## Requirements

- Use Profile/Release builds for representative Windows measurements; do not use Debug frame times as performance conclusions.
- Measure foreground frame budget misses and build/raster p95 alongside scan/sync/batch/cloud-connect stage timings.
- Record database wait/transaction duration if existing diagnostics expose it; identify the remaining instrumentation gap instead of silently assuming a cause.
- Do not log credentials, titles, or full filesystem paths.
- Android device is not currently connected; record mobile baseline as pending rather than extrapolating from Windows.

## Acceptance Criteria

- [ ] Windows Profile/Release baseline scenarios are recorded with environment and run steps.
- [ ] P95/frame misses and operation stage timings distinguish UI CPU, file I/O, remote wait, and database lock contention where tooling allows.
- [ ] Android baseline limitation is explicit; no Debug timings are used as evidence.
- [ ] Any proposed new instrumentation is narrowly scoped and documented before it is added.

## Notes

- Keep `prd.md` focused on requirements, constraints, and acceptance criteria.
- Lightweight tasks can remain PRD-only.
- For complex tasks, add `design.md` for technical design and `implement.md` for execution planning before `task.py start`.
