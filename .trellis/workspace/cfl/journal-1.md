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


## Session 8: 提交 P1/对话框修复 + P3 原生库可行性评估

**Date**: 2026-08-08
**Task**: M6 Android — 提交更新 + P3 PDF/RAR 评估
**Branch**: `master`

### Summary

提交上一批更新（commit `18565f1`：P1 SAF 导入 + 书源对话框错误自动可见 + 回归测试 + Trellis 记录）。随后对 P3 原生格式做实证评估：

**RAR/CBR — 构建级已验证可行**：P0 时 unrar 在 Android 上被隔离；放开隔离后交叉编译唯一报错是 bionic 无 `lutimes`（unrar os.hpp 在 `__linux` 下定义 USE_LUTIMES，Android 也定义 `__linux`）。修复为 vendored os.hpp 加 `!defined(__ANDROID__)` 条件。`cargo check --target aarch64-linux-android` 与 `flutter build apk --debug`（4 ABI）全部通过；RarBook 实现全可移植（临时文件 + 静态 libunrar.a + 内存读页），待真机验收。

**PDF — 待打包 .so 与加载路径**：pdfium-render 0.9.3 已参与 Android 编译；缺 libpdfium.so 按 ABI 进 jniLibs（来源 bblanchon/pdfium-binaries 或 pdfium-android Maven 制品）+ pdf.rs 通过 nativeLibraryDir 显式加载（Dart 侧 method channel 取 ApplicationInfo.nativeLibraryDir 传给 Rust，仿 set_cache_root_path 参数模式）。

### Main Changes

- `app/rust/Cargo.toml`、`app/rust/src/document/mod.rs`：解除 unrar 的 Android 隔离（工作区未提交）
- `app/rust/vendor/unrar_sys/vendor/unrar/os.hpp`：Android 禁用 USE_LUTIMES 补丁（工作区未提交）
- `.trellis/.../research/android-native-libs.md`：记录 RAR 验证结论；父任务 implement.md 勾选 unrar NDK 编译验证项

### Testing

- [OK] cargo check --target aarch64-linux-android（含 unrar）通过
- [OK] flutter build apk --debug（armv7/aarch64/x86_64/i686 全 ABI）通过

### Status

[WIP] RAR 侧构建打通待真机；PDF 侧待 .so 打包 + 加载路径；P3 建议按父任务设计继续


## Session 9: P3 收尾 — PDF/RAR 真机验证通过并回归提交

**Date**: 2026-08-08
**Task**: M6 P3 原生格式 PDF/RAR 安卓适配
**Branch**: `master`

### Summary

P3 全部验收达成。过程要点：

1. **构建链修复**：Gradle 9.1.0 transforms 缓存大面积丢失导致 Kotlin/Gradle 失败 → 移走 `~/.gradle/caches/9.1.0/transforms` 后恢复。
2. **cargokit host build-script 失败**：诊断确认瞬态/环境残留（干净 shell 复现成功），非工具链配置问题。
3. **C++ 运行时缺失**：unrar C++ 代码使 cdylib 引用 libc++ 符号；仅打包 `libc++_shared.so` 不够，因 DT_NEEDED 缺失。最终方案：unrar_sys build.rs 对 Android 输出 `cargo:rustc-link-lib=c++` → DT_NEEDED libc++_shared.so → 运行时自动加载。
4. **真机验证（MuMu x86_64）**：应用启动无崩溃；PDF（dummy.pdf）1/1 页渲染 + 进度记录；CBR（手工 RAR4 stored，2 张 PNG）2 页解码 + 翻页；全程无报错。

### Main Changes（待提交）

- Rust：解除 unrar 的 Android 隔离；os.hpp 禁用 USE_LUTIMES；build.rs 按目标平台修 powrprof/pthread 并加 `-lc++`；pdf.rs 支持 native lib 目录；新增 api/pdf.rs `set_native_lib_dir`
- Android：MainActivity method channel `nativeLibraryDir`；jniLibs 打包 libpdfium.so（arm64/x86_64）+ libc++_shared.so（4 ABI）
- Dart：main.dart 启动时注入 nativeLibraryDir；storage_access.dart 新增方法
- FRB：codegen 重新生成绑定 + Windows release DLL

### Testing

- [OK] flutter analyze 0 issues；flutter test 全过
- [OK] cargo test --lib 全过（Windows 回归）
- [OK] MuMu 真机 PDF/CBR 阅读闭环

### Status

[OK] P3 完成，待提交


## Session 6: 115 扫码 Cookie 书源 + v0.4.2 发布

**Date**: 2026-08-08
**Task**: 115 扫码 Cookie 书源 + v0.4.2 发布
**Branch**: `master`

### Summary

115 网盘新增扫码获取 Cookie 书源（无需 APP ID）：web_qr_* 三接口、Cloud115WebClient 列表/直链/Range/缓存、p115client 固定 key m115 加密、默认 wechatmini 扫码设备；全局 errors.log；修复扫码对话框、pickcode 字段、setState during build。Rust 84 + Dart 45 测试全绿，打 tag v0.4.2 发布。

### Git Commits

| Hash | Message |
|------|---------|
| `e9a4ebb` | (see git log) |

### Status

[OK] **Completed**


## Session 7: 115 Cookie 自动续期 + GitHub 下载镜像

