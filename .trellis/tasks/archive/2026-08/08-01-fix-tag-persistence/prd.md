# 修复标签持久化 — 重启后标签丢失（AI超分/已读）

## Goal

修复"重启应用后标签丢失"：`AI超分`、`已读`（以及普通标签/阅读进度）在写入后必须跨重启保留。用户价值：整本超分打的 `AI超分` 标签和阅读记录不再无故消失，元数据标签体系可信。

## Background（已确认的事实）

### 1. 标签 ID 算法已对齐（唯一已完成部分）

- `TagRepository._tagId()` 已从 DJB2 hash 改为 `name.trim().toLowerCase()`（[tag_repository.dart](C:/Users/cfl/Desktop/RCH/app/lib/repository/tag_repository.dart:300)），与 Rust `db::tag_id()`（[db/mod.rs](C:/Users/cfl/Desktop/RCH/app/rust/src/db/mod.rs:920)）一致。已核对一致。
- 该改动在工作区未提交；旧 hash ID 数据依赖 `_normalizeTagIds()` 归一化（loadFromSqlite 中已调用）。
- **改完后重启标签仍丢失**，说明根因不在 ID 算法，而在持久化链路。

### 2. `recordRead()` 是 fire-and-forget，且不调 `_save()`

`recordRead()` 签名是 `void`（[library_store.dart](C:/Users/cfl/Desktop/RCH/app/lib/store/library_store.dart:188)），内部：

- `_records.saveOneToSqlite(r)`（L197）— **未 await**，只写阅读记录到 SQLite，不写标签关联；
- `_saveJsonBackup()`（L198）— **未 await**，只写全量 JSON（包含 `TagRepository.toJson()`，见 L216-226）；
- **从不调用 `_save()`**，因此 `已读` 标签关联只会出现在 JSON，SQLite 里没有——直到之后某次其他操作（设置/元数据变更等）触发全量 `_save()` 才补上。

调用点（热路径）：打开书 [opener.dart](C:/Users/cfl/Desktop/RCH/app/lib/ui/opener.dart:66)、翻页 [reader_page.dart](C:/Users/cfl/Desktop/RCH/app/lib/ui/reader_page.dart:87)、条漫点击 [reader_page.dart](C:/Users/cfl/Desktop/RCH/app/lib/ui/reader_page.dart:201)。

### 3. 整本超分的 `saveToDisk()` 未 await

`_upscaleAll()` 打完 `AI超分` 标签后调用 `LibraryStore.instance.saveToDisk();`（[book_detail_page.dart](C:/Users/cfl/Desktop/RCH/app/lib/ui/book_detail_page.dart:119)），未 await。若用户在保存完成前退出/杀进程，标签丢失。

### 4. `_save()` 的错误处理

`_save()`（[library_store.dart](C:/Users/cfl/Desktop/RCH/app/lib/store/library_store.dart:118)）：SQLite 失败 → catch + 打印，继续写 JSON（正确）；JSON 写失败 → `rethrow`（L137）。fire-and-forget 调用下 rethrow = 未处理异步异常，且 JSON 失败直接放弃。

### 5. 启动加载不做 JSON↔SQLite 交叉校验

`main.dart`：已迁移 → `LibraryStore.load()` → `_loadFromSqlite()` → `TagRepository.loadFromSqlite()`（[tag_repository.dart](C:/Users/cfl/Desktop/RCH/app/lib/repository/tag_repository.dart:86)），**从不读 JSON 补缺**。JSON fallback 仅在 SQLite 加载失败时启用（[library_store.dart](C:/Users/cfl/Desktop/RCH/app/lib/store/library_store.dart:41)）。

推论：`recordRead` 只把 `已读` 标签写进 JSON → 重启后从 SQLite 加载 → 标签消失。与用户描述一致。`TagRepository.load(File)` 与 `loadFromSqlite()` 各有 `_loaded` 守卫（L87-88），后续交叉校验不能直接复用这两个入口。

## Requirements

