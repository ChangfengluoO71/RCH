# 修复标签持久化 — 实施计划

## 前置

- [ ] 提交当前工作区基线（AI 功能 + `_tagId` 修复），作为回滚安全网（需用户确认后执行）

## 实施步骤（按序，每步验证）

1. **TagRepository 增量入口**：新增 `persistBookLinks(bookKey)`，复用 `dbEnsureTag` + `dbLinkTag`；`cargo check` / `flutter analyze` 通过。
2. **recordRead 异步化**：签名改 `Future<void>`；await 轻量写；移除 `_saveJsonBackup()` 调用。
3. **调用点 await**：[opener.dart:66](C:/Users/cfl/Desktop/RCH/app/lib/ui/opener.dart:66)、[reader_page.dart:87](C:/Users/cfl/Desktop/RCH/app/lib/ui/reader_page.dart:87)、[reader_page.dart:201](C:/Users/cfl/Desktop/RCH/app/lib/ui/reader_page.dart:201)。
4. **全量保存防抖**：`saveToDisk()` 防抖（~800ms）+ `flushPendingSave()`；`_save()` SQLite 失败 rethrow + 记录，JSON best-effort 不 rethrow。
5. **生命周期 flush**：`RchApp` 挂 `AppLifecycleListener` → `flushPendingSave()`。
6. **`_upscaleAll` await**：[book_detail_page.dart:119](C:/Users/cfl/Desktop/RCH/app/lib/ui/book_detail_page.dart:119) 加 await + 失败 SnackBar。
7. **一次性对账**：`load()` SQLite 分支末尾补缺（tags/book_tags/records）+ `json_reconcile_done` 标记。
8. **验证**：
   - `flutter analyze` 0 issues；`cargo test --lib` 通过；
   - 手工：读一页 → 立即退出 → 重启（`已读` + 最后进度在）；
   - 手工：整本超分完成 → 立即退出 → 重启（`AI超分` 标签在）；
   - 手工：删除标签 → 重启不复活；重命名/批量打标签 → 重启保留；
   - 手工：翻页流畅度无可见劣化；
   - R5 回归：2x 传参、AI超分红色元数据标签、阅读器自动查缓存、取消超分、阅读未超分版本。
9. **收尾**：勾选 PRD 验收项；提交；按 Trellis 流程归档/记录 session。

## 回滚点

- 步骤 3 后：恢复 3 个调用点为 fire-and-forget + 恢复 `_saveJsonBackup()`。
- 步骤 4-5 后：移除防抖与生命周期挂载即可回到"每次全量写"（现状）。
- 步骤 7 后：删除标记 key 即重新触发对账（幂等可重复执行）。

## 风险文件

- `app/lib/store/library_store.dart`
- `app/lib/repository/tag_repository.dart`
- `app/lib/ui/reader_page.dart`、`opener.dart`、`book_detail_page.dart`
- `app/lib/main.dart`（生命周期挂载）
