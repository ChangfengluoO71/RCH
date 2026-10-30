# RCH 当前项目状态

> 当前状态唯一入口。最后核验：2026-10-30 15:05（Asia/Shanghai）。版本发布信息以 GitHub Release 为准；功能/验收分层以本文件为准。详细证据见 [本轮核验报告](../../audit/rch-next-step-2026-10-25/EVIDENCE_REPORT.md) 与 2026-10-30 的 OpenList 协议验证记录。

## 版本与代码基线

- 当前候选版本：**v0.7.0**（仓库 `app/pubspec.yaml` 为 `0.7.0+100700`；Release 工作流按 tag 计算 Android versionCode = `100000 + major*10000 + minor*100 + patch`，`0.7.0 → 100700`）。
- 当前分支：`local-ai/cover-quark-debug`，HEAD `48d3d99`；远程协议、凭据保险箱、S3 封面与远端统计修复尚未提交，v0.7.0 尚未打 tag / 发布。
- 基线 Release：v0.6.7（2026-10-17）。v0.6.7..HEAD 已包含其他 agent 的 `d6596f0 feat: add 115 workflows, Android daily runs, and tablet mode`（54 files）及本轮的远程协议实现。
- 本轮已完成 Windows Release 构建与本地 OpenList 只读验收；Android / 平板 / 115 云端仍缺真机或 Provider 验收，见下文。

## 核心功能状态

| 能力 | 实现状态 | 自动化测试 | 真实设备 / Provider 验收 | 发布状态 | 证据来源 |
|---|---|---|---|---|---|
| 本地与远程漫画阅读 | 已实现：统一 Document 与 ByteSource，远程按需读取 | Rust 文档/来源测试及 Flutter Reader 测试通过 | 2026-10-30 本地 OpenList 4.2.4 只读打开 SMB/SFTP/WebDAV/FTP/S3 多本漫画；未逐格式验收 | v0.7.0 候选 | `app/rust/src/document/`、`app/rust/src/source/`、`app/rust/src/reader.rs` |
| SMB / SFTP / FTP / S3 书源 | 已实现：SMB2/3 只读、SFTP 主机密钥与私钥、FTP REST/RETR、S3 SigV4 + Range | 对应 provider 测试、S3 单元测试与远端扫描契约通过 | 本地 OpenList 4.2.4：五协议均能列目录并打开多本漫画；S3 封面 240/240 | v0.7.0 候选 | `app/rust/src/source/smb.rs`、`sftp.rs`、`ftp.rs`、`s3.rs`、`app/lib/store/*_session.dart` |
| 远程阅读「自动」策略 | 已实现：缓存 → 流式 → 失败回退下载；显式「优先下载整本」保留进度 | 历史 Reader/策略测试通过 | 本轮 `cache\raw` 不增长，确认五协议流式 | v0.6.0 起；v0.7.0 覆盖 FTP/S3 | `app/lib/store/reader_source_policy.dart`、`reader_page.dart` |
| 凭据保险箱与主机指纹 | 已实现：Android Keystore AES-GCM、其他平台 secure storage；SFTP 指纹确认/固定/变更拒绝 | 保险箱与 host-key 测试通过 | Windows 实测指纹确认；Android Keystore 未真机验收 | v0.7.0 候选 | `app/lib/store/credential_vault.dart`、`app/lib/ui/sftp_host_key_dialog.dart` |
| 远程扫描、封面和远程文件夹 | 已实现主体：Engine、持久化、FRB、Dart Coordinator、文件夹阅读、封面处理 | Engine、Coordinator、Provider contract 和 UI 回归测试通过 | 本地 OpenList 五协议各 239 本漫画；失败扫描/取消/恢复未做完整 GUI 矩阵 | v0.7.0 候选 | `app/rust/src/remote_scan/`、`app/lib/store/remote_scan_coordinator.dart` |
| Android 宽页拆分与虚拟页进度映射 | 已实现：智能/强制/不拆分及原始页进度映射 | `reader_display_test.dart` 等模型/界面测试通过 | 本轮无 Android 设备 | v0.6.4 起发布 | `app/lib/store/reader_display.dart`、`app/lib/ui/reader_page.dart` |
| Windows / 平板双页 | 已实现：相邻原始页并排显示 | Reader 模型/Widget 测试通过 | 本轮无 Windows 平板操作验收 | v0.6.5 起发布 | Reader 代码、v0.6.5 CHANGELOG |
| 本地/云端刮削与元数据导入 | 已实现：本地 Catalog-only 识别与独立 E 站导入 | Rust parser/match/import 测试通过 | 无真实 E 站账户验收；M8 真样本闭环未完成 | E 站导入 v0.6.2 起发布 | `app/rust/src/scraper.rs`、`eh_import.rs` |
| WebDAV 状态同步 | 已实现：快照、三方合并、落库和同步历史 | Merge/history 自动化通过 | 本轮未连真实同步 Provider；建连失败仍不生成 `sync_history` | v0.7.0 候选 | `app/lib/store/sync_engine.dart`、`app/rust/src/sync/`、ADR-027 |
| EH 订阅与每日扫描 | 已实现：检查、筛选、保存、手动工作流和 Windows Daily；Android 每日后台默认可选关闭 | EH 定向测试与 Flutter/Rust 全套通过 | 10-17 用户报告在应用内验证（`USER_REPORTED`）；Android WorkManager 未真机验收 | v0.6.6/v0.6.7；Android 后台 v0.7.0 候选 | `app/rust/src/eh_subscription.rs`、`app/lib/store/eh_daily_coordinator.dart`、`eh_daily_background.dart` |
| 115 Cookie Web BT | 已实现：Torrent 上传/解析、BT 提交、只读查询、批次账本及未知状态恢复 | Mock/SSP/Batch/FFI 测试覆盖 | 10-17 用户称已在应用检验（`USER_REPORTED`）；本轮未请求 115、未验证新 BT 或云端文件闭环 | v0.6.6/v0.6.7，实验性 | `app/rust/src/source/cloud115_offline.rs`、`app/lib/store/cloud115_bt_batch.dart` |
| Windows AI 超分 | 已实现 Windows 执行器与模型资产 | 有限单测；本轮未运行推理 | 本轮无真实图片推理验收 | Windows 组件随当前版本提供 | `app/lib/store/ai_upscale_manager.dart`、`app/rust/src/ai/` |
| Windows / Android 更新器 | 已实现下载、SHA-256 校验、重试和安装交接 | 更新器、下载交接测试通过 | Android MuMu 0.6.1→0.6.2 有历史安装验证；Windows UAC/安装 GUI 尚未完成 | v0.6.7 | `app/lib/store/update_manager.dart`、`app/test/update_manager_test.dart` |

