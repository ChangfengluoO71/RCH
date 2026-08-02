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

- **R1** 单页 / 双页 / 条漫三种模式均支持 90° 顺时针、90° 逆时针、180° 旋转（工具栏按钮；建议顺带加入快捷键动作）。
- **R2** 旋转与缩放/平移共存：旋转后仍可缩放拖动（PhotoView 变换与图片旋转叠加）。
- **R3** 旋转状态范围（推荐）：会话级全局旋转——翻页保留、退出阅读器复位；不按页独立记忆（MVP 简化）。
- **R4** 旋转后页面适应窗口（contain 布局仍成立，不留大片黑边）。
- **R5** 不改变已读记录/进度等数据模型。

## Acceptance Criteria

- [ ] 三种模式旋转 90°/180° 后页面方向正确、适应窗口
- [ ] 旋转后缩放 2x 并可拖动
- [ ] 翻页后旋转状态保持；退出重进复位
- [ ] 快捷键可触发旋转（绑定面板可见）
- [ ] `flutter analyze` 0 issues；`flutter run` 实测

## Out of Scope

- 裁边、智能拼页（M4 其余部分，另行规划）
- 按页独立旋转记忆、旋转持久化到全局设置
- 自动方向检测（EXIF 自动修正可作后续增强）

## Open Questions

- 无阻塞问题。R3 若用户希望旋转持久化到设置，可在验收后追加小改动。
