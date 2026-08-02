# Component Guidelines

> How components are built in this project.

---

## Overview

<!--
Document your project's component conventions here.

Questions to answer:
- What component patterns do you use?
- How are props defined?
- How do you handle composition?
- What accessibility standards apply?
-->

(To be filled by the team)

---

## Component Structure

<!-- Standard structure of a component file -->

(To be filled by the team)

---

## Props Conventions

<!-- How props should be defined and typed -->

(To be filled by the team)

---

## Styling Patterns

<!-- How styles are applied (CSS modules, styled-components, Tailwind, etc.) -->

(To be filled by the team)

---

## Accessibility

<!-- A11y requirements and patterns -->

(To be filled by the team)

---

## Common Mistakes

<!-- Component-related mistakes your team has made -->

- **photo_view 0.15 共享 controller 翻页后缩放失效**：翻页时只 `reset()` `PhotoViewController` 不够——内部 `PhotoViewScaleStateController` 会残留 zoomedIn，换图后 `PhotoViewCore` 跳过缩放重算（`markNeedsScaleRecalc` 仅在非 zooming 状态生效），新页沿用上一页缩放。必须在翻页/跳转/复位时同时 reset 两个 controller（参见 `app/lib/ui/reader_page.dart` 的 `_go`/`_zoomReset`）。
- **InteractiveViewer 双页模式拖动失效**：阅读器双页拼接的 `InteractiveViewer` 曾写死 `panEnabled: false`，键盘缩放后无法拖动查看细节。启用 pan 时必须确认缩放矩阵的锚点与边界钳制（原点锚定缩放时只能朝一个方向拖动），`panEnabled` 不应与缩放功能耦合关闭。

(To be filled by the team)
