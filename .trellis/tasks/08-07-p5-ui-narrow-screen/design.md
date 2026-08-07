# M6 窄屏 UI 适配 — 技术设计

## 1. 响应式基线

- 断点：`MediaQuery.sizeOf(context).width < 600` = compact（移动壳）；≥ 600 = wide（现有桌面布局）。≥600dp 时 `AppSettings.tabletLayout` 可强制 mobile（手机壳）或 desktop/auto（桌面侧栏），方便平板用户按横/竖屏习惯选择；方向锁只在 compact 布局生效（home_page 随布局切换，reader 退出时按进入时布局恢复）。
- 共享工具放 `lib/ui/common.dart`：
  - `bool isCompact(BuildContext)` — 断点判断。
  - `double dialogMaxWidth(BuildContext)` = `min(420, screenWidth - 48)`，供所有对话框内容 `ConstrainedBox` 使用。
- 原则：wide 分支尽量不改动原 widget 树，所有适配走 compact 分支，降低 Windows 回归面。

## 2. 主壳（home_page.dart）

现状：`Scaffold(body: Row([SizedBox(width:230, _buildSidebar()), VerticalDivider, Expanded(_buildContent())]))`。

compact 分支：
- `Scaffold(appBar: AppBar(搜索入口 + 标签筛选入口), body: _buildContent(), bottomNavigationBar: NavigationBar)`。
- 底部导航 4 目的地：书架（最近/最多/网格切换放 AppBar 菜单）、标签管理、书源（现有 sources 列表 + 添加）、设置。
- 侧栏的搜索框/标签 Chip/书源列表按目的地归属：搜索框 + 标签 Chip 进书架页 AppBar 下方（横向滚动）；书源列表进"书源"页；标签管理进"标签"页。
- 标签详情 split（home_page.dart:478-492 的 `Row(列表 + VerticalDivider + 详情)`）在 compact 下改为：点标签进入全屏详情页（或详情替换列表区域），不再并排。
- `_section`/`_searchCtrl`/`_tags` 等既有状态复用，不重建数据流。

## 3. 详情页（book_detail_page.dart）

现状：`Row(封面列 + VerticalDivider + Expanded(元数据/标签/简介))`。

compact 分支：
- 改为 `Column`：封面区（含操作按钮）在上，元数据/标签/简介在下（ScrollView 包裹）。
- `SizedBox(width: 220, OutlinedButton)` 按钮改 `SizedBox(width: double.infinity)` 或在 Column 内自然撑满。
- AI 相关按钮保持现状由 p1 隐藏；本任务只处理布局。

## 4. 对话框/表单统一

- 全部 `AlertDialog`（添加/编辑/删除书源、重命名/删除标签、批量打标签、跳转页码、快捷键捕获、QR 授权、迁移确认等）内容包 `ConstrainedBox(maxWidth: dialogMaxWidth(context))`。
- `AddSourceDialog` 的 `SizedBox(width: 420)` 改为 `ConstrainedBox(maxWidth: dialogMaxWidth)`。
- WebDAV 连接表单（`ConstrainedBox(maxWidth: 420)`）已可收缩，仅巡检确认无溢出。

## 5. 设置面板（home_page.dart settings 区）

compact 分支：
- `Row(Text + SizedBox(width:120, Slider) + Text)` → `Column(label, Slider)` 或 `ListTile` 结构。
- Dropdown 行（如默认阅读模式）→ `Row(Expanded(Text), Dropdown)` 或 ListTile trailing。
- 快捷键绑定行 → 可换行或 ListTile。

## 6. 书源/WebDAV/缓存/同步/封面编辑

- 逐页巡检：固定宽度 Row、`SizedBox(width: n)`、Dialog 内容、AppBar 标题溢出。
- `library_page.dart` 的封面渲染 340x480 改为按 Grid 单元宽度缩放（或保持，仅在 grid 内 `fit` 处理）。
- 封面编辑页已有 LayoutBuilder（cover_editor_page.dart:146），仅巡检。

## 7. 触控/insets/方向

- compact 壳统一 `SafeArea`；底部 NavigationBar 自带 insets 处理。
- Android 列表页竖屏锁定（`SystemChrome.setPreferredOrientations`，在对应页面或壳层进入时设置，离开时恢复）；阅读器允许旋转。
- 触控目标巡检：IconButton 默认 48dp 达标；阅读器左右 80dp 点按区达标；设置区小图标按钮补 `constraints`。

## 8. 文本缩放

- 主要页面用弹性布局 + `TextOverflow.ellipsis`；验收在字体缩放 1.3 下巡检。

## 9. 兼容与回归

- wide 分支代码路径保持原样；改动集中在 compact 分支与共享工具。
- Windows 回归：`cargo test` + `flutter analyze` + `flutter build windows`（或现有 release 构建脚本）。
- 与 p1 的边界：阅读器手势、AI 隐藏、SAF 导入归 p1；本任务只处理布局与导航，验收时联调。
