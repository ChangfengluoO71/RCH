# 缓存目录切换重构 — 技术设计（方向反转版）

## 1. 目标架构

- **应用根目录 = 用户选择的目录**（默认 `%APPDATA%\RCH`，可切换到如 `D:\Documents\TEST`）。
- 根目录内：`database.db`（数据）+ `cache/` + `download/`（缓存）+ 根级普通文件；**嵌套的应用支持目录（如 `%APPDATA%\RCH\RCH`）不迁移**（library.json 按 O-JSON=A 固定留在支持目录）。
- `db_path()` 保持 `cache_root()/database.db`（[db/mod.rs](C:/Users/cfl/Desktop/RCH/app/rust/src/db/mod.rs:26)），不引入 data_dir。
- 切换 = 整个根目录迁移（复制 + 校验 + 成功后删源）。

## 2. 变更点与数据流

### 2.1 启动恢复自定义根（解决先后依赖）

问题：设置存在 `database.db` 里，而 `database.db` 在自定义根里——打开数据库前必须先知道根在哪。

- 新增标记文件：`<支持目录>/cache_root.txt`，内容为当前根路径（空 = 默认根）。
- `cache_manager` 每次切换/恢复时同步写标记 + 更新设置；
- `main()` 启动顺序（[main.dart](C:/Users/cfl/Desktop/RCH/app/lib/main.dart:17)）：
  1. `RustLib.init()`；
  2. 读 `cache_root.txt` → 无则读 library.json 的 `settings.cacheDir` 兜底 → 有则 `setCacheRootPath`；
  3. **数据愈合（R5）**：若当前根缺 `database.db`，从候选位置（支持目录、默认根）挑最新的搬入（复制+校验+删源）；
  4. `dataIsMigrated` / 迁移 / `LibraryStore.load()`（load 内不再需要恢复逻辑）。

### 2.2 Rust 迁移 API（重建，不含 data_dir）

- `migrate_cache_root(from, to, support_dir) -> Result<u64>`：后台线程复制
  - `database.db`（若存在）、`cache/`、`download/`（若存在）、根级普通文件；
  - **跳过**：`support_dir` 相对 `from` 的路径（嵌套支持目录）、`migration.partial` 标记自身；
  - 进度写 `AtomicU64`（copied/total）；失败清理目标已复制内容、保留源、移除标记；
  - 开始写 `<from>/migration.partial`（JSON `{from, to}`），成功/优雅失败时移除。
- `migration_progress() -> (u64, u64)`：Dart 300ms 轮询。
- `available_space(path) -> u64`（fs2）。
- `delete_root_items(root)`：删除迁移过的项目（database.db + cache/ + download/ + 已复制的根级文件），仅限根目录内，拒绝磁盘根目录。
- `migration_pending(root) -> Option<(from, to)>`、`clear_migration_marker(root)`。
- 校验：复制后逐项比对文件数量/总大小。
- 路径安全：拒绝 from==to、互为祖先/子孙、磁盘根目录。

### 2.3 Dart 切换流程（cache_manager）

1. 文件夹选择器（file_selector）→ 校验目标 ≠ 支持目录及其父/子、≠ 当前根；
2. 空间检查（源大小 vs 目标可用空间）；
3. 确认对话框（显示源大小、目标位置，说明"数据库+缓存将整体迁移"）；
4. 进度对话框（轮询 `migration_progress`）；
5. 成功：`setCacheRootPath(to)` → 写 `cache_root.txt` → `settings.cacheDir = to` + `saveToDisk()` → `delete_root_items(from)`；
6. 失败：Rust 已清理目标残留，提示错误，不切换。

恢复默认 = 同流程反向（from=自定义，to=默认根），`cache_root.txt` 写空。

### 2.4 中断恢复（R7）

- `migration.partial` 在**源根**（启动时已知的位置：标记/默认根）；启动检测到 → 弹"继续迁移/稍后"；
- 继续迁移 = 重跑 `migrate_cache_root(from, to)` → 切换根 + 写标记 + 保存设置 + 删源（复用 2.3 步骤 5）；
- 整体重试，不做断点续传。

### 2.5 数据愈合（R5）

- 条件：当前根无 `database.db`；
- 候选：支持目录 `database.db`（旧版搬迁残留）、默认根 `database.db`、library.json 记录的旧 cacheDir 下的 `database.db`；
- 取 LastWriteTime 最新的候选 → 复制+校验+删源；失败不阻塞启动（下次重试）。

## 3. 兼容与回滚

- 不引入 data_dir；`db_path()` 行为与旧版一致（数据库在根目录）。
- 标签归一化修复（`ea0916b`）保留。
- 迁移中进程被杀：双份并存、无数据丢失；下次启动标记提示重试。
- 回滚：撤销相关文件即可，无 schema 变更。

## 4. 风险

| 风险 | 缓解 |
|---|---|
| 大目录迁移耗时 | 后台线程 + 进度轮询，不卡 UI |
| 迁移中杀进程 | 双份并存 + 标记恢复 |
| 误选支持目录/根目录为目标 | 路径校验（Dart + Rust 双层） |
| 删源误删 | 只删迁移清单内项目，拒绝磁盘根 |
| library.json 兜底读到过期 cacheDir | 以 `cache_root.txt` 标记为准，JSON 仅兜底 |
