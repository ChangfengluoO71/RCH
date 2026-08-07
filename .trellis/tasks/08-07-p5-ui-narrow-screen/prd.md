# M6 窄屏 UI 适配（手机宽度）

## Goal

让 RCH 现有 UI 在手机宽度（逻辑宽约 360–411dp）下可用：不横向溢出、可滚动、可触控；主界面从桌面式"固定 230px 侧栏 + 内容区"改为响应式布局，同时保证 Windows 桌面端回归不破坏。范围 = 全部页面一起改（用户确认 2026-08-07）。

## Confirmed Facts（代码盘点 2026-08-07，模拟器基线 1080x2400 @ 420dpi ≈ 411dp 宽）

- 主壳为桌面布局：`Scaffold(body: Row([SizedBox(width: 230, 侧栏), VerticalDivider, Expanded(内容)]))`（home_page.dart:103-120；home_page.dart:478-492 标签详情也并列右侧），手机 360dp 宽下内容区仅剩约 100dp。
- 设置面板位于主壳右侧内容区，含固定宽度 Slider 行（`SizedBox(width: 120, Slider)`，home_page.dart:584）、Dropdown 行、快捷键绑定行（home_page.dart:577-596），窄屏会溢出/难操作。
- AddSourceDialog 内容固定 `SizedBox(width: 420)`（home_page.dart:813）；手机 AlertDialog 可用宽度约 320dp，超宽溢出。WebDAV 连接表单是 `Center + ConstrainedBox(maxWidth: 420)`（webdav_page.dart:136-140），窄屏会自然收缩，问题较小。
- 详情页为左右分栏：`Row(封面列 + VerticalDivider + Expanded(元数据/标签/简介))`（book_detail_page.dart:276-318），窄屏右侧被压到很窄。
- 书架/搜索结果 GridView 已用 `SliverGridDelegateWithMaxCrossAxisExtent(maxCrossAxisExtent: 180)`（home_page.dart:291/333/522；library_page.dart:120），列数自适应；封面渲染固定 340x480（library_page.dart:58）需按密度缩放。
- 阅读器已有 LayoutBuilder + PhotoView + 左右 80dp 点按区（reader_page.dart:329-330, 348-366），基础可用；阅读设置走底部弹层（reader_page.dart:446），方向正确。
- 全库几乎没有 MediaQuery/LayoutBuilder/断点体系（除 reader、cover_editor），缺少统一响应式基线。
- Windows 桌面布局不得回归（M6 设计第 1 节约束）。

## Key Decisions

- **断点 = 600dp**（Material compact/medium 边界）：< 600dp 走移动壳；≥ 600dp 保持现有侧栏布局（Windows 窗口缩窄到 600dp 以下时同样受益）。
- **移动壳导航 = 底部导航**（书架/标签/书源/设置 4 个目的地），搜索移入书架 AppBar；宽屏布局代码路径保持不变。
- **最小支持宽度 = 360dp**，320dp 尽力兼容（不保证）。
- **对话框统一策略**：内容 `ConstrainedBox(maxWidth: min(420, 屏宽-48))`，替代固定 `SizedBox(width: 420)`；宽屏观感不变。
- **列表页竖屏锁定、阅读器允许旋转**（沿用 M6 设计第 5 节）。

## Requirements

- R1 主壳响应式：≥ 600dp 保持侧栏布局；< 600dp 切换移动壳（底部导航 + AppBar），内容不被 230px 侧栏挤压。
- R2 全部页面（主壳、书架、详情、阅读器、设置、书源、WebDAV、缓存、同步、封面编辑、标签管理）在 360dp 宽下无横向溢出、可滚动、可操作。
- R3 对话框/表单统一最大宽度适配，窄屏不溢出、宽屏观感不变。
- R4 触控目标 ≥ 48dp（图标按钮/列表项/阅读器点按区），与 P1 触屏方案衔接。
- R5 SafeArea/insets 处理（刘海、手势条）；列表页竖屏锁定、阅读器允许旋转。
- R6 系统字体缩放 1.0–1.3 下主要页面不截断关键内容。
- R7 阅读器内 AI 超分入口在 Android 隐藏（与 p1 衔接，不重复实现）。

## Acceptance Criteria

- [ ] 模拟器（411dp）与 ≥360dp 窄屏下：全部页面无横向溢出，内容可达、可操作。
- [ ] 主壳 < 600dp 切换移动导航；≥ 600dp 保持侧栏布局（Windows 桌面回归不变）。
- [ ] 所有 AlertDialog/表单在 360dp 宽下可完整显示与操作。
- [ ] 主要操作按钮/列表项触控目标 ≥ 48dp；系统返回、横竖屏切换不丢状态。
- [ ] 字体缩放 1.3 下主要页面无关键内容截断。
- [ ] Windows 桌面回归通过：`cargo test` + `flutter analyze` + Windows 构建。

## Out of Scope

- 平板/大屏专属布局（宽度 ≥ 600dp 沿用现有桌面布局）。
- 阅读器触屏手势本身、AI 超分入口隐藏（由 p1-local-reader 处理，本任务仅验收衔接）。
- SAF 文件选择/目录迁移（由 p1-local-reader 处理）。
- 无障碍语义完整化、深色主题微调（除非顺带修复）。

## Dependencies

- 前置：p0-android-buildchain（已完成，APK 已跑通）。
- 并行：p1-local-reader（阅读器手势、AI 隐藏、SAF 导入）。
- 父任务：08-04-m6-android。

## Risks & Deferred

- 主壳改造回归面大 → 宽屏分支保持原代码路径，改动隔离在 compact 分支。
- 对话框/设置改动影响 Windows → 用 maxWidth 上限而非改变宽屏样式，Windows 构建+人工回归把关。
- 阅读器相关窄屏微调若与 p1 冲突，以 p1 为准并在本任务验收。
