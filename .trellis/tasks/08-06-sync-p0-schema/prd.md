# P0 同步元数据落库（schema + 迁移）

## Goal

把 SQLite 数据层升级为"同步就绪"：六张同步实体表（tags / book_tags / book_metas / read_records / book_sources / app_settings）增加同步元数据列，新增 devices / sync_state / source_alias 三张支撑表；存量与增量读写行为完全不变。

## Requirements

- R1 同步列：`book_sources` 增加 `fingerprint` / `remote_only` / `origin_device_id` / `updated_at` / `deleted`；`read_records`、`book_metas` 增加 `stable_id` / `updated_at` / `deleted`；`tags`、`book_tags`、`app_settings` 增加 `updated_at` / `deleted`。
- R2 新表：`devices(id, name, created_at, last_seen_at)`、`sync_state(key, value, updated_at)`、`source_alias(source_id, fingerprint, device_id, updated_at)`；相关索引。
- R3 老库迁移：幂等 ALTER TABLE 补列（沿用现有 PRAGMA + ADD COLUMN 模式），不丢数据。
- R4 Upsert 语义：改为 `ON CONFLICT ... DO UPDATE`，保留 fingerprint / stable_id / remote_only / origin_device_id / deleted 等新列，每次写入刷新 `updated_at`（LWW 基础）。
- R5 内部 helper（Rust-only，P0 不暴露 FRB）：设备 ID 生成与注册、sync_state 读写、source_alias 读写、stable_id / fingerprint 写入。
- R6 回归：现有 CRUD 语义不变；`load_all_book_tags` 过滤 `deleted = 0`；删除路径维持硬删除（墓碑写入语义属 P3）。

## Acceptance Criteria

- [ ] 新库 init 后 PRAGMA 校验：所有同步列、三张新表、索引齐全
- [ ] 老库（无新列）init 后自动补列、原数据完整
- [ ] upsert 保留 fingerprint / stable_id 且 `updated_at` 刷新；重复 upsert 不产生重复行
- [ ] device_id 幂等稳定；source_alias / sync_state 读写正确
- [ ] `cargo check && cargo test` 通过；`flutter analyze && flutter test` 回归通过

## Out of Scope

- FRB/Dart API 暴露（P1 起按需）
- 墓碑写入与软删除语义（P3）
- 标准包格式（P1）、传输通道（P2）、合并引擎（P3）
