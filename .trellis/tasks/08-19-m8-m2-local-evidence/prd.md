# M8-M2 — Canonical Identity & Migration

## Goal

在 M8-M1 的本地名称/角色 proposal 稳定后，建立可确认、可去重、可关联、可同步的 canonical 作品身份地基。

## Requirements

- 独立 ordered DDL migration ledger，不复用 `schema_version` / `CURRENT_SCHEMA_VERSION` 的 `library.json` import 语义。
- 建立 `works`、`work_external_ids`、`work_links`、confirmed provenance 与本地 sync-dirty；不添加 `book_metas.work_id`。
- 稳定 work ID、book key、external identity、唯一关系、tombstone 与 merge 合同明确。
- M8-M1 的 proposal 仍是 working state；本任务不把未确认的标题/作者写入 canonical。

## Acceptance Criteria

- [ ] 新库、真实旧库副本、重复升级、注入失败回滚和未来版本拒绝均通过。
- [ ] 一个 work 可关联多个文件/版本；external ID 与 work link 唯一约束有效。
- [ ] tombstone 保留稳定 ID 并标记 sync-dirty；不触发 sync transport。
- [ ] 现有书架、阅读进度、标签、手工元数据和 `library.json` import tests 无回归。

## Dependencies / Out of Scope

- 依赖 M8-M1 输出合同。
- 不实现 Provider、内嵌 metadata、fingerprint、review UI 或 sync payload。
