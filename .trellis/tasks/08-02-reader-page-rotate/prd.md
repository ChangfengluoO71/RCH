# 阅读器页面旋转（M4 子集）
## Goal

阅读器内支持把页面旋转 90°（顺/逆时针）与 180°，解决横版图片/扫描件方向不对时的阅读问题。这是 M4 复杂场景中「旋转」的独立子集。

## Background（已确认事实）

- M4 里程碑包含 智能拼页 / 旋转 / 裁边，其余两项复杂度高，本任务只做旋转 [SPEC.md §10]
- 阅读器单页用 `PhotoView(controller: _photoCtrl)` 渲染，缩放/平移由 PhotoView 管理 [app/lib/ui/reader_page.dart:217]
- 双页用 InteractiveViewer + 自绘 Row [app/lib/ui/reader_page.dart:237-257]
- 条漫用 InteractiveViewer + ListView [app/lib/ui/reader_page.dart:263-279]
- 已有自定义快捷键系统（zoomIn/zoomOut/zoomReset/forward/back 5 个动作）[app/lib/ui/reader_page.dart:166-186]

## Requirements

- **R1** 入口：阅读器右键菜单新增「界面旋转」，点击进入旋转模式（菜单项变为「退出旋转模式」，可再次点击退出）。
- **R2** 旋转按钮：旋转模式下，单页模式在页面下方显示 1 个旋转按钮；双页模式在两页下方各显示 1 个（左页/右页独立）。
- **R3** 每页独立旋转：按钮点击顺时针旋转该页 90°（0/90/180/270 循环）；旋转左页不影响右页。
- **R4** 持久化：每页旋转结果按书保存（`BookMeta.rotations`: pageIndex → 度数），下次打开该漫画自动应用。
- **R5** 旋转与缩放/平移共存：单页旋转后仍可缩放拖动；双页旋转后仍可缩放拖动。
- **R6** 翻页后旋转状态保留并应用；退出旋转模式不重置已旋转页面。
- **R7** 条漫模式暂不支持：点击「界面旋转」时提示，不进入旋转模式。

## Acceptance Criteria

- [ ] 右键菜单出现「界面旋转」，进入后单页下方有旋转按钮，点击循环 90° 且方向正确
- [ ] 双页模式两页各有独立按钮，旋转左页不影响右页
- [ ] 旋转后缩放/拖动正常、页面适应窗口
- [ ] 重启应用/重新打开漫画后旋转结果保留（SQLite + library.json 双写，round-trip 测试覆盖）
- [ ] 旧库升级：已有 database.db 自动补 rotations 列（ALTER TABLE），数据不丢
- [ ] `flutter analyze` 0 issues；`cargo test --lib` 通过；模型 JSON round-trip 测试通过

## Out of Scope

- 条漫模式旋转（R7 明确暂不支持）
- 自动方向检测（EXIF 修正可作后续增强）
- 旋转的自定义快捷键
- 裁边、智能拼页（M4 其余部分，另行规划）

## Open Questions

- 无阻塞问题。

## Decisions（2026-08-02）

- 用户明确设计：右键「界面旋转」→ 旋转模式下每页下方独立旋转按钮 → 按页持久化（覆盖原 PRD 的"会话级全局旋转"方案）。
- 持久化采用 `BookMeta.rotations`（Map<int,int>），JSON（library.json）与 SQLite `book_metas.rotations` 列双写；旧库启动时 `ALTER TABLE` 补列。

## Verification（2026-08-02）

- [x] `flutter analyze` 0 issues
- [x] `cargo test --lib` 通过（31 passed，含 `book_metas.rotations` 列测试）
- [x] 模型 JSON round-trip 测试（[app/test/book_meta_rotation_test.dart]）
- [x] 旧库升级：`init_tables` 对已存在表幂等补列（ALTER TABLE）
- [x] FRB 桥接重新生成（BookMetaDto.rotations 贯通 Rust ↔ Dart）
- [ ] `flutter run` 桌面实测（本环境无法启动 GUI，待用户验证：右键入口 / 单页旋转 / 双页独立旋转 / 重启保留）
