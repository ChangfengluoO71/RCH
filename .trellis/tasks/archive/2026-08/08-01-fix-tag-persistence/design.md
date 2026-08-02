# 修复标签持久化 — 技术设计

## 1. 目标状态

- **本地真源收敛**：SQLite 唯一真源；JSON 只导出不自动读（一次性对账后）。
- **recordRead 轻量落盘**（O1-B）：阅读记录 + `已读` 标签关联增量写 SQLite，调用点 await。
- **全量保存防抖 + 生命周期 flush**：写放大最小化，正常退出不丢。
- **一次性对账**（O3-A）：升级后首次启动 JSON 补 SQLite 缺口并回写，只跑一次。

## 2. 变更点与数据流

### 2.1 TagRepository 增量持久化入口

现状 `saveToSqlite()`（[tag_repository.dart](C:/Users/cfl/Desktop/RCH/app/lib/repository/tag_repository.dart:109)）每次全量 diff（读全表 `dbLoadAllBookTags`），不适合翻页热路径。

新增 `Future<void> persistBookLinks(String bookKey)`：只处理该书——

1. 该书所有标签 `dbEnsureTag(name)`（幂等，Rust 已有，[api/db.rs](C:/Users/cfl/Desktop/RCH/app/rust/src/api/db.rs:239)）；
2. 该书所有关联 `dbLinkTag(bookKey, name)`（幂等，[api/db.rs](C:/Users/cfl/Desktop/RCH/app/rust/src/api/db.rs:260)）。

不读全表、不 diff 全量；FRB API 已存在，无需改 Rust。

### 2.2 recordRead 异步化

`recordRead()` 改 `Future<void>`（[library_store.dart](C:/Users/cfl/Desktop/RCH/app/lib/store/library_store.dart:188)）：

- 内存变更（upsert 记录 + link `已读`）与 `notifyListeners()` 保持同步（UI 不延迟）；
- `await _records.saveOneToSqlite(r)`；
- `await TagRepository.instance.persistBookLinks(key)`；
- **移除** `_saveJsonBackup()` 调用（JSON 导出职责移交 2.3 的导出路径，避免每次翻页写全量 JSON）。

调用点全部 await：

- [opener.dart](C:/Users/cfl/Desktop/RCH/app/lib/ui/opener.dart:66)（openBook 本身 async）
- [reader_page.dart](C:/Users/cfl/Desktop/RCH/app/lib/ui/reader_page.dart:87)（翻页）
- [reader_page.dart](C:/Users/cfl/Desktop/RCH/app/lib/ui/reader_page.dart:201)（条漫点击）

### 2.3 全量保存防抖 + 生命周期 flush

- `LibraryStore` 增加防抖器（~800ms Timer）：`saveToDisk()` 排队合并，同一次操作风暴只落盘一次；内部用 Future 链串行化，避免并发写。
- 新增 `Future<void> flushPendingSave()`：等待当前排队/执行中的保存完成。
- `RchApp`（或 main.dart）挂 `AppLifecycleListener`：`onDetach`/`onPause`/`onHide` 时 `await flushPendingSave()`。
- `_save()` 语义调整（O2）：SQLite 分支失败 → 记录 `_lastSaveError` + rethrow（调用方可观测）；JSON 导出改为 best-effort（try/catch 仅日志，删除原 L137 的 `rethrow`）。JSON 写入保留在 `saveToDisk()` 里作为导出快照，但失败不影响 SQLite 结果。

实现细化（2026-08-01）：`saveToDisk()` 的 Future **不抛异常**——`_drainSaves()` 捕获所有失败并写入 `_lastSaveError`，等待者正常完成；调用方 await 后检查 `LibraryStore.lastSaveError` 观测结果（`_upscaleAll` 已按此实现）。原因：`saveToDisk` 在 `_cancelAiSuperResolve` 等多处 fire-and-forget 调用，抛异常会产生未处理异步异常；`lastSaveError` 通道在保持可观测的同时杜绝该风险。

### 2.4 一次性对账

标记：settings 新增 key `json_reconcile_done`（bool）。

`LibraryStore.load()` 的 SQLite 分支（[library_store.dart](C:/Users/cfl/Desktop/RCH/app/lib/store/library_store.dart:41)）末尾：

1. 标记未设置且 library.json 存在 → 直接读文件解析（绕过 `TagRepository._loaded` 守卫）；
2. 补缺：SQLite 缺失的 tag 行与 book_tags 关联补进内存，并通过 `dbEnsureTag`/`dbLinkTag` 写回 SQLite（本轮范围：tags + book_tags + records；metas/settings 不动，避免范围蔓延）；
3. 设置标记并保存；
4. JSON 不存在 → 直接置标记。

幂等：标记存在则跳过。边界：一次性窗口内 JSON 残留已删标签可能被补回（SQLite 删除成功但 JSON 写失败的历史数据），接受，窗口关闭后不再发生。

### 2.5 `_upscaleAll` 保存等待

[book_detail_page.dart](C:/Users/cfl/Desktop/RCH/app/lib/ui/book_detail_page.dart:119) 改为 `await LibraryStore.instance.saveToDisk()`，catch 后 SnackBar 提示"标签已记录但保存失败"（缓存与内存不受影响）。

## 3. 兼容与回滚

- 不碰 SQLite schema、不碰 `tag_id` 算法；同步元数据列属 `08-01-data-layer-sync` 任务。
- `recordRead` 签名变更影响 3 个调用点，回滚需同步恢复；其余变更局部。
- **前置基线**：当前工作区含大量未提交改动（AI 功能 + `_tagId` 修复），实施前先提交基线 commit，避免回滚时丢失。

## 4. 风险与缓解

| 风险 | 缓解 |
|---|---|
| 翻页热路径新增 2 次轻量 SQLite 写 | 预计 <5ms；实测超标则回退为"record 必写 + 标签关联进防抖队列" |
| Windows 生命周期事件触发时机 | 实测 Alt+F4 / 任务栏关闭 / 崩溃三种退出；崩溃场景本就接受丢最后一条 |
| 对账补回残留已删标签 | 一次性窗口，接受 |
