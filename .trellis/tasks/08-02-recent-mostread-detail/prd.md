# 最近阅读/最多阅读 点击进入漫画详情页

## Goal

主页"最近阅读"和"最多阅读"列表的漫画卡片，点击后进入**漫画详情页**（而不是直接开始阅读），与本地书架行为一致；详情页内保留"开始阅读"入口。

## Background（已确认）

- 本地书架卡片已打开详情页：[home_page.dart](C:/Users/cfl/Desktop/RCH/app/lib/ui/home_page.dart:399) `BookDetailPage(source, path, title)`。
- 最近阅读（L258）与最多阅读（L288）当前直接调 `openBook(...)` 进入阅读器——行为不一致。

## Requirements

- **R1** 最近阅读/最多阅读卡片点击 → 打开 `BookDetailPage`。
- **R2** 详情页有"开始阅读"入口（复用现有阅读按钮/右键逻辑）。

## Acceptance Criteria

- [x] 两个列表点击均进入详情页，不再直接开书（`_buildLocalResults` 的 onTap 改为 push `BookDetailPage`）
- [x] 从详情页可正常开始阅读、续读进度正确（复用详情页 L169 "开始阅读"按钮 → `openBook`，进度逻辑未动）
- [x] `flutter analyze` 0 issues

## Out of Scope

- 列表布局/卡片样式改动。

## Verification（2026-08-02）

- `flutter analyze`：No issues found（0 issues）。
- `flutter test`：8/8 通过（既有阅读器/路径测试无回归）。
- 改动仅 [home_page.dart](C:/Users/cfl/Desktop/RCH/app/lib/ui/home_page.dart:288) 一处 onTap；详情页入口与续读逻辑完全复用，无新增状态。
