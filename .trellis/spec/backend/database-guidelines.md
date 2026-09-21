# Database Guidelines（Rust 核心的 SQLite 约定）

> 2026-09-21 补实（替换原模板）。每条都能指向真实代码。

## 连接与锁

- 唯一入口：`crate::db::get()`（全局连接，Mutex 保护）。**不要**另开连接。
- 时间统一用 `crate::db::now_ms()`（epoch 毫秒），不要混用 `SystemTime`。
- 事务用 `conn.unchecked_transaction()`（见 `remote_scan/cover_service.rs`）；
  **文件读写、网络、provider 调用一律在事务之外**（先备好数据，再进事务写）。
- 事务内只做 SQL；`tx.commit()` 之后才发事件/唤醒（commit-after-emit，见封面 revision 通知）。

## 命名与键

- 表名列名统一 `snake_case`。关键表：`library_index`、`remote_cover_job` / `remote_cover_variant` /
  `remote_cover_blob` / `remote_cover_ref`、`remote_view_revision`、`remote_scan_epoch`、`app_settings`。
- 封面复合键顺序固定 `(source_id, asset_id, content_revision, selection_revision, profile)`；
  同一组字段也是封面缓存文件名 `sha256(5 段长度前缀).cover-v2` 的输入（`CacheDir::Cover`）。
- 代际列（`generation` / `session_epoch` / `revision`）必须与写入方绑定的值一致：
  旧会话不得在新会话绑定后发布或结束任务。

## 迁移与兼容

- 迁移集中在 `db/mod.rs`；新表/新列要允许旧数据缺失（`COALESCE` 或读取侧兜底）。
- **不猜状态**：新增枚举值必须同时更新状态机与契约测试（例 `remote_cover_job.state`）。
- 批量写用单事务包裹，避免逐条 commit；只读查询用 `prepare` + `query_map`。

## 禁止

- 持有 `MutexGuard` 跨越可能重入同一锁的调用（回调、prefetch、唤醒）；
- 在锁内做文件/网络 I/O；
- `format!` 拼 SQL（一律 `params![]` 绑定）。
