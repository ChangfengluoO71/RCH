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

- [ ] 列表可打开、状态/进度实时更新
- [ ] 排队任务拖拽后执行顺序改变，且新任务按调整后的顺序入队
- [ ] 重启后顺序保持
- [ ] 进行中任务不可移动
- [ ] `flutter analyze` 0 issues

## Open Questions（开工前确认）

- **O1** 列表入口位置：悬浮窗展开面板 vs 详情页 vs 设置页（推荐悬浮窗展开）。
- **O2** 排序字段：`ai_tasks.sort_order INTEGER`（推荐）——已有表的升级用 `ALTER TABLE` 兼容（`init_tables` 检测列缺失时补列）。
