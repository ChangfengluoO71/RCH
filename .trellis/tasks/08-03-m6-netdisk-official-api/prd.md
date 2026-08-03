# M6 网盘直连书源（百度 / 115 官方 API）

## Goal

在 RCH 中新增两种书源类型「百度网盘」和「115 网盘」，直连两家官方开放平台 API，支持浏览、打开（三种策略）、封面、缓存，复用现有书源框架。用户无需自建 OpenList/AList。

## Background

- 已确认方向：不做 OpenList/AList 聚合书源，改为直连百度网盘开放平台 + 115 生活开放平台官方 API（见本任务 `research/netdisk-direct-api-vs-aggregator.md`）。
- 百度有官方 OAuth2（授权码模式，`redirect_uri=oob` 适合桌面应用）；115 有官方 OAuth2 设备码 + PKCE（手机扫码，适合桌面应用）。
- 夸克 / PikPak 无官方 API，不在本任务范围。

## Requirements

1. 书源类型扩展为 `'baidu'`（百度网盘）与 `'115'`（115 网盘），与现有 local/webdav/smb/sftp 并列。
2. 添加 / 编辑书源表单：
   - 百度：根目录（默认 `/`）、可选 AppKey/SecretKey（留空用内置默认）、「授权登录」按钮（浏览器打开官方 OAuth 页，用户粘贴授权码换 token，自动连通性测试）；高级模式支持直接粘贴 refresh_token。
   - 115：「扫码授权」按钮（应用内显示二维码，115 APP 扫码，自动轮询换取 token）、根文件夹 ID（默认 `0`）；高级模式支持直接粘贴 refresh_token。
3. 浏览：列目录（百度按路径、115 按文件夹 ID），目录在前自然排序，与现有浏览体验一致；凭证失效时给出中文提示。
4. 打开：复用全局「打开策略」（auto / download / stream），语义与 WebDAV/SFTP 完全一致（auto=先整本下载到 raw/ 缓存失败回退流式，download=强制整本，stream=直链 Range 流式、不支持 Range 则整本）。
5. 封面：走现有 cover 管线（cover/ 磁盘缓存 → raw/ 本地缓存 → 流式解码第一页），对 CBZ/ZIP 等漫画格式生效。
6. Token 持久化与自动刷新：refresh_token 存入书源配置（新增 DB 列，模式同 M5 的 `port` 列）；访问令牌过期自动刷新并回写；刷新失败给中文错误并引导重新授权。
7. 清理：删除书源时清理缓存/记录，复用现有 `removeSourceWithCleanup`，确保不误删其他类型（吸取 M5 purgeStale 误删 SFTP 教训）。
8. 兼容性：旧数据可读；未配置新字段的书源不受影响；重复启动不重复加列（迁移幂等）。

## Acceptance Criteria

- [ ] Rust 单测通过（URL 构造、JSON 解析、token 刷新/回写逻辑），`cargo test` 全绿
- [ ] `flutter analyze` 0 issues；`flutter test` 全绿（含更新后的添加书源 widget 测试）
- [ ] 百度书源：授权添加 → 浏览 → 打开 CBZ（三种策略）→ 封面 → 重启后凭据保持
- [ ] 115 书源：扫码授权 → 浏览 → 打开 CBZ（三种策略）→ 封面 → 重启后凭据保持
- [ ] token 过期自动刷新；刷新失败有中文提示
- [ ] 删除书源后缓存/记录清理干净，且不影响其他书源
- [ ] `cargo build --release` 通过；用户手动 `flutter build windows --release` 验证

## Constraints / Non-goals

- 只读（浏览 + 下载），不实现上传 / 移动 / 删除 / 重命名
- 不实现搜索（R2 候选）
- 不实现夸克 / PikPak / OpenList 聚合（另议）
- 不依赖第三方 token 服务；百度授权走官方 OAuth 页，115 走官方设备码接口
- 合规：不公开分享、不图床/外链分发；用户凭据仅存本机 SQLite（与现有 password 同等保护级别）

## Notes

- 前置条件：项目方注册百度开放平台应用（个人实名 + 网盘基础服务权限）与 115 生活开放平台应用（申请制），凭证内置 RCH。115 审核未通过时的降级方案：表单允许用户自填 APP ID。
- 115 限制：同一账号同一应用最多 2 个有效 refresh_token（第三次获取顶掉第一个）；API 有频率限制（AList 默认 1 r/s），实现需节流。
- 百度限制：>20MB 文件下载直链须带 `User-Agent: pan.baidu.com` 请求头；dlink 有效期约 8 小时。
