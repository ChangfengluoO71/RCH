# Implement — 使用反馈修复执行计划

> 顺序：R2（书源顶栏，最小改动）→ R1（条漫页码，逻辑较多）→ R3（PC 图标，需生成素材+重建）。
> 每步完成即静态复核 + 局部验证，最后统一 `flutter analyze` + 测试 + 构建。

## Step 1 — R2 书源顶栏与状态栏重叠

- 文件：`app/lib/ui/source_browser.dart`
- 改动：`Scaffold(body: SafeArea(child: ListenableBuilder(...)))`，SafeArea 默认顶部+底部避让；`_convertProgressBar` 的 `Positioned(bottom:0)` 随 SafeArea 上移，符合预期。
- 复核点：
  - <600dp 宿主（home 有 AppBar）：SafeArea 顶部 padding=0，无视觉变化；
  - ≥600dp 宿主（home 无 AppBar）：顶栏下移避让状态栏。
- 验证：`flutter analyze`；MuMu/平板布局截图核对顶栏位置（如环境允许）。

## Step 2 — R1 条漫模式页码滚动跟随

- 文件：`app/lib/ui/reader_page.dart`
- 改动：
  1. 新增 `final List<double> _webtoonHeights = [];` 页高缓存；
  2. `_buildWebtoon()` 的 itemBuilder：每页外包 `LayoutBuilder`，渲染后记录高度到 `_webtoonHeights[i]`（按 index 补齐）；
  3. `_webtoonCtrl.addListener`：累计高度定位视口页（视口中心对应页），页码变化才 `setState(() => _page = p)`；
  4. 复用现有 `pageLabel`（AppBar 标题页码自动跟随）。
- 复核点：
  - 不打断点击记录进度（onTap 仍更新 `_page`）；
  - 滚动监听与 onTap 的 `_page` 更新路径不冲突（同值不重复 setState）；
  - `_webtoonHeights` 在 `_recreatePageCtrl`/模式切换时重置。
- 验证：`flutter analyze`；`flutter test test/reader_swipe_webtoon_test.dart`；实机滚动核对页码。

## Step 3 — R3 PC 端应用图标（紫底白字 RCH）

- 素材：以 Android `mipmap-xxxhdpi/ic_launcher.png`（紫底白字 RCH）为参考，生成同风格 1024px 源图 → 多尺寸 `.ico`。
- 改动：
  1. 生成 `app/windows/runner/resources/app_icon.ico`（16/24/32/48/64/128/256）；
  2. （可选）`app/windows/installer/setup.iss` 增加 `SetupIconFile=runner\resources\app_icon.ico`（相对路径按实际调整）→ 安装包/卸载图标一致；
  3. 无需改 `Runner.rc`（已引用 `resources\app_icon.ico`）。
- 验证：Windows Release 构建（工具环境 cl.exe 挂起时用 WMI 逃逸，见 SETUP.md）；运行后任务栏/窗口图标为新图标；Inno 打包产物图标生效。

## Step 4 — 总体验证与收尾

- `flutter analyze` 0 issues；`flutter test` 全过（含 reader_swipe_webtoon_test）。
- `git diff` 复核为最小修改；更新 LOG.md / LOG-INDEX.md / TODO.md。
- 交付用户验证：三个反馈逐项复测（条漫页码、书源顶栏、PC 图标），确认后更新 README（如需）并归档任务。
