# 夸克网盘书源

## Goal

在 RCH 中新增「夸克网盘」书源类型（`type='quark'`），与百度网盘 / 115 并列，用户粘贴 Cookie 认证后即可浏览本人网盘漫画、以三种打开策略阅读、查看封面，凭据重启保持，删除书源时记录与元数据清理干净。全程复用现有书源框架，改动面与 115 相当。

## Background（已确认事实）

- 夸克网盘无官方开放平台 API（M6 任务已确认并据此排除，见 `.trellis/tasks/archive/2026-08/08-03-m6-netdisk-official-api/prd.md`），但存在被 AList / quark-auto-save / QuarkPanTool 等长期使用的非官方 Web API（Cookie 认证）。实现细节以 AList `drivers/quark_uc` 为基准（`{meta,types,util,driver}.go`），实现前用真实 Cookie 冒烟确认（见 implement.md 步骤 0）。
- 关键接口：base `https://drive.quark.cn/1/clouddrive`；请求头 `Cookie` + `Referer: https://pan.quark.cn` + quark-cloud-drive Electron UA；query `pr=ucpro&fr=pc`；`GET /config` 校验凭据；`GET /file/sort`（`pdir_fid` 分页，根目录 `0`）列目录，响应 `data.list[]`（`fid` / `file_name` / `size` / `file` 布尔 / `updated_at` 毫秒）；`POST /file/download`（body `{"fids":[fid]}`）取直链 `data[0].download_url`；下载直链仍需三件套头；`Range: bytes=0-0` 206 则流式可用；响应 `code!=0` 即错误，Set-Cookie 中的 `__puus` 可回写续期。
- 现有框架可直接复用：Rust `ByteSource` / `Entry` / `RateGate`（`app/rust/src/source/mod.rs`）、三态打开与 raw/cover 缓存（WebDAV / 百度 / 115 模式）、`removeSourceWithCleanup`（`app/lib/store/library_store.dart:409`，按 `type|id|` 前缀清理记录与元数据）、Dart `BookSource`（`app/lib/store/models.dart`）与 `needsSession` 分发点（`source_browser.dart` / `book_detail_page.dart` / `ai_upscale_manager.dart` / `comic_cover.dart`）。
- 认证方案（已确认）：v1 采用**粘贴 Cookie**（与 AList 同款），不逆向 passport 扫码登录。
- 字段约定（已确认）：Cookie 存新增 DB 列 `cookie`；根目录 fid 存 `rootId`（默认 `'0'`，镜像 115）；浏览/打开路径为 fid 存 `path`；书 key `bookKeyOf('quark', sourceId, fid)`。
- 教训借鉴：115 把提取码当 path 传给 `open_document`（扩展名探测失败隐患，115 联调未完成）；夸克改为用 download 响应 / 列表中的**真实文件名**做格式探测，fid 仅作 API 与缓存键。

## Requirements

- **R1** 新增书源类型 `'quark'`，`BookSource` getter（`isQuark` / `needsSession` / `capabilityDisplay`）与 `_sourceTypeLabel` 同步扩展。
- **R2** 添加 / 编辑表单：Cookie 字段（必填）、根文件夹 ID（默认 `0`）、保存时连通性测试（`/config` + 首屏 list）。
- **R3** 浏览：按 fid 列目录，目录在前自然排序；凭据失效 / 风控时给出中文提示。
- **R4** 打开：复用全局「打开策略」（auto / download / stream），语义与 WebDAV / SFTP / 百度 / 115 一致：auto=先整本下载 raw/ 缓存、失败回退流式；download=强制整本；stream=直链 Range 流式（不支持 Range 则整本）。
- **R5** 封面：走现有 cover 管线（cover/ 磁盘缓存 → raw/ 本地缓存 → 流式解码第一页），对 CBZ/ZIP 生效。
- **R6** 凭据持久化：Cookie 存 `cookie` 列，重启可用；`__puus` 等续期 cookie 尽力回写。
- **R7** 清理：删除书源时清理其记录与元数据（`removeSourceWithCleanup` 前缀天然覆盖），不影响其他类型；磁盘 raw/cover 缓存沿用应用现有「清空缓存」机制（与全部书源一致）。
- **R8** 兼容：旧数据可读；未配置新字段的书源不受影响；重启不重复加列（幂等 ALTER，模式同 M5 `port` / M6 `refresh_token` 列）。

## Acceptance Criteria

- [ ] 添加夸克书源（粘贴 Cookie）→ 连通性测试 → 浏览 → 三策略打开 CBZ → 封面 → 重启后凭据保持，全链路可用
- [ ] Cookie 失效 / 风控时给出明确中文提示并引导重新粘贴；`__puus` 续期后请求继续可用
- [ ] `cargo test --lib` 全绿（sort / download JSON 解析、fid 处理、raw 缓存命名、错误码映射单测）
- [ ] `flutter analyze` 0 issues；`flutter test` 全绿（`add_source_dialog_test.dart` 更新为 7 类型并断言 quark 表单）
- [ ] 删除书源后记录 / 元数据清理干净，且不影响其他书源
- [ ] `cargo build --release` 通过；用户手动 `flutter build windows --release` 冒烟验证

## Out of Scope

- 夸克分享链接解析 / 转存（sharepage 接口），v1 仅限本人网盘文件
- 扫码登录（v2 候选：需逆向 passport.quark.cn 登录接口，风险与维护成本高）
- 上传 / 移动 / 删除 / 重命名等写操作
- 搜索（夸克无公开搜索接口）
- Android 平台适配（走 `08-04-p2-remote-sources` 任务线，桌面先行）
- 视频转码直链（AList transcoding address 仅适用视频；本项目为漫画阅读器）
