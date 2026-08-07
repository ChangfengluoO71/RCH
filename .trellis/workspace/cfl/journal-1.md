# Journal - cfl (Part 1)

> AI development session journal
> Started: 2026-07-31

---



## Session 1: 修复缩放拖动 Bug + 实现阅读器页面旋转（含批量规划）

**Date**: 2026-08-02
**Task**: 修复缩放拖动 Bug + 实现阅读器页面旋转（含批量规划）
**Branch**: `master`

### Summary

修复阅读器缩放后移动区域只在第一页生效（photo_view scaleState 同步重置 + 双页 panEnabled）；实现阅读器页面旋转（右键界面旋转、每页独立 90° 旋转、BookMeta.rotations SQLite/JSON 双写持久化、旧库 ALTER TABLE 补列）；批量规划 7 个小功能 PRD。

### Main Changes

- 修复单页翻页后缩放/拖动失效：同步重置 PhotoViewScaleStateController
- 双页模式启用 panEnabled，缩放后可拖动
- 阅读器页面旋转：右键界面旋转 + 单页/双页独立旋转按钮（90° 循环）
- 旋转持久化：BookMeta.rotations + SQLite book_metas.rotations 列 + FRB 桥接重生成

### Git Commits

| Hash | Message |
|------|---------|
| `c2cfc69` | (see git log) |
| `e7a4dac` | (see git log) |
| `8f28e26` | (see git log) |
| `59d2f48` | (see git log) |

### Testing

- [OK] flutter analyze 0 issues；flutter test 5 passed（缩放回归 + 旋转模型 round-trip）
- [OK] cargo test --lib 31 passed（含 rotations 列测试）

### Status

[OK] **Completed**

### Next Steps

- 用户本地 flutter run 实测旋转与缩放手感
- 剩余规划任务待开工（M5 书源 / 标签分层 / 后缀识别 / 转 CBZ / AVIF）


## Session 2: M6 网盘直连书源 + v0.3.2 发布

**Date**: 2026-08-03
**Task**: M6 网盘直连书源 + v0.3.2 发布
**Branch**: `master`

### Summary

M5 收尾提交；M6 实现百度/115 官方 API 书源（OAuth/设备码授权、三态打开、封面缓存、token 回写）；联调通过；归档 M5/M6；发布 v0.3.2（README/CHANGELOG 更新）

### Git Commits

| Hash | Message |
|------|---------|
| `de8de58` | (see git log) |

### Status

[OK] **Completed**


## Session 3: 百度网盘 31045 修复：dlink 拼接 access_token + 403 强制刷新 + 书源删除 SQLite 持久化

**Date**: 2026-08-06
**Task**: 百度网盘 31045 修复：dlink 拼接 access_token + 403 强制刷新 + 书源删除 SQLite 持久化
**Branch**: `master`

### Summary

修复百度网盘源远程下载 31045（access_token 验证未通过）：下载 dlink 统一拼接当前 access_token；下载 403 时强制刷新 token 重取 dlink 重试；API 遇 -6/110/31045 自动刷新；拦截 200+JSON 错误体；书源删除/清理失效记录同步删 SQLite 行。实测 dlink+token+UA → 302 → 200 PDF。已建并归档任务 08-06-baidu-31045-fix。

### Main Changes

- dlink 下载统一拼接当前 access_token（官方要求）
- 下载 403/31045 强制刷新 token 后重试
- removeSourceWithCleanup / purgeStaleRecords 同步删除 SQLite 行，修复删除重启复活

### Git Commits

| Hash | Message |
|------|---------|
| `a637ffc` | (see git log) |
| `0128e1f` | (see git log) |

### Testing

- [OK] cargo check + 8 个百度单测 + flutter analyze 通过；真实账号端到端下载 200

### Status

[OK] **Completed**

### Next Steps

- 跑 flutter run/build windows --release 全量构建，让 Dart 层修复进入正式包


## Session 4: 缓存管理清理：移除缩略图/旧下载缓存层

**Date**: 2026-08-06
**Task**: 缓存管理清理：移除缩略图/旧下载缓存层
**Branch**: `master`

### Summary

按 08-02-cache-tier-review 实施：删除 thumb/ 占位缓存层；download/ 的 WebDAV 无 Range 回退并入 raw/；total 改为缓存分类之和；清空全部缓存覆盖旧版遗留目录；删除磁盘上 thumb/download/旧哈希缓存约 40MB 与 baidu_debug.log、database.db 备份；cargo test 59 项通过、flutter analyze 无问题

### Git Commits

| Hash | Message |
|------|---------|
| `c660661` | (see git log) |
| `860c457` | (see git log) |

### Status

[OK] **Completed**


## Session 5: 缓存管理收尾：codegen 脚本 + 缓存文案修正 + 详情页提示去具体化

