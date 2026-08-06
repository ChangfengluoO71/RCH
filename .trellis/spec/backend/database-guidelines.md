# Database Guidelines

> Database patterns and conventions for this project.

## Overview

- 驱动：`rusqlite`（bundled SQLite），全项目唯一持久化层，位于 `app/rust/src/db/mod.rs`。
- 全局单例 `Mutex<Connection>`（`OnceLock` 惰性打开 `cache_root()/database.db`）；所有 pub CRUD 函数自行持锁完成整个操作，调用方不关心锁。
- Dart 侧通过 FRB 桥接 `api/db.rs` 读写，repository 层（`app/lib/repository/`）是数据持有者。
- 表命名 snake_case，主键统一 `TEXT PRIMARY KEY`（无自增主键），时间戳统一**毫秒** i64。

## Query Patterns

- 批量写操作必须包在显式 `BEGIN` / `COMMIT`（或 `transaction()`）里，失败回滚。
- 加载用 `query_map` + `filter_map(|r| r.ok())`，保持对坏行宽容。
- 同步实体表统一带 `updated_at`（毫秒 LWW）与 `deleted`（墓碑，查询过滤 `deleted = 0`）。

## Migrations

- 新库：`init_tables()` 里 `CREATE TABLE IF NOT EXISTS` 直接包含新列/新表（幂等）。
- 老库：同一函数末尾用 `ensure_columns(conn, table, &[("col", "ALTER ...")])` 幂等补列（PRAGMA table_info + ALTER TABLE ADD COLUMN）。
- 顺序陷阱：**依赖新列的索引必须在补列之后创建**，否则老库在 CREATE INDEX 处直接报错（P0 已踩过，见 Common Mistakes）。
- 迁移完成标记：`schema_version` 表（library.json → SQLite 一次性迁移用）。

## Naming Conventions

- 同步列：`updated_at` / `deleted`；跨端稳定标识：`stable_id`（书）、`fingerprint`（书源）。
- 索引：`idx_<table>_<col>`。

## Common Mistakes

- **`INSERT OR REPLACE` 会重置未出现在插入列里的新列**（如 fingerprint / stable_id / deleted 被清空）。新增列后必须改用 `INSERT ... ON CONFLICT(<pk>) DO UPDATE SET ...`，且 DO UPDATE 不包含需要保留的新列。
- **先建索引后补列**：老库升级时若 CREATE INDEX 引用的列尚不存在，SQLite 直接报 `no such column`。索引语句要放到所有 ensure_columns 之后。
- 依赖 SQLite 外键级联不可靠：FK 开关依连接而异，显式 DELETE 关联行（如 `delete_source` 同时清理 `source_alias`）。
