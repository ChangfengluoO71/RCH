# 批量标签数据库往返优化

## Goal

将批量标签改为可等待分阶段操作，批量索引及关系/元数据持久化并限制事务时长。

## Requirements

- Make `batchTag` awaitable and return typed phase/final results.
- Carry target path, file/directory type, name, and parent path through index and tag persistence.
- Batch index upserts and BookMeta/Tag/BookTag persistence using bounded idempotent transactions; avoid N FFI calls and avoid one oversized lock hold.
- Merge tag repository notifications rather than notifying once per relation.
- Associate SQLite persistence success/failure with the current batch operation; do not rely solely on `Future<void>` completion or mutable global `lastSaveError`.
- Show phase progress and final processed/total plus success/skip/failure counts; do not rebuild UI for every target.
- Preserve existing read marker and author/genre/series semantics.

## Acceptance Criteria

- [ ] Files/directories and parent paths survive the batch operation unchanged.
- [ ] Index and relation persistence are batched and bounded; partial failure is reported accurately and retries are idempotent.
- [ ] Metadata tags and normalized tag links both reach SQLite.
- [ ] User sees stage progress/final summary and receives no false success on persistence failure.
- [ ] Database lock p95 and foreground open latency do not worsen due to larger transactions.

## Notes

- Keep `prd.md` focused on requirements, constraints, and acceptance criteria.
- Lightweight tasks can remain PRD-only.
- For complex tasks, add `design.md` for technical design and `implement.md` for execution planning before `task.py start`.
