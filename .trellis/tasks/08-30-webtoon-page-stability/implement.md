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
- [ ] Real Windows/Android smoke with a 50+ page variable-height webtoon remains pending; keep this task `in_progress` until device validation is recorded.

## 人工关卡与回滚

- [ ] 使用至少 50 页、页高差异明显的条漫，连续快速前进/后退、跳转并重开，确认标题、底部页码和阅读进度一致。
- [ ] 通过后单独提交；若出现回归，仅回滚本子任务，不删除回归测试。
