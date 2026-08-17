# 使用反馈修复：条漫页码 / 书源顶栏重叠 / PC 端图标

> 来源：飞书群「RCH项目组」2026-08-17 长风落反馈（@助手）。
> 三个反馈相互独立，合并为一个任务逐项修复、逐项验证。

## Goal

1. 条漫模式滚动阅读时能显示当前页码（滚动跟随，不再是「点击才更新」）。
2. 书源浏览界面最上面一层不再与手机状态栏（时间显示层）重叠。
3. PC 端应用图标更换为与手机一致：紫底白字「RCH」三个字母。

## Constraints

- 阅读器（`app/lib/ui/reader_page.dart`）与书源浏览器（`app/lib/ui/source_browser.dart`）改动遵循最小修改原则，不动已有功能与数据流。
- 条漫页码计算必须**纯前端本地计算**，不引入 Rust 改动、不改页码语义（`_page` 仍为真实页索引）。
- 书源顶栏修复必须同时兼容两种宿主布局：宽度 <600dp（home 有 AppBar，顶栏本就不重叠，SafeArea 不应产生视觉差异）与 ≥600dp（home 无 AppBar，顶栏需避让状态栏）。
- PC 图标必须生成标准多尺寸 `.ico`（至少 16/24/32/48/64/128/256），被 `windows/runner/Runner.rc` 引用；不改变 Android 图标。
- 若 Windows 构建受工具环境 cl.exe 挂起影响，沿用 SETUP.md 记录的 WMI 逃逸方式构建验证。

## Requirements

- R1 条漫页码：
  - 条漫模式下滚动 ListView 时，AppBar 标题中的页码（`x / N`）随视口实时更新；
  - 需缓存各页实际渲染高度（图片高度不同，不能假设固定 itemExtent），按滚动偏移定位视口页；
  - 页码变化才 setState，避免滚动期间高频重建影响性能；
  - 双击缩放 / 手势缩放状态下页码定位不受影响（按滚动偏移计算即可）。
- R2 书源顶栏重叠：
  - 根因：`source_browser.dart` 的 Scaffold 无 AppBar，自定义 `Material+ListTile` 顶栏画在状态栏之下；宽度 ≥600dp 时 home 无 AppBar 故无兜底避让；
  - 修复：内部 Scaffold 的 body 顶部（及底部进度条区域）包 SafeArea，或等价的最小改动；
  - 验收：Android 上（尤其 ≥600dp 宿主/平板布局）书源界面顶栏完整显示在状态栏下方，按钮可点、无遮挡。
- R3 PC 图标：
  - 生成紫底白字「RCH」多尺寸 `app_icon.ico`（参考 Android `mipmap-*` 图标风格）；
  - 替换 `app/windows/runner/resources/app_icon.ico`；
  - （可选）安装器 `setup.iss` 增加 `SetupIconFile` 使安装包与卸载程序图标一致；
  - 重新构建 Windows Release 验证任务栏 / 窗口 / 快捷方式图标生效。

## Acceptance Criteria

- [ ] 条漫模式滚动时 AppBar 页码实时跟随视口，点击跳转/记录阅读进度行为不变（既有测试 `test/reader_swipe_webtoon_test.dart` 仍通过）。
- [ ] Android（含平板布局）书源浏览界面顶栏与状态栏无重叠，返回/上级/刷新等按钮全部可点。
- [ ] Windows Release 构建后任务栏与应用窗口显示新图标（紫底白字 RCH）；安装包图标（若配置）同步生效。
- [ ] `flutter analyze` 0 issues；改动文件经 `git diff` 复核为最小修改。
