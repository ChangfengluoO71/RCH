# 条漫快速翻页稳定性与页码回跳修复

## Goal

使条漫模式在图片异步加载、页面高度变化和连续快速翻页同时发生时，始终以最后一次用户导航意图为准，保持视觉位置、标题页码、底部页码与持久化阅读进度一致。

## Confirmed Facts

- 条漫使用 `ListView` 和 `_webtoonHeights` 保存运行时测得的页面高度。[`reader_page.dart`](D:/Projects/RCH-source/app/lib/ui/reader_page.dart:306)
- `_webtoonOffsetTo` 仅累加当前已知高度；快速连续导航时，目标页的偏移可能基于不完整高度计算。
- `_onWebtoonScroll` 会根据视口中心重新设置 `_page`。异步布局、动画和该回调可把刚设置的目标页覆盖为旧页，符合“页码不动后回跳”的反馈。
- 现有 `reader_swipe_webtoon_test.dart` 覆盖条漫手势/缩放，但不覆盖异步测高与快速连续导航的页码稳定性。

## Requirements

- **R1** 所有条漫导航入口（底部前后页、键盘、页码跳转）在连续触发时以最后一次有效目标为准。
- **R2** 未测得或随后变化的页面高度不得将一个已确认的导航目标错误映射回较早页面。
- **R3** 滚动稳定后，AppBar、底部页码、视觉中心页与 `recordRead` 的页码一致。
- **R4** 图片由占位高度切换为真实高度、切换 AI 版本或重新进入条漫时，页码状态不会短暂倒退或写入错误阅读进度。
- **R5** 保持既有单指滚动、双指缩放、日漫/美漫翻页和双页模式行为不变。

## Acceptance Criteria

- [x] 为“部分高度未知 → 连续导航到多个目标页 → 图片高度收敛”的场景加入回归测试；最终页必须等于最后一次导航目标，之后布局回调不得将其回写为旧页。
      （`app/test/webtoon_navigation_test.dart` 的 `WebtoonNavigationModel` 组：`latest programmatic target wins and stale generations are ignored`、`programmatic completion clears only the matching pending target`、`user gesture cancels a pending target without jumping to stale target`）
- [x] 为页码跳转到未测高目标页加入测试；滚动和高度更新后，页码保持目标页或按明确、可解释的稳定映射更新，不能跳回旧目标。
      （`measured heights replace estimates while preserving monotonic offsets`、`fast scroll observes viewport without overwriting stable page until settle`）
- [x] 手工验证至少 50 页、页高明显不均的条漫：连续快速前进/后退和跳转后，标题、底部页码及重开后的阅读进度一致。
      **2026-09-23 手机实机（OPPO PGFM10，0.6.2+102602）：50+ 页不等高条漫快速下拉，原有“划着划着突然跳回好几页前”消失，用户确认。**
- [x] `flutter test test/reader_swipe_webtoon_test.dart`、新增回归测试和 `flutter analyze` 全部通过。

### 收口说明（2026-09-23，第133轮）
- 已报告症状（**快速下拉时可见内容被整体推走**）的实际根因与设计文档的假设**不同**：不是"导航意图被旧回调覆盖"，
  而是 `ListView` 无 `itemExtent` 时，**视口上方**的页由 200px 占位收敛成真实高度、`SliverList` 只保持像素偏移。
  已用 `WebtoonAnchorKeeper`（测高变化时 `position.correctBy` 静默纠偏）+ 阅读器接线修掉，含真实 `ListView`
  对照实验回归（不补偿被推走 5600px / 补偿后与对照组差 <1px）。详见 `docs/project/LOG.md` 第133轮、`TODO.md`。
- **设计文档里的 `WebtoonNavigationModel` 至今未接进阅读器**（全仓仅被自己的单测引用）。它覆盖的是
  "页码回跳 / 阅读进度写错"这一类问题，与本次症状不同，且本次未复现该类问题 ⇒ **未接线**，
  作为独立待办（TODO「步骤②（接通导航模型）」）保留，不随本任务归档关闭。
- 顺带修掉一个真实缺陷：测高回调未查条目自身 `mounted`，sliver 回收视口外子项时会 `findRenderObject()`
  打到 DEFUNCT element。

## Out of Scope

- 改变条漫的视觉设计、滚动手势、图片解码质量或远程书源协议。
- 通过延迟按钮、禁用快速翻页或丢弃用户输入来规避问题。

## Planning Status

需求、边界和验收条件已明确；该任务可在技术设计评审后启动。
