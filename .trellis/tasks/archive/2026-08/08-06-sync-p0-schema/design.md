# P0 设计 — schema 与迁移

## 1. DDL（init_tables 内）

新库 CREATE TABLE IF NOT EXISTS 直接包含新列；老库在 init_tables 末尾走 `ensure_columns` 幂等补列。

### book_sources 追加

```sql
fingerprint TEXT,
remote_only INTEGER NOT NULL DEFAULT 0,
origin_device_id TEXT,
updated_at INTEGER NOT NULL DEFAULT 0,
deleted INTEGER NOT NULL DEFAULT 0
```

### read_records / book_metas 追加

```sql
stable_id TEXT,
updated_at INTEGER NOT NULL DEFAULT 0,
deleted INTEGER NOT NULL DEFAULT 0
```

### tags / book_tags / app_settings 追加

```sql
updated_at INTEGER NOT NULL DEFAULT 0,
deleted INTEGER NOT NULL DEFAULT 0
```

### 新表

```sql
CREATE TABLE IF NOT EXISTS devices (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    last_seen_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS sync_state (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS source_alias (
    source_id TEXT PRIMARY KEY,
    fingerprint TEXT NOT NULL,
    device_id TEXT NOT NULL DEFAULT '',
    updated_at INTEGER NOT NULL,
    FOREIGN KEY (source_id) REFERENCES book_sources(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_sources_fingerprint ON book_sources(fingerprint);
CREATE INDEX IF NOT EXISTS idx_metas_stable_id ON book_metas(stable_id);
CREATE INDEX IF NOT EXISTS idx_records_stable_id ON read_records(stable_id);
CREATE INDEX IF NOT EXISTS idx_source_alias_fp ON source_alias(fingerprint);
```

## 2. 迁移 helper

```rust
fn ensure_columns(conn: &Connection, table: &str, cols: &[(&str, &str)]) -> Result<()> {
    // PRAGMA table_info 收集现有列名；缺失列执行 ALTER TABLE ADD COLUMN
}
```

表名与 DDL 均为硬编码常量，无注入面。在 init_tables 中按表调用，放在现有 rotations/port 升级之后、normalize_legacy_tag_ids 之前。

## 3. Upsert 重写（保留新列 + 刷新 updated_at）

`upsert_source` / `upsert_record` / `upsert_meta` / `save_setting` / `link_tag` / `set_book_tags` 从 `INSERT OR REPLACE`（或裸 INSERT）改为：

```sql
INSERT INTO <t> (<原列...>, updated_at) VALUES (..., ?N)
ON CONFLICT(<pk>) DO UPDATE SET <原列>=excluded.<原列>, ..., updated_at=excluded.updated_at
```

`DO UPDATE` 不包含新列 → fingerprint / stable_id / remote_only / origin_device_id / deleted 被保留。`link_tag` 额外 `deleted=0`（兼容未来墓碑行）。`load_all_book_tags` 加 `WHERE deleted = 0`。

公共函数保持"锁全局连接"签名不变，内部拆出 `*_on(conn)` 变体供测试与复用：

```rust
fn upsert_source_on(conn: &Connection, s: &BookSourceRow) -> Result<()> { ... }
pub fn upsert_source(s: &BookSourceRow) -> Result<()> {
    let conn = get().lock().unwrap();
    upsert_source_on(&conn, s)
}
```

## 4. 新 helper（Rust-only）

```rust
pub fn get_or_create_device_id() -> Result<String>   // sync_state['device_id']，缺省生成 dev_{now_ms}_{pid} 并持久化
pub fn register_device(id: &str, name: &str) -> Result<()>  // devices upsert，last_seen_at=now
pub fn list_devices() -> Vec<DeviceRow>
pub fn set_sync_state(key: &str, value: &str) -> Result<()>
pub fn get_sync_state(key: &str) -> Option<String>
pub fn set_source_alias(source_id: &str, fingerprint: &str, device_id: &str) -> Result<()>
pub fn get_source_alias(source_id: &str) -> Option<SourceAliasRow>
pub fn load_source_aliases() -> Vec<SourceAliasRow>
pub fn set_source_fingerprint(id: &str, fingerprint: &str) -> Result<()>
pub fn set_meta_stable_id(key: &str, stable_id: &str) -> Result<()>
pub fn set_record_stable_id(key: &str, stable_id: &str) -> Result<()>
```

`delete_source` 显式清理 `source_alias` 行（不依赖 SQLite FK 开关）。

## 5. 测试计划

`db/mod.rs` 内 `#[cfg(test)] mod tests`，使用 `Connection::open_in_memory()`：

- 新库 schema：PRAGMA table_info / sqlite_master 校验新列与新表
- 老库迁移：先建旧版 book_sources，再 init_tables，校验补列且原数据保留
- upsert 保留语义：写入 fingerprint/stable_id 后重复 upsert，校验不被清空且 updated_at 更新
- device_id 幂等、source_alias / sync_state CRUD
