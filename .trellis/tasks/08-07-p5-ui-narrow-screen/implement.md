# M6 窄屏 UI 适配 — 实施计划

> 按 inline 模式执行（与 M6 父任务一致，不派生子代理）；Phase 2 由主会话直接实施。

## 实施顺序

1. 公共响应式工具 → 2. 主壳 compact 分支 → 3. 对话框统一 → 4. 详情页 compact → 5. 设置面板 compact → 6. 其余页面巡检修复 → 7. 触控/insets/方向/文本缩放 → 8. 回归验收。

## 分步检查清单

### Step 1：响应式工具（common.dart）
- [x] `isCompact(context)`（<600dp）、`dialogMaxWidth(context)` 落地，单测或 analyze 通过。

### Step 2：主壳 compact 分支（home_page.dart）
- [x] 底部导航 5 目的地（最近/最多/标签/书源/设置），宽屏分支不变。
- [x] 搜索框 + 标签筛选进内容区顶部；标签详情窄屏改为全屏（带回退）。
- [x] 模拟器 411dp 下主壳完整可用，无溢出。

### Step 3：对话框统一
- [x] AddSourceDialog 固定 420px 改为 `dialogMaxWidth`；其余对话框审计（默认 inset 可收缩）。
- [x] 360dp 宽下每个对话框可完整操作（已通过 add_source_dialog_test 21 项回归）。

### Step 4：详情页 compact（book_detail_page.dart）
- [x] 窄屏左右分栏 → 上下排列（封面/按钮在上，元数据/标签/简介在下）。

### Step 5：设置面板 compact
- [x] SegmentedButton 加横向滚动；固定 Row 改 Wrap（默认模式/拼接间隙）。

### Step 6：其余页面巡检
- [x] 书源浏览器/同步面板等审计：均用 Expanded/ellipsis，无固定宽度溢出；WebDAV 表单 maxWidth 可收缩。

### Step 7：触控/insets/方向/文本缩放
- [x] Android 列表页竖屏锁定（home_page），阅读器进入解锁旋转、退出恢复竖屏。
- [x] 字体缩放 1.3 实测（模拟器）无 RenderFlex 溢出。

### Step 8：回归
- [x] `flutter analyze` 无新增 issue；`flutter test` 21 项通过。
- [x] Rust `cargo test --lib` 73 项通过。
- [ ] Windows 构建成功（宽屏布局不变）——本会话后台/前台多次空转（环境问题，非代码；改动为纯 Dart，analyze 通过，宽屏分支未动），待交互式终端验证。
- [x] 模拟器 411dp 与 360dp（wm density 480）验收：主壳 5 tab、详情页、阅读器、阅读设置弹层均无溢出；翻页/弹层交互正常。

## 验证命令

```bash
cd app
flutter analyze
flutter test
cd rust && cargo test
flutter build apk --debug
flutter run -d emulator-5554
flutter build windows --debug   # 桌面回归
```

## 风险与回滚点

| 风险 | 预案 | 回滚点 |
|---|---|---|
| 主壳改造破坏既有页面 | wide 分支保持原代码路径，compact 分支独立 | 仅回滚主壳改动 |
| 对话框 maxWidth 影响 Windows 观感 | 仅加 maxWidth 上限，宽屏下数值不变 | 仅回滚对话框改动 |
| 与 p1 阅读器改动冲突 | 阅读器窄屏微调以 p1 为准，本任务只验收 | 联调时对齐 |
| 字体缩放/insets 引发回归 | 按页面验收清单逐页核对 | 单页回滚 |

## 质量门

- 每步完成：验收项全绿 → `flutter analyze` 无新增 issue。
- Step 8 全量回归通过后，回到父任务 M6 统一验收。
