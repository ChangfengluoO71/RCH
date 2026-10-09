# RCH 外观设置与平台材质设计

状态：待用户审阅

## 目标

参考用户提供的外观设置截图，整理 RCH 的外观设置，让配色、字体和显示选项更容易浏览；在保留现有配色与字体的基础上，补充跟随系统亮度、Android 系统动态配色，以及 Windows 云母和亚克力材质。

改动只涉及 RCH 界面和窗口表面，不改变漫画图像、阅读画布、书籍元数据或阅读进度。

## 用户需求与约束

- 设置行应有清楚的标题、简短说明和靠右对齐的控件，分组之间留出稳定间距；窄屏和桌面窗口都不能拥挤或裁切。
- 保留当前四种配色及四种字体。配色和字体预览应帮助用户判断效果，正文文字仍须清晰可辨。
- 亮度支持深色、浅色和跟随系统。继续以深色作为新装和旧设置的默认值。
- Android 12 及以上可以选择系统动态配色；其他平台或系统不提供动态配色时，使用用户保存的 RCH 固定配色。
- Windows 11 22H2 及以上可以选择主窗口云母背景和弹窗亚克力效果。其他系统和平台继续显示标准材质。
- 配色、亮度、字体和材质分别保存，只保存在本机应用设置中，不扩展到书籍元数据或同步数据。

## 设计方案

### 外观设置布局

沿用现有“外观与布局”设置入口，不重排其他设置类别。将外观相关控件整理为“外观”“显示”“窗口”小节，并使用轻微的主题色面板承载相关行。每行采用一致的内边距和间距，包含图标、标题、可选的一行说明及右侧选择器。

配色预览同时展示主色、背景和文字样例，字体选择展示所选字体的实际样例。所有颜色从当前 `ColorScheme` 和其容器角色取值；不依赖低对比度的装饰色，普通字号文字保持至少 4.5:1 的对比度。布局只改造外观设置，不做全应用设置页重排。

### 亮度和配色

- 将 `themeMode` 扩展为 `dark`、`light`、`system`；未知值回退为 `dark`，既有 `light` / `dark` 值保持原义。
- 增加本机设置 `useSystemDynamicColors`，默认 `false`。仅 Android 12+ 显示“RCH 配色 / 系统动态”来源选项。
- 使用 Flutter Material Foundation 的 `dynamic_color` 包获取 Android 提供的明暗 `ColorScheme`。仅在 Android 使用该值，不能把该包在其他桌面系统上提供的系统强调色误当作 Android 壁纸动态配色。
- 开启系统动态配色后，系统提供的配色覆盖当前主题的颜色方案，但保留用户所选固定 RCH 配色作为回退值；动态色不可用时不清空、不重写固定配色设置。
- 字体仍独立于配色和亮度。动态色方案继续使用 RCH 现有的语义色角色和文字可读性处理。

### Windows 主窗口云母

- 增加 `windowMaterial`，取值为 `standard` 或 `mica`，默认为 `standard`。
- Windows 11 22H2（build 22621）及以上使用 Win32 DWM `DWMWA_SYSTEMBACKDROP_TYPE` 配置 `DWMSBT_MAINWINDOW`。Flutter 主内容背景在启用云母时允许显示原生窗口背景，卡片、输入框等交互表面仍使用可读的 Material 3 色阶。
- Windows 原生窗口通过一个窄范围的 MethodChannel 接收“查询能力”和“设置主窗口材质”请求。DWM 不支持或调用失败时清除系统背景并回到标准色面；Flutter build 过程不直接发起平台 I/O。
- Windows 设置中显示材质可用状态和最低系统版本提示。Windows 10 或更早的 Windows 11 版本不能选择云母；若设置从其他环境恢复为 `mica`，运行时仍回退为标准背景。
- Android、Linux、macOS 不显示此项，也不改变其窗口背景。

### Flutter 弹窗亚克力

