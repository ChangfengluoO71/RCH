# 超分计划任务列表 — 支持拖拽排序

## Goal

提供后台超分任务**列表视图**，支持拖拽调整排队任务的执行顺序（正在执行的任务不可移动），顺序持久化、重启后保持。

## Background

- 目前只有右上角悬浮小窗展示任务（进行中/排队 + 取消），无任务列表页；
- 任务顺序由 `ai_tasks` 表 `created_at` 决定（[db/mod.rs](C:/Users/cfl/Desktop/RCH/app/rust/src/db/mod.rs:315) 按 created_at 排序加载）。

## Requirements

- **R1** 任务列表入口（建议：悬浮窗展开/详情页/设置页一处即可）。
- **R2** 列表展示所有任务（进行中/排队/状态/进度/取消按钮）。
- **R3** 拖拽调整**排队中**任务的顺序；进行中任务固定在顶部不可拖。
- **R4** 顺序持久化：`ai_tasks` 增加 `sort_order`（或等价机制），重启后保持；worker 按新顺序取任务。

## Acceptance Criteria

- [x] 列表可打开、状态/进度实时更新（悬浮窗展开面板，250ms 轮询 + notify 双刷新）
- [x] 排队任务拖拽后执行顺序改变，且新任务按调整后的顺序入队（新任务 sort_order = 当前最大值 + 1）
- [x] 重启后顺序保持（sort_order 持久化到 ai_tasks，加载按 sort_order 排序）
- [x] 进行中任务不可移动（固定在顶部、无拖拽手柄，reorder 对 running 下标直接忽略）
- [x] `flutter analyze` 0 issues

## Open Questions（开工前确认）

- **O1** 列表入口位置：悬浮窗展开面板 vs 详情页 vs 设置页（推荐悬浮窗展开）。
- **O2** 排序字段：`ai_tasks.sort_order INTEGER`（推荐）——已有表的升级用 `ALTER TABLE` 兼容（`init_tables` 检测列缺失时补列）。

## Decisions（2026-08-02 已定）

- **O1**：入口 = 悬浮窗展开面板（折叠态顶部"展开"按钮 → 任务列表，进行中置顶不可拖、排队任务带拖拽手柄）。
- **O2**：`ai_tasks.sort_order INTEGER NOT NULL DEFAULT 0`；`init_tables` 用 PRAGMA 检测列缺失时 `ALTER TABLE` 补列（兼容旧库）。
- 加载排序：`ORDER BY (status='running') DESC, sort_order, created_at`——进行中恒在顶部，不依赖 sort_order；旧数据（0）按创建时间兜底。
- 新任务入队：`sort_order = 当前最大值 + 1`（排队恒 ≥1）；拖拽后仅重排排队任务并重编号 1..N，进行中任务不参与。
- worker（AiUpscaleManager）按 `activeTasks`（running 在前 + 排队按 sort_order）顺序取任务；`reorderQueued` 持久化后更新内存。

## Verification（2026-08-02）

- Rust：新增 `sort_order` 列 + 迁移、`reorder_ai_tasks`（事务批量更新）；单测 `ai_tasks_sort_order_migration_reorder_and_ordering` 覆盖旧库补列、旧数据默认 0、running 置顶、重排生效。`cargo test --lib` 35 passed。
- FRB：`flutter_rust_bridge_codegen generate` 重新生成，diff 仅 `AiTaskDto.sortOrder` 与 `db_reorder_ai_tasks`。
- Flutter：`AiTask.sortOrder` + `enqueue` 自动编号 + `reorderQueued` + `activeTasks`；悬浮窗折叠/展开双形态，展开态 `ReorderableListView`（`onReorderItem`）。`flutter analyze` 0 issues，`flutter test` 8/8 通过。

## 复盘修正（2026-08-02，用户反馈展开面板异常后）

- **红屏根因（Overlay）**：用户截图 OCR 确认报错 `No Overlay widget found. RawTooltip/ReorderableListView require an Overlay ancestor`。悬浮窗挂在 `MaterialApp.builder` 层（Navigator/Overlay 之上），展开面板里的 `ReorderableListView`（拖拽代理）与 `IconButton(tooltip:)` 都需要 Overlay。修复：展开形态用**局部 `Overlay` 承载面板**（`Positioned(top:0,right:0)`，其余区域透明不挡点击）；widget 测试改为与 main.dart 一致的 `MaterialApp.builder` 挂载方式，复现并锁定该异常。
- **拖拽无效 Bug**：`reorderQueued` 重编号时先按"旧的 sortOrder"排序再重排，导致拖拽后顺序不变（DB 也写回旧序）。已改为按 `_tasks` 新顺序重编号；新增 widget 测试 `ai_floating_progress_test.dart`（展开渲染 + 真实手势拖拽断言顺序变化），复现并锁定该 Bug。
- 布局加固：展开面板原用 `Flexible` 嵌 `mainAxisSize.min` 的 Column（脆弱），改为 `width:340 + ConstrainedBox(maxHeight)` + 列表自带 `maxHeight:320` 自适应。
- 数据核查：数据库 `ai_tasks` 标题/进度均正常（之前"乱码"为终端输出编码假象）；文件与生成代码均为合法 UTF-8。
- 验证：`flutter analyze` 0 issues；`flutter test` 10/10 通过（新增 2 个悬浮窗测试，其中拖拽测试含真实手势与顺序断言）。
