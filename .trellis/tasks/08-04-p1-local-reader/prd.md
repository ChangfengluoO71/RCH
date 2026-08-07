# P1 本地阅读闭环(触屏适配)

## Goal

手机 / 平板上可完整使用"本地漫画阅读闭环":导入 → 书架 → 详情 → 阅读 → 进度记忆,交互全部触屏化。

## Requirements

- 阅读器触屏适配:点按区域翻页、双指缩放(photo_view 已有)、长按弹出操作菜单(替代右键)、系统返回键(PopScope)、横竖屏切换、SafeArea / insets。
- 本地格式:ZIP/CBZ、EPUB、文件夹、CB7、CBT、MOBI(PDF/RAR 由 p3-native-formats 合入后验收)。
- 数据目录:应用私有目录 + SAF 导入复制(file_selector 的 openFile/openFiles),缓存 / 进度 / 标签 / 书源索引落库。
- AI 超分入口在安卓端隐藏 / 禁用(桌面端不变)。
- 现有键盘快捷键逻辑保留但非主交互。

## Acceptance Criteria

- [ ] 真机上通过 SAF 导入一本 CBZ → 书架出现 → 打开阅读 → 翻页 / 缩放 / 进度记忆 → 重开续读。
- [ ] 长按弹出操作菜单,且不出现 AI 超分入口。
- [ ] 横竖屏切换不丢阅读状态;系统返回行为正确。
- [ ] EPUB / 文件夹 / CB7 / CBT / MOBI 各至少一本真机可读。
- [ ] `flutter analyze` 与 Rust `cargo test`(Windows 回归)通过。

## Dependencies

- 前置:p0-android-buildchain。
- PDF/RAR 验收依赖 p3-native-formats 合入。