- 增加 `overlayMaterial`，取值为 `standard` 或 `acrylic`，默认为 `standard`。
- 亚克力使用 Flutter `BackdropFilter` 加有色半透明表面实现，因为 RCH 的对话框和模态底部面板位于现有 Flutter 窗口内，并非独立的 Win32 瞬态窗口。
- 本阶段将亚克力应用到 RCH 自有的 `AlertDialog` 和模态底部面板；锚定式下拉框、上下文菜单、系统文件选择器继续使用标准材质。
- 模糊必须限制在弹窗或面板的裁剪范围内，不模糊漫画网格、整页内容或阅读画布。表面不透明度和前景色需保证文字及控件可辨。
- Flutter 报告高对比度模式时强制使用标准材质；材质设为“标准”时不创建滤镜。该选项只在受支持的 Windows 上显示。

## 设置持久化与兼容

在现有 `AppSettings` JSON 读写中增加 `useSystemDynamicColors`、`windowMaterial` 和 `overlayMaterial`。缺失值使用上述默认值；未知枚举值分别回退为 `false`、`standard`、`standard`。`themeMode` 继续缺省为 `dark`。所有设置留在现有本机设置存储中，不写入 `BookMeta`、`MetaSyncRow` 或书籍同步记录。

## 涉及模块

- `app/lib/store/models.dart`：设置字段、JSON 默认值及兼容处理。
- `app/lib/theme/app_theme.dart`：明暗和动态 `ColorScheme` 的构造，材料相关主题表面。
- `app/lib/main.dart`：系统亮度、动态色方案的接入，以及窗口材质设置的生命周期应用。
- `app/lib/ui/home_page.dart`：外观设置分组、配色预览、字体样例和平台材质选项。
- `app/lib/ui/` 与 `app/lib/store/` 中的对话框入口：使用共享的亚克力弹窗/模态面板呈现器，集中控制透明表面和模糊范围。
- `app/windows/runner/`：Windows 11 DWM 能力查询和主窗口云母调用。
- `app/pubspec.yaml` 与锁文件：添加兼容当前 Flutter SDK 的 `dynamic_color` 依赖。

## 验收标准

- 外观设置在窄屏和桌面窗口都能完整显示；配色预览、字体样例与实际应用主题一致。
- 深色、浅色和系统亮度模式立即生效并跨重启保存；旧设置继续默认深色。
- Android 12+ 开启动态色时使用系统提供的明暗配色；关闭、旧版 Android 或平台不可用时使用已保存的 RCH 配色。
- Windows 11 22H2+ 可启用主窗口云母；不支持系统、原生调用失败和旧设备都呈现标准背景。
- Windows 上启用亚克力只模糊 RCH 对话框和模态底部面板；阅读图像与漫画封面不被滤镜影响；高对比度模式和标准材质下不启用模糊。
- 新设置经过 JSON 保存、读取和缺省处理后保持稳定，不改变同步数据结构。
- 现有四种配色、四种字体以及阅读画布的独立外观继续可用。

## 不在本阶段

- 重做整个设置页或其他应用页面。
- 为 Android 添加云母/亚克力名称或伪装成 Windows 材质的模糊效果。
- 把亚克力扩展到锚定菜单、下拉框、系统选择器、漫画海报墙或阅读图像。
- 添加用户自定义颜色、壁纸背景、动态字体或在线字体下载。

## 技术参考

- [Windows 云母材质指南](https://learn.microsoft.com/en-us/windows/apps/design/style/mica)
- [Windows 亚克力材质指南](https://learn.microsoft.com/en-us/windows/apps/design/style/acrylic)
- [DWM_SYSTEMBACKDROP_TYPE](https://learn.microsoft.com/en-us/windows/win32/api/dwmapi/ne-dwmapi-dwm_systembackdrop_type)
- [Flutter BackdropFilter](https://api.flutter.dev/flutter/widgets/BackdropFilter-class.html)
- [Android 动态颜色](https://developer.android.com/design/ui/mobile/guides/styles/color?hl=en)
- [dynamic_color Flutter 包](https://pub.dev/packages/dynamic_color)