**Date**: 2026-08-06
**Task**: 缓存管理收尾：codegen 脚本 + 缓存文案修正 + 详情页提示去具体化
**Branch**: `master`

### Summary

新增 codegen.ps1（绑定生成+release DLL 重建）防止 FRB content hash 不同步；修正缓存管理相关文案（raw/ 为远程书源整本下载、temp/ 为 AI 超分临时文件）；漫画详情页元数据编辑去掉具体提示文案；排查并清理构建僵尸进程导致的构建卡死

### Git Commits

| Hash | Message |
|------|---------|
| `875a8c3` | (see git log) |
| `0162ee7` | (see git log) |
| `7b4c7c7` | (see git log) |

### Status

[OK] **Completed**


## Session 6: 接手安卓端开发 — 状态梳理 + P1 SAF 导入落地

**Date**: 2026-08-07
**Task**: M6 Android 适配（接手续做）
**Branch**: `master`

### Summary

接手 M6 安卓端开发并梳理状态：P0 构建链已打通（applicationId=com.rch.reader、图标/应用名/INTERNET 权限，debug+release APK 已产出，仅剩 flutter doctor license 未接受）；P1 已落地存储授权、AI 入口隐藏、阅读器触屏（点按翻页/双指缩放/长按菜单/PopScope 返回/横竖屏）；P5 窄屏 UI 八步全绿（模拟器 411dp/360dp 验收，Windows 构建待环境验证）；P2/P3/P4 仍为 planning（unrar 已 vendor，PDF libpdfium 待 p3）。

本次落地 P1 缺口：书源页新增“导入本地漫画”入口（SAF openFiles → 流式复制进应用私有 books/ 目录 → 自动创建/复用 `local_import` 本地书源并跳转），文件名清洗加单测。

### Main Changes

- `app/lib/ui/home_page.dart`：`_importLocalComics()` + `safeImportedFileName()`；书源列表头部新增导入按钮（宽屏侧栏与手机书源页共用）
- `app/test/safe_import_name_test.dart`：新增 3 项文件名清洗单测

### Testing

- [OK] flutter analyze 0 issues；flutter test 28 项通过（含新增 3 项）
- [OK] cargo test --lib 76 passed / 1 ignored（Windows 回归）

### Status

[WIP] P1 SAF 导入已实现，待真机验收（SAF 选文件 → 书架 → 阅读 → 进度记忆 → 续读）

### Next Steps

- 接受 Android license（`flutter doctor --android-licenses`）后跑 `flutter build apk --debug` 验证含新代码的安卓构建
- P1 真机验收：SAF 导入 CBZ 闭环 + EPUB/文件夹/CB7/CBT/MOBI 各一本 + 横竖屏/返回行为
- P5 剩余：Windows 构建回归（交互式终端）
- 后续按序推进 P3（PDF/RAR 原生库）→ P2（远程书源）→ P4（发布）


## Session 7: MuMu 添加云端书源“没反应”根因修复

**Date**: 2026-08-07
**Task**: M6 Android — MuMu 云端书源添加无反馈
**Branch**: `master`

### Summary

排查 MuMu 上“无法添加云端书源”：先发现 MuMu 安卓实例实际未启动（`is_android_started=false`，adb 端口全拒，已用 `mumu-cli control --vmindex 0 launch` 拉起，adb 端口 16384）；复现 WebDAV 连接（rustls TLS 到 dav.jianguoyun.com 成功，返回 HTTP 401）确认网络链路正常。

**根因**：添加书源对话框的错误文本渲染在 `SingleChildScrollView` 底部；MuMu 为 720x1280/density 240 横屏（逻辑宽 853dp），失败较快时“测试中…”一闪而过，红色错误落在可视区下方 → 用户点了“添加”却看不到任何反馈。

### Main Changes

- `app/lib/ui/home_page.dart`：`_AddDialogState` 新增 `_scrollCtrl` + `_setError()`（错误后自动滚动到可见处）；19 处错误赋值统一改走 `_setError`
- `app/test/add_source_dialog_test.dart`：新增“提交校验失败时错误信息自动滚动可见”回归测试

### Testing

- [OK] flutter analyze 0 issues；flutter test 29 项通过（含新增回归）
- [OK] `flutter build apk --debug` 成功并装到 MuMu 复测：点“添加”后错误立即可见（修复前需手动下滑才看得到）

### Other Findings

- 内置百度 AppKey/SecretKey、115 APP ID 均为空占位（netdisk_credentials.dart TODO）：授权类书源需用户在“高级选项”自填，否则“授权登录”会直接报“未配置…”，现已可见。
- MuMu 会旋转/缩放应用窗口（setPresentationRotation、lastNonFullscreenBounds），自动化点击坐标会抖动；属模拟器环境因素，非应用代码问题。

### Status

[OK] 修复完成并实机验证；待用户真实凭据继续验收 WebDAV/百度/115/夸克 添加闭环
