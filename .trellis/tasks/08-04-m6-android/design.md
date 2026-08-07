# M6 Android 适配 — 技术设计

## 1. 总体架构与边界

复用现有四层架构,Android 只是 Flutter 的又一个平台目标:

```
Flutter UI(现有 lib/ui,触屏适配) → FRB(cargokit 编译 Rust cdylib) → Rust 核心(Document/Source/Cache/DB)
```

- Rust 核心按 ABI 编译:aarch64-linux-android(必发)、armv7-linux-androideabi、x86_64-linux-android(模拟器)。
- 新增的 Android 专属边界只落在:平台构建配置(android/)、原生库打包(jniLibs)、触屏交互、SAF 文件访问、OAuth 回调方式。Rust 业务逻辑不因平台分叉(除 `pdf.rs` 的库加载路径)。
- 现有 Windows 构建链不受影响;任何 Rust 改动必须通过 Windows 回归(`cargo test` + `flutter analyze` + Windows 构建)。

## 2. 构建链设计(P0)

- 环境:Android SDK + NDK(Flutter 3.44.8 默认 NDK 28.2.13676358),无 Android Studio 也能用 sdkmanager 命令行安装。
- 镜像(国内网络):Gradle 仓库走阿里云 Maven 镜像;pub.dev 走 PUB_HOSTED_URL 镜像;crates.io 走 rsproxy / 中科大镜像。
- cargokit 已含 Android 支持(`rust_builder/android/`),无需改 FRB 骨架;验证 `cargo ndk` 目标安装即可。
- 应用标识:`applicationId = com.rch.reader`(已确认 2026-08-07)、应用名 "RCH"、图标替换。
- Manifest:`INTERNET` 权限必加;后续按需加 `MANAGE_EXTERNAL_STORAGE`(不推荐,首版不用)。

## 3. 原生库策略(P3)

### PDF(libpdfium.so)
- 来源候选:pdfium-android Maven 制品(含 libpdfium.so 各 ABI)或按 ABI 自行编译 pdfium。
- 落位:`android/app/src/main/jniLibs/{abi}/libpdfium.so`,由 Gradle 打进 APK。
- 加载:现有 `pdf.rs` 按 进程工作目录 → exe 所在目录 → PATH → 系统目录 链式查找(2026-08-04 修复);Android 上 `current_exe()` 指向 /proc/self/exe,父目录不是 nativeLibraryDir,必须显式传入库目录:通过 Dart 侧拿 `ApplicationInfo.nativeLibraryDir` 传给 Rust。FRB 已暴露 `set_cache_root_path` 这类路径参数模式,新增一个"native lib 目录"参数即可。
- 验证关卡:先做最小验证(渲染一页 PDF 成功)再合入阅读器。

### RAR / CBR(unrar)
- unrar 0.5.8 依赖 unrar_sys(cc 编译 unrar C++ 源码),NDK 交叉编译需要 clang + 正确 target;大概率可行,但需验证。
- 备选:若 NDK 编译失败:① unrar crate 的静态编译特性;② 纯 Rust `rar` crate(仅 RAR4,能力有限);③ 首版暂不支持 CBR,并在研究记录中写明结论。
- 决策记录:由 p3-native-formats 子任务产出"可行性验证结论",作为该子任务的验收产物。

## 4. 数据目录与文件访问

- 默认根:`getApplicationSupportDirectory()`(Android 上为应用私有目录,零权限)。
- Android 首版采用"全部文件访问"授权(MANAGE_EXTERNAL_STORAGE,用户确认 2026-08-07):设置面板提供授权入口与状态检查;授权后本地书源可直读外部目录(如 /sdcard/Download),未授权时引导授权;SAF 导入复制保留为备选。
- 现有 `cache_root_marker` / `setCacheRootPath` / 迁移机制保留;首版 UI 不开放自定义根(不暴露目录迁移入口)。
- 缓存层级(v0.3.4 重构):thumb/ 与旧下载缓存层已移除;WebDAV 回退并入 raw/;缓存管理 total = 各分类之和。Android 沿用同一结构。
- 导入:file_selector 在 Android 的 `openFile` / `openFiles` 走 SAF,返回可复制源 → 复制进应用私有目录 → 建索引(复用 openLocalBook 逻辑)。
- 导出 CBZ:`file_selector` 的 `getSaveLocation` 在 Android 不支持 → 改用系统分享(share_plus 或 MediaStore 写 Downloads)。
- 跨端数据:SQLite / JSON 结构与 Windows 同构;v0.3.5 起多端同步(.rchpkg:标签/书源配置/漫画详情/阅读进度/设置),Android 首版复用 **WebDAV 同步通道**;网盘同步盘本地目录通道依赖 `getDirectoryPath`(Android 不支持)后置。

