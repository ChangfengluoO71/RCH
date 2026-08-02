# M5 书源扩展（SMB / SFTP）
## Goal

为漫画库新增 **SMB 与 SFTP** 两种网络书源，沿用现有「目录浏览 → 流式阅读 → 本地缓存」链路，让 NAS / Linux 服务器上的漫画可直接在 RCH 中阅读。

## Background（已确认事实）

- 书源抽象：Dart `BookSource`（type: `'local' | 'webdav'`，含 url/username/password/path 字段）[app/lib/store/models.dart:6]
- Rust 侧统一字节流抽象 `ByteSource`（len + read_at）+ 目录条目 `Entry` [app/rust/src/source/mod.rs:20]
- 本地书源 `LocalFile` + `list_dir`（自然排序）[app/rust/src/source/local.rs]
- WebDAV 书源已实现 session / 流式 / 整本下载回退，可作为 SFTP 的参照实现 [app/rust/src/source/webdav.rs]
- 打开链路：`openLocalBook(path)` / `openWebdavBook(session, path)` [app/rust/src/api/book.rs:74, app/rust/src/api/source.rs:99]
- 添加书源 UI：SegmentedButton（WebDAV / 本地目录）[app/lib/ui/home_page.dart:474]
- SPEC M5 里程碑：MOBI/CBR 等格式已完成，剩余为书源扩展（SMB / SFTP / 更多网盘）[SPEC.md §10]
- 平台约束：Windows-first；Windows 原生支持 UNC 路径访问 SMB 共享，可零成本复用本地文件链路

## Requirements

- **R1** SMB 书源：通过 UNC 路径（`\\server\share`）接入，复用 `list_dir` + `LocalFile`；补充路径校验与连接失败/未授权提示。
- **R2** SFTP 书源：新增 Rust 侧 SFTP 目录枚举与 Range 随机读（接口对齐 `ByteSource` / `Entry`）；不支持 Range 时整本下载到 raw/ 缓存回退（沿用 WebDAV 回退逻辑）。
- **R3** 书源管理 UI：新增书源对话框扩展为「本地目录 / WebDAV / SMB / SFTP」四种类型；SFTP 提供服务器地址、端口、用户名、密码、初始路径字段；凭据持久化沿用现有 BookSource JSON（密码字段已有掩码）。
- **R4** 全链路可用：source_browser 目录下钻、封面（SFTP 走懒加载，参照 WebDAV 保守策略）、打开阅读、阅读记录 key 与现有来源类型兼容。
- **R5** 依赖选型（建议）：SFTP 优先评估 `russh`（纯 Rust 异步）或 `ssh2`（libssh2 绑定，Windows 需预编译库）；SMB 不做自研协议客户端，仅走 UNC。

## Acceptance Criteria

- [ ] 添加 `\\server\share` 类型 SMB 书源后，浏览、打开、阅读与本地目录行为一致；无权限时给出明确错误提示
- [ ] 添加 SFTP 书源后可枚举目录、打开漫画；支持 Range 的服务器流式阅读，不支持的整本下载回退并显示进度
- [ ] 重启应用后凭据可用，无需重新输入
- [ ] 删除书源时同步清理其阅读记录与元数据（沿用 `removeSourceWithCleanup`）
- [ ] `flutter analyze` 0 issues；`cargo test --lib` 通过（新增 SFTP 路径拼接/URL 解析单测）

## Out of Scope

- 阿里云盘/百度网盘等自定义 API 网盘（无标准协议，需单独评估）
- Android 平台 SMB/SFTP（M6 暂缓）
- 断点续传增强、传输速率限制配置
- 自研 SMB 协议客户端

## Open Questions

- 无阻塞问题。实现时验证 R5：russh vs ssh2 在 Windows cdylib 环境下的编译成本与稳定性。
