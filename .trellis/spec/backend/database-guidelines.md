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

## Source Identity（ADR-020）

- `fingerprint = sha256(type + "://" + endpoint + "/" + root)`，**不含用户名/账号**（同库不同账号 = 同一个源、不同凭据）。
- 规则：`local`→规范化路径；`smb`→规范化 UNC；`webdav/sftp`→规范化 URL（host 小写、剥离 userinfo、去尾斜杠）+ 初始路径；`baidu`→根目录路径；`115/quark`→root_id。
- 写入点：`upsert_source_on` 每次按身份字段派生并写入（禁止 NULL 新增）；`init_tables` 末尾对 NULL/空值存量行幂等回填；v1 旧包导入后在 `merge_package` 内立即回填。
- `library_index`（ADR-020/021）：物理资产发现层（path/size/mtime/cover_ref），与 `book_metas` 用户认知层严格分离，互不生成。

## Common Mistakes

- **`INSERT OR REPLACE` 会重置未出现在插入列里的新列**（如 fingerprint / stable_id / deleted 被清空）。新增列后必须改用 `INSERT ... ON CONFLICT(<pk>) DO UPDATE SET ...`，且 DO UPDATE 不包含需要保留的新列。
- **先建索引后补列**：老库升级时若 CREATE INDEX 引用的列尚不存在，SQLite 直接报 `no such column`。索引语句要放到所有 ensure_columns 之后。
- 依赖 SQLite 外键级联不可靠：FK 开关依连接而异，显式 DELETE 关联行（如 `delete_source` 同时清理 `source_alias`）。
- **SQL 组合条件注意 `AND`/`OR` 优先级与可空列**：`load_source_credentials` 曾因 `fingerprint IS NOT NULL ... OR 凭据非空` 把 fingerprint 为 NULL 的行选进来，`row.get::<String>` 读到 NULL 直接崩（`Invalid column type Null`）。可空列要么用 `Option<T>` 读取，要么在 WHERE 里先过滤非空。