**Date**: 2026-08-08
**Task**: 115 Cookie 自动续期 + GitHub 下载镜像
**Branch**: `master`

### Summary

115 Cookie 模式失效自动弹扫码续期（复用共享扫码组件，扫码后自动替换 Cookie 并重连，编辑书源可一键重扫）；更新系统新增下载通道镜像：内置预设 + 自定义前缀 + 从 jsDelivr CDN 自动拉取 mirrors.json（24h TTL）合并列表 + 下载失败自动切换下一个通道。analyze 干净，50 个 Dart 测试全绿。

### Git Commits

| Hash | Message |
|------|---------|
| `9e1a77e` | (see git log) |

### Status

[OK] **Completed**


## Session 8: 使用反馈修复：条漫页码 / 书源顶栏重叠 / PC 图标（v0.5.1）

**Date**: 2026-08-17
**Task**: 使用反馈修复：条漫页码 / 书源顶栏重叠 / PC 图标（v0.5.1）
**Branch**: `master`

### Summary

修复飞书群RCH项目组长风落反馈的三项：①条漫模式底部页码/进度栏+滚动跟随+翻页跳转；②书源界面顶栏改用SafeArea修复与Android状态栏重叠；③PC端应用图标更换为紫底白字RCH并配安装器图标。生成图标工具make_app_icon.py。发布v0.5.1。

### Git Commits

| Hash | Message |
|------|---------|
| `971af41` | (see git log) |

### Status

[OK] **Completed**


## Session 9: Repository documentation reorganization

**Date**: 2026-08-19
**Task**: Repository documentation reorganization
**Branch**: `master`

### Summary

Moved user, setup, project, issue, and release documentation under docs; repaired relative links; added local-artifact ignore rules; rebased onto concurrent GitHub README updates and pushed.

### Git Commits

| Hash | Message |
|------|---------|
| `7391607` | (see git log) |
| `76f0a7b` | (see git log) |

### Status

[OK] **Completed**


## Session 10: Freeze catalog-rules-v3 offline proposal baseline

**Date**: 2026-08-23
**Task**: Freeze catalog-rules-v3 offline proposal baseline
**Branch**: `master`

### Summary

Committed the production catalog-rules-v3 offline proposal parser and archived its design task after the after8 347-row local/Quark validation.

### Main Changes

- Frozen zero-byte, zero-remote-book-source catalog parsing over persisted filename, ancestor and sibling metadata.
- Retained explicit release groups, metadata-gated bilingual aliases and unresolved parenthetical context candidates.

### Git Commits

| Hash | Message |
|------|---------|
| `6ccc696` | (see git log) |

### Testing

- [OK] cargo fmt -- --check; cargo test --lib -- --test-threads=1 (209 passed, 2 ignored)
- [OK] flutter analyze --no-pub (no issues); flutter test --no-pub (57 passed)

### Status

[OK] **Completed**

### Next Steps

- Keep provider enrichment and canonical auto-materialization out of this baseline; continue only after manual proposal review.


## Session 11: M8 离线刮削自动化与标签投影修复

**Date**: 2026-08-24
**Task**: M8 离线刮削自动化与标签投影修复
**Branch**: `master`

### Summary

完成本地快照到离线刮削、Ready 提案自动物化、标签/元数据投影和路径别名合并；隐藏生成式资源标签，保留用户作者/系列命名；389 资产离线物化与幂等验证通过；Flutter 63 项、Rust 232 项通过。归档 M8-A0 自动化任务。

### Git Commits

| Hash | Message |
|------|---------|
| `1265a28` | (see git log) |

### Status

[OK] **Completed**


## Session 12: v0.5.4 发布：离线刮削与书源清理

**Date**: 2026-08-24
**Task**: v0.5.4 发布：离线刮削与书源清理
**Branch**: `master`

### Summary

完成离线刮削自动流程、Ready proposal 元数据/标签投影、标签词汇收敛、详情页原文件名复制、书源即时刷新、115 根目录一致性及远程删除清理；通过 Flutter analyze、72 个 Flutter 测试和 235 个 Rust 测试，准备推送 v0.5.4。

### Git Commits

| Hash | Message |
|------|---------|
| `5c4eaab` | (see git log) |
| `1912d50` | (see git log) |

### Status

[OK] **Completed**


## Session 13: 文件夹批量标签修复与 v0.5.5 发布准备

**Date**: 2026-08-25
**Task**: 文件夹批量标签修复与 v0.5.5 发布准备
**Branch**: `master`

### Summary

修复本地文件夹批量打标签目标类型丢失，并复用漫画文件夹检测结果，准备 v0.5.5 发布。

### Main Changes

- 修复文件夹批量打标签：目录目标保留 directory 类型并正确建索引
- 复用可见目录检测结果，减少标签对话框打开前的重复文件系统探测
- 补充跨层契约、回归测试与 v0.5.5 发布说明

### Git Commits

| Hash | Message |
|------|---------|
| `63af629` | (see git log) |

### Testing

- [OK] flutter test：76 项通过
- [OK] flutter analyze：No issues found
- [OK] cargo test --lib document::folder::tests::is_comic_folder_works：通过

### Status

[OK] **Completed**

### Next Steps

- 推送 master 与 v0.5.5 标签，确认 GitHub Actions 发布产物
