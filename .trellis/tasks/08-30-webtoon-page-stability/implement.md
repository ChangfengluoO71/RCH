# 条漫快速翻页稳定性：实施计划

## 步骤 1：建立可复现回归

- [ ] 在 `app/test/reader_swipe_webtoon_test.dart` 或相邻新测试中，先写“未知高度 → 连续导航 → 高度收敛”的失败用例。
- [ ] 以可控的图片/高度假件记录滚动回调和 `recordRead` 的最终页，避免依赖真实网络或计时偶然性。
- [ ] 记录当前失败表现，作为修复前证据。

## 步骤 2：拆出条漫导航状态

- [ ] 在 `app/lib/ui/reader_page.dart` 引入 generation、pendingTarget 与非零高度估算，不改变其他阅读模式。
- [ ] 将 `_go`、页码跳转和相关按页入口收敛到同一导航方法。
- [ ] 让 `_onWebtoonScroll` 在导航会话期间只记录观察值；导航稳定或用户手势取消后才提交页码。
- [ ] 将阅读进度写入移到统一提交点，并避免旧异步回调覆盖新目标。

## 步骤 3：布局收敛与交互回归

- [ ] 高度更新后只校正最新目标，且校正次数有界。
- [ ] 验证快速前进/后退、页码跳转、键盘、图片加载、AI 版本切换和重入条漫。
- [ ] 验证手势滚动可自然更新当前页，且不会被已取消的导航重新覆盖。

## 验证命令

```powershell
cd app
flutter test test/reader_swipe_webtoon_test.dart
flutter test
flutter analyze
```

## Execution update (2026-09-11)

- [x] Added `WebtoonNavigationModel` with stable-page, viewport-page, pending-target, generation-token, monotonic estimated offsets, measured extents, and stale-intent rejection.
- [x] Reader integration commits reading progress only after matching programmatic completion or settled user scroll; a user drag cancels a pending target.
- [x] Fixed the `animateTo`/`ScrollEndNotification` race so the notification cannot clear a live programmatic intent before its future completes.
- [x] Automated verification: `flutter test --no-pub test/webtoon_navigation_test.dart test/reader_swipe_webtoon_test.dart` passed (30 tests); `flutter analyze --no-pub` passed.
- [x] Real Windows/Android smoke with a 50+ page variable-height webtoon remains pending; keep this task `in_progress` until device validation is recorded.

## Execution update (2026-09-23, 第133轮)：已报告症状的真正根因与修复

- **用户现象**：手机端条漫**快速下拉**时"划着划着突然跳回好几页前"。
- **真正的根因（与 2026-09-11 的假设不同）**：`ListView` 无 `itemExtent`，未加载页按 200px 占位；
  当**视口上方**的页换成真实高度（常 1000–4000px）时，`SliverList` 只保持像素偏移 ⇒ 可见内容被整体推走。
  与"导航意图被旧回调覆盖"无关（该模型的 stale-intent 机制本次未参与）。
- **修复**：`WebtoonAnchorKeeper`（仅当条目"旧底边仍在视口顶边之上"时按高度变化量补偿；
  `announceGrowth()` 在字节到达时告知占位高度，覆盖"视口上方的页从未被构建过"这一形状）
  ＋ 阅读器接线（`position.correctBy` **静默**纠偏，不打断快速下拉惯性；`animateTo` 期间暂停补偿）
  ＋ 顺带修掉测高回调打到 DEFUNCT 元素的真实缺陷（补条目自身 `mounted` 守卫）。
- **回归**：`app/test/webtoon_navigation_test.dart` 用真实 `ListView` 做对照实验（同一拖拽轨迹、有/无增长）：
  有补偿时与对照组锚点差 **<1px**；不补偿时被推走 **5600px**（钉住根因）。
- **实机验证（本任务的人工关卡，已完成）**：OPPO `PGFM10`，release 包 `0.6.2+102602`；
  50+ 页不等高条漫快速下拉 ⇒ **跳变消失，用户确认**。
- **未随本任务关闭**：`WebtoonNavigationModel` **仍未接进阅读器**（全仓仅被自己的单测引用）。
  它覆盖"页码回跳 / 阅读进度写错"，本次未复现该类问题 ⇒ 转独立待办（见 `docs/project/TODO.md`
  「步骤②（接通导航模型）」）。

## 人工关卡与回滚

- [ ] 使用至少 50 页、页高差异明显的条漫，连续快速前进/后退、跳转并重开，确认标题、底部页码和阅读进度一致。
- [ ] 通过后单独提交；若出现回归，仅回滚本子任务，不删除回归测试。