## 5. 触屏交互适配(P1)

- 阅读器:点按左/中/右区域翻页(现有 GestureDetector 扩展)、双指缩放(photo_view 已有)、长按弹操作菜单(替代 `onSecondaryTapUp`,AI 超分入口在安卓隐藏)、系统返回(PopScope 拦截"阅读中返回 = 退出阅读器")。
- 横竖屏:阅读器允许旋转并保留进度;书架等列表页锁定竖屏(首版建议,可后续放宽)。
- 沉浸式:全屏阅读隐藏状态栏 / 导航栏(可选,后续迭代),SafeArea / MediaQuery.padding 处理刘海与手势条。
- 键盘快捷键代码保留(Android 外接键盘、模拟器仍可用),不作为主交互。

## 6. 远程书源(P2)

- WebDAV / SFTP:Rust 逻辑与桌面一致;下载器(重试 / 并发 / 优先级)复用;新增网络状态提示。
- 夸克:桌面为 Cookie 认证(无 OAuth),Android 复用同一登录/会话逻辑(Cookie 输入/持久化回写),可移植性最好。
- 百度 OAuth:桌面是 localhost 回调;Android 需 deep link(intent-filter 自定义 scheme)→ 应用内处理回调,或回退"复制 code 粘贴"。
- 115:扫码授权(Android 相机扫码,或沿用手动输入)+ token 自动刷新回写(现有实现复用)。
- WebDAV 同步通道:复用 v0.3.5 备份/同步面板(推/拉/恢复/归档清理),WebDAV 同步复用 WebDAV 书源配置,Android 直接可移植;网盘同步盘本地目录通道后置。
- 打开策略(auto / download / stream)与缓存机制与桌面一致;token 存储位置不变(SQLite / 本地 JSON)。

## 7. 兼容性与迁移

- minSdk 24(Android 7.0)、targetSdk 36、compileSdk 36(Flutter 3.44.8 默认)。
- ABI:arm64-v8a 必发;armeabi-v7a 视设备覆盖;x86_64 仅模拟器 / 少部分平板。
- 数据库:首版不迁移 Windows 数据(路径语义不同);同一设备上应用升级保持数据兼容(现有 SQLite 迁移逻辑直接复用)。

## 8. 发布与回滚(P4)

- 签名:生成正式 keystore(不入库,写入 local 配置或 CI secret),release 替换 debug 签名。
- 版本:沿用 `0.3.x+<build>` 策略;versionCode 递增。
- 回滚点:每个子任务独立可回滚;P0 不通过则整个 M6 暂停;P3 原生库不通过按预案降级为"该格式延后";P2 OAuth 不通过可先上线 WebDAV/SFTP 再补网盘。

## 9. 任务图(子任务映射)

| 子任务 | 内容 | 依赖 | 验收产物 |
|---|---|---|---|
| p0-android-buildchain | SDK/NDK、镜像、模板跑通、标识/权限 | 无 | 可安装 release APK(arm64) |
| p1-local-reader | 本地阅读闭环 + 触屏 + SAF 导入 + AI 隐藏 | p0 | 真机完整读完一本本地漫画 |
| p3-native-formats | PDF / RAR 原生库 | p0 | PDF / CBR 真机可读 |
| p2-remote-sources | WebDAV / SFTP / 百度 / 115 / 夸克 + WebDAV 同步通道 | p0 + p1 | 五类书源闭环 + token 刷新 + 同步推/拉 |
| p5-ui-narrow-screen | 窄屏 UI 适配（主壳/全部页面响应式） | p0（p1 并行衔接） | 360dp 宽全页面无溢出 + 桌面回归 |
| p4-release | 签名、ABI、版本、文档、回归 | p1+p2+p3 | 已签名发布 APK + 回归通过 |

实施顺序:p0 → (p1 ∥ p3 ∥ p5) → p2 → p4。
