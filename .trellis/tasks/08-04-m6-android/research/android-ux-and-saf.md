# 安卓触屏与文件访问调研

## 触屏交互差距

- 现状桌面依赖:`reader_page.dart` 的 `Focus` + `onKeyEvent`(键盘翻页/缩放)、`onSecondaryTapUp`(右键菜单);home_page 有快捷键设置。
- Android 适配:点按区域翻页、双指缩放(photo_view 已用)、长按弹菜单、`PopScope` 处理返回、`MediaQuery.padding` / SafeArea 处理刘海与手势条。
- 键盘逻辑保留:Android 外接键盘 / 模拟器仍可用。

## SAF 与 file_selector

- `file_selector` 在 Android 支持 `openFile` / `openFiles`(走 SAF,返回可读源);`getSaveLocation` / `getDirectoryPath` 在 Android 不受支持。
- 因此:导入走 openFiles → 复制进应用私有目录;导出 CBZ 走系统分享(share_plus / MediaStore)。
- 外部目录书架(SAF tree + persistable permission)不在首版(用户决策 A),后续版本再评估。

## 数据目录现状(可复用)

- `cache_root_marker.dart` + `setCacheRootPath` + 迁移机制(含 main.dart 的 `_healDatabaseLocation`)已支持自定义根;Android 首版固定为应用私有目录,不暴露自定义/迁移入口。
- 远程书源配置(SQLite + JSON 备份)与 Windows 同构,可直接复制。