- **R1** `recordRead()` 的持久化链路保证标签/记录最终落盘（**已定：B 方案 — 轻量落盘 + 防抖全量 + 生命周期 flush**）：
  - `recordRead()` 改 `Future<void>`，调用点 await（打开书 [opener.dart](C:/Users/cfl/Desktop/RCH/app/lib/ui/opener.dart:66)、翻页 [reader_page.dart](C:/Users/cfl/Desktop/RCH/app/lib/ui/reader_page.dart:87)、条漫点击 [reader_page.dart](C:/Users/cfl/Desktop/RCH/app/lib/ui/reader_page.dart:201)）；
  - await 的内容是轻量持久化：`_records.saveOneToSqlite(r)` + `已读` 标签关联增量写 SQLite（TagRepository 增量 upsert），不写全量 JSON；
  - 全量 `saveToDisk()` 改为防抖合并（同一次操作风暴只落一次盘）+ 应用退出生命周期（`AppLifecycleListener` detached/paused）强制 flush 一次；
  - 接受取舍：强杀进程（非正常退出）仍可能丢最后一条记录。
- **R2** `_upscaleAll()` 中 `saveToDisk()` 改为 await，且失败要有可观测结果（标签与缓存行为一致）。
- **R3** `_save()` 语义（**已定，随长期架构收敛**）：SQLite 是唯一真源，SQLite 写失败 = 真实失败（日志 + 可观测报错返回调用方）；JSON 降级为 best-effort 导出，失败只记日志、永不阻塞/中断 SQLite 写入。
- **R4** 启动一次性对账（**已定：O3-A 一次性对账，非长期双源**）：SQLite 已迁移时，升级后首次启动读一次 library.json，把 SQLite 缺失的标签/关联补进内存并回写 SQLite，用标记位（settings 或文件）保证只跑一次；对账完成后 JSON 不再被自动读取。边界：一次性窗口内 JSON 残留的已删标签可能被补回（SQLite 删除成功但 JSON 写失败的历史数据），窗口关闭后不再发生，接受。
- **R5** 回归验证清单（用户提供的"已实现需验证"项）：
  - AI 超分 scale 为 2x（已实现，验证 UI 传参 2 且 exe 用 x2 模型）；
  - `AI超分` 作为元数据标签显示为红色（已实现，验证重启后仍在）；
  - 阅读器自动查 AI 缓存加载超分页（已实现，验证重启后仍秒开超分页）；
  - 详情页"取消 AI 超分"（清标签+缓存）与"阅读未超分版本"（跳过 AI 缓存）功能正常。

## Acceptance Criteria

- [ ] 模拟"整本超分完成 → 立即退出进程"后重启，`AI超分` 标签和书关联仍在
- [ ] 模拟"阅读一页 → 立即退出进程"后重启，`已读` 标签和最后阅读进度仍在
- [ ] 普通标签新增/删除/重命名、批量打标签后重启，结果保留
- [ ] 翻页/打开书路径性能无可见劣化（B 方案下轻量 SQLite 写应在每次翻页可感知延迟内完成）
- [x] `flutter analyze` 0 issues；`cargo test --lib` 通过（24 passed，2026-08-01）
- [ ] R5 清单 5 项手工验证通过

> 手工验证项（读一页/整本超分后立即退出→重启标签在；删标签→重启不复活；翻页流畅度）需用户在真机/桌面端执行，见 implement.md 第 8 步。

## Out of Scope

- 早期 backlog 需求（标签折叠/缩放区域/后缀识别/CBZ 转换/后台计划超分）→ 已另建 `08-01-m2-backlog` 任务
- SQLite schema 变更（同步元数据列）、标准包格式、WebDAV 备份/多端同步 → 已另建 `08-01-data-layer-sync` 规划任务（本任务只做真源收敛，不碰 schema）
- 标签 UI 重构

## Resolved Decisions（已确认）

- ~~**O1** `recordRead()` 的持久化策略~~ → 已定：B 方案（轻量落盘 + 防抖全量 + 生命周期 flush）
- ~~**O2** `_save()` 失败语义~~ → 已定：SQLite 失败真实报错，JSON 导出 best-effort 不阻塞
- ~~**O3** 启动交叉校验方向~~ → 已定：A 方案，一次性对账（JSON 补 SQLite + 回写 + 只跑一次）
- ~~**O4** backlog 拆分~~ → 非本任务阻塞，保持 `08-01-m2-backlog` 单任务登记，开工前再拆