**标记规则：**用户亲自报告只记作 `USER_REPORTED`，不替代独立测试、GUI 录像或 Provider 证据。历史 CI 结果不是本轮重跑结果。

## 本周 0.7.0 验证记录

- 环境：OpenList 4.2.4（HTTP 5244、FTP 5221、SFTP 5222、S3 5246），bucket `1` 映射 `/日漫`；RCH 数据 `%APPDATA%\RCH\RCH`，缓存 `D:\Documents\RCH`。
- 只读结果：SMB `smb_1793332281399`、SFTP `sftp_1793332390650`、WebDAV、FTP、S3 `s3_1793338754522` 均能列目录 240 本、打开多本漫画翻页；S3 封面可用 240/240、失败 0。
- 流式：五协议 `cache\raw` 目录数保持 6 不增长；仅显式「优先下载整本」创建整书缓存。
- 统计：远端书源侧栏各显示 239 本漫画（240 行中 `.cbz` 与 `.zip` 同书按扩展名归一化去重，属预期）。
- 测试：`flutter analyze` 通过；`flutter test` 411 passed / 1 skipped；`cargo test --lib --locked -- --test-threads=1` 577 passed / 4 ignored；`cargo test --locked --test remote_scan_contract` 18 passed。全量 Rust 集成测试因本机页面文件不足未跑完，交给 CI。

## 已确认的用户决策

- 115 继续走现有 Cookie Web 认证路线；不默认切换到 Open API。
- 10-11 的 EH→115 P0 研究记录保留为历史研究状态；v0.6.7 已移除运行时代码门禁，不要把旧研究 Gate 恢复成代码开关。
- 上传成功、BT 提交、远端任务确认是不同状态。`UNKNOWN` 不得盲目重传、报告成功或删除本地 Torrent。
- Daily 自动工作流 Windows 默认关闭；Android 每日后台为可选、默认关闭，受系统约束调度。
- 已进入当前代码及 Release 的能力，不因旧 Trellis 状态或历史设计文档再次开发。先核实当前代码、版本和验收缺口。

## 当前问题与验收缺口

### 当前可复现的代码缺口

- EH 保存目录选择先更新可见/内存规则，再调用 `saveRules()`；写入异常会向上抛出，当前选择回调没有回滚或用户可读错误提示。代码位置：`app/lib/ui/eh_subscription_panel.dart` 的 `_pickDir()` 与 `app/lib/store/eh_subscription_store.dart` 的 `saveRules()`。
- 远端 Book 计数的父路径推导已修复；仍需在全新数据库上补一条回归测试，避免未来扫描模型变化再次出现「0 本漫画」。

### 已实现但缺真实验收

- Android EH 目录选择、WorkManager 后台、重启后权限/路径可用性、无 115 只保存 Torrent：代码与自动化覆盖存在，但没有 Android 设备 GUI 验收。
- Android 宽页拆分/进度映射、Windows/平板双页、条漫快速滚动/远跳和阅读恢复：实现与自动化覆盖存在；当前版本 GUI/设备矩阵未完成。
- SFTP 主机密钥变更提示、私钥认证与 SMB Guest/域账号：本地 OpenList 与代码测试通过，但未覆盖真实企业 NAS 的加密共享/域环境。
- 远程扫描仍需真实 Provider 和异常恢复验收；不重写已有 Engine。
- 115 Web BT 代码区分上传与提交，未知状态有账本与自动化测试；本轮没有真实云端复核。
- Windows UAC 更新器尚缺 GUI 验收。

### 长期架构债务

- WebDAV 建连失败仍只更新 `lastError`，没有对应 `sync_history` 尝试行。
- Repository/Use-case 覆盖不完整、SQLite 单连接锁等待未量化、旧 Downloader 职责边界不清。先按用户影响测量和局部处理，不安排大型重构。

### 历史状态或已废弃计划

- EH→115 `P0_WEB_WRITE_AUTH_BLOCKED` / `P0_REAL_PASS=NO` 是 10-11 研究记录，不是当前运行时门禁；P1 长期上传队列也没有在 v0.6.7 启动。
- `docs/architecture.md` 仍保留 v0.6.2 的实现快照内容；它是架构背景，不是当前 Release/验收入口。
- Trellis `planning` / `in_progress` 用于任务和验收追踪，不单独证明功能未实现；不为减少任务数批量归档。
