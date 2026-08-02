# 修复阅读器缩放后移动区域只在第一页生效
## Goal

修复阅读器 Bug：缩放后拖动查看细节只在第一页生效，翻页后无法拖动。目标是任意页缩放后都能正常拖动。

## Background（已确认事实）

- 阅读器使用单个共享 PhotoViewController（`_photoCtrl`）[app/lib/ui/reader_page.dart:26]
- 单页渲染 `PhotoView(controller: _photoCtrl, ...)`（minScale=contained, maxScale=covered*8）[app/lib/ui/reader_page.dart:217]
- 缩放实现：`_zoomBy` 直接修改 `_photoCtrl.scale`（clamp 0.5~8.0）；双页/条漫用各自 TransformationController [app/lib/ui/reader_page.dart:148-153]
- 翻页 `_go` 中调用 `_photoCtrl.reset()` + 双页矩阵复位 [app/lib/ui/reader_page.dart:159]
- 页面左右 80px 覆盖层 GestureDetector（HitTestBehavior.opaque，onTap 翻页）[app/lib/ui/reader_page.dart:228-234]
- Bug 现象：第 1 页缩放后可拖动；翻页后 pan 失效（用户反馈）
- **根因（已通过 widget 回归测试验证）**：`_go()` 只调用 `_photoCtrl.reset()`，而 PhotoView 0.15 内部的 `PhotoViewScaleStateController` 停留在 zoomedIn。翻页时若下一页图片已预载（`_go` 预取 ±2 页，属常态），PhotoView 元素存活、图片更换使 scale boundaries 变化，但 `PhotoViewCore.scale` 因 scaleState 处于 zooming 状态跳过重算，新页沿用上一页缩放值；第一页状态全新（initial），因此缩放/拖动只在第一页正常。
- 修复：自持 `PhotoViewScaleStateController`，翻页/跳转/0 键复位/版本切换时与 `_photoCtrl` 一起重置（[app/lib/ui/reader_page.dart]）。

## Requirements

- **R1** 任意页缩放后均可拖动查看图片（pan 正常）。
- **R2** 翻页后的缩放状态行为明确：推荐翻页保留缩放状态（0 键/快捷键复位）；若实现困难，翻页复位后也必须能再次缩放并拖动。
- **R3** 双页拼接模式与单页模式行为一致；条漫模式不受影响。
- **R4** 左右 80px 边缘点击翻页仍可用，不与拖动冲突。
- **R5** 缩放边界（min/max）与现在一致，不做手势行为回归。

## Acceptance Criteria

- [ ] 第 N 页（N>1）缩放 2x 后可拖动查看图片四角
- [ ] 第 1 页 → 第 2 页翻页后，缩放与拖动均正常（或明确按 R2 决策行为）
- [ ] 双页模式缩放后拖动正常
- [ ] 边缘 80px 区域点击翻页正常，不影响中间区域拖动
- [ ] `flutter analyze` 0 issues；`flutter run` 实测通过

## Out of Scope

- 滚轮缩放（已从 backlog 删除，明确不做）
- 双指旋转、手势惯性等增强
- 条漫模式改动（仅确认不受影响）

## Open Questions

- 无阻塞问题。

## Decisions（2026-08-02）

- **R2 决策：翻页复位缩放**。PRD 原推荐"翻页保留缩放"，但经代码验证，photo_view 0.15 的 scaleState 状态机在跨页保留缩放下会与 `markNeedsScaleRecalc` 冲突（换图后无法按新页可靠重算），保留方案不可靠；采用 PRD 允许的备选：每次翻页回到"适应窗口"，0 键/快捷键复位。若用户后续希望保留缩放，需另建任务。
- 双页模式翻页时 `_dualZoomCtrl` 保持现有复位行为；条漫模式不受影响。
- **双页模式独立根因（2026-08-02 补充）**：`_buildPair` 的 InteractiveViewer 原实现 `panEnabled: false`，键盘 +/- 放大后完全无法拖动。已改为 `panEnabled: true`（保持 `scaleEnabled: false`，捏合缩放维持现状）。

## Verification（2026-08-02）

- [x] `flutter analyze` 0 issues
- [x] 新增回归测试 [app/test/reader_zoom_reset_test.dart]，实证：只重置 photoCtrl 时翻页后沿用旧页缩放（旧 Bug）；photoCtrl + scaleState 一起重置后回到新页 contained
- [x] 双页模式回归测试：InteractiveViewer 缩放后拖动有效（`panEnabled: true`）
- [ ] `flutter run` 桌面实测（当前环境无法启动 GUI，待用户验证）
