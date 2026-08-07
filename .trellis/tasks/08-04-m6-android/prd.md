# M6 Android 适配 — 安卓端开发

## Goal

将 RCH(本地优先的漫画阅读器)从 Windows 单端扩展为 Windows + Android 双端:复用现有 Flutter UI + Rust 核心(FRB 桥接),让用户在手机/平板上完成"添加本地漫画 → 阅读 → 管理"的核心闭环,并逐步覆盖远程书源。产品价值:同一套代码两端运行,数据本地优先、无账号、不依赖云端。

## Confirmed Facts(仓库/环境核查结论)

- 技术栈:Flutter 3.44.8 + Rust cdylib(flutter_rust_bridge 2.12),`rust_builder` 使用 cargokit,已包含 Android 的 Gradle 构建支持。
- SPEC.md 第 3 节架构(Flutter UI → FRB → Rust Document/Source/Cache/DB/AI)就是为多端设计的;SPEC 第 10 节 M6 = Android 适配(手机/平板)。
- 本机环境:Android SDK/NDK 未安装(`flutter doctor` 报错),无 Android Studio;网络访问 maven.google.com / github 存在 TLS 握手错误,需要国内镜像(Gradle / Maven / pub)。
- `app/android` 为 Flutter 默认模板:`applicationId=com.example.app`、release 用 debug 签名、无 `INTERNET` 权限、默认图标与应用名、minSdk/targetSdk 走 Flutter 默认值。
- Rust 核心跨平台性:
  - 纯 Rust / 可 NDK 编译:zip、epub、mobi、tar、sevenz-rust、image、reqwest(rustls)、russh(SFTP)、rusqlite(bundled)、sha2、fs2。
  - 需原生处理:pdfium-render(PDF,需按 ABI 打包 libpdfium.so 并改加载路径)、unrar(unrar_sys 用 NDK 编 C++,需验证)、AI 超分 CLI(realesrgan-ncnn-vulkan.exe 为 Windows 专属)。
  - local 书源随机读已有 `#[cfg(unix)]` 分支(Android 走 `read_at`),无需额外工作。
- UI 桌面交互依赖:右键菜单(单页超分)、键盘快捷键(翻页/缩放)、`file_selector` 的目录/保存选择在 Android 不可用;需要触屏、系统返回键、SAF 选文件适配。
- 远程书源:WebDAV / SMB / SFTP / 百度 / 115 / 夸克 已在 Rust 实现;Android 需要 `INTERNET` 权限;SMB 当前走 Windows UNC 直连,Android 需要替代方案(待调研)。

## Key Decisions(已确认)

- **MVP 范围 = B:本地阅读闭环 + 远程书源,首版不含 AI 超分**(用户确认,2026-08-04)。
- **AI 超分在首版隐藏/禁用入口**,待 M2 Phase 3(ONNX Runtime)落地后再评估 Android 方案;不影响首版验收。
- **首版本地阅读格式范围 = 全格式对齐桌面**:ZIP/CBZ、EPUB、文件夹、CB7、CBT、MOBI、PDF、RAR/CBR 全部进首版;PDF 需打包 libpdfium.so,RAR 需 NDK 编译 unrar(用户确认,2026-08-04)。
- **SMB 不进首版**:首版远程书源 = WebDAV / SFTP / 百度 / 115 / 夸克;SMB 留待后续版本(Android 需替代实现,先调研再排期)(用户确认,2026-08-04)。
- **夸克书源进首版(P2)**:cookie 认证、无 OAuth,桌面端已上线(v0.3.3),Android 跨端复用成本最低(用户确认,2026-08-07)。
- **WebDAV 同步通道进首版(P2)**:复用 v0.3.5 备份/同步能力(标签/书源/详情/进度),Android 支持 WebDAV 通道推/拉/恢复;网盘同步盘本地目录通道依赖 `getDirectoryPath`(Android 不可用)后置(用户确认,2026-08-07)。
- **数据目录 = 应用私有目录 + SAF 导入复制**:默认书架/缓存放应用私有目录(零权限);外部漫画通过系统文件选择器(SAF)导入并复制进应用目录;首版不开放自定义外部目录挂载(用户确认,2026-08-04)。

## Requirements

- R1:安卓端可构建、可安装、可启动(debug 与 release APK)。
- R2:本地漫画阅读闭环:书架 / 详情 / 阅读器,支持 ZIP/CBZ、EPUB、文件夹、CB7、CBT、MOBI、PDF、RAR/CBR;触屏翻页、缩放、长按菜单、系统返回、横竖屏。
- R3:远程书源能力:WebDAV / SFTP / 百度 / 115 / 夸克可用(SMB 不在首版);WebDAV 同步通道(备份/同步面板)可用。
- R4:数据与缓存目录 = 应用私有目录;外部漫画经 SAF 导入并复制进应用目录;导出 CBZ 走系统分享。
- R5:发布配置:正式 applicationId、应用名、图标、签名、ABI 拆分、版本号。
- R6:兼容基线:minSdk 24(Android 7.0,Flutter 3.44 默认,待最终确认)、targetSdk 36、compileSdk 36。
- R7:AI 超分在安卓端隐藏/禁用入口(桌面端保持不变)。

## Acceptance Criteria(初版,待收敛后定稿)

- [ ] 能构建并安装 release APK 到真机(至少 arm64-v8a)。
- [ ] 手机/平板上可完整读完一本本地漫画:翻页、缩放、进度记忆、续读。
- [ ] 首版范围内的远程书源可连接、浏览、打开、缓存。
- [ ] 夸克书源可连接/浏览/打开/缓存;WebDAV 同步通道可推/拉(标签/书源/进度)。
- [ ] 应用名 / 图标 / 包名正确,无 Flutter 默认模板痕迹。
- [ ] 安卓端不出现 AI 超分入口;远程书源打开策略 / token 刷新可用。
- [ ] Windows 端现有功能不回归。

## Out of Scope(默认排除)

- 在线漫画聚合、账号体系与第三方服务器云同步(SPEC 已排除;用户自有 WebDAV 同步通道见 R3)。
- 网盘同步盘本地目录通道(依赖 `getDirectoryPath`,Android 后置,见 Key Decisions)。
- iOS(未在本任务范围)。
- AI 超分(首版隐藏/禁用,见 Key Decisions)。
- SMB 书源(见 Key Decisions)。

## Open Questions(阻塞规划)

无阻塞开放问题。已确认:minSdk = 24(2026-08-07)、applicationId = `com.rch.reader`。
