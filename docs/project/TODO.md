# TODO.md — 当前项目状态与后续工作

> 更新日期：2026-10-05。此文件只保留当前状态、可执行待办和交接要点；逐轮实现证据留在 [LOG.md](LOG.md)，任务进度以各 Trellis `task.json` 为准。
> 当前稳定发布：**v0.6.2**（`app/pubspec.yaml` 为 `0.6.2+100602`）。本地后续修复尚未发布。

## 状态口径

- [README](../../README.md) 说明当前用户可用功能；[用户手册](../user-guide.md) 说明具体操作。
- [SPEC](SPEC.md) 记录目标与冻结契约，不随普通状态整理修改。
- 本文件汇总 Doing / Waiting / Backlog / Done；实现细节和历史原因以 LOG、任务 PRD 及 evidence 为准。
- 任务未完成所有验收时保持 `in_progress`；已有代码或自动化测试通过，不等同于跨平台/真实 provider 验收完成。

## Doing（进行中）

### P0：全云端远程扫描与封面索引

任务：[父任务](../../.trellis/tasks/09-14-remote-cloud-scan/task.json)

- 父任务仍为 `in_progress`，5 个子任务尚无一个完成归档。
- 当前实现已有扫描引擎、持久化、FRB API、Dart coordinator、文件夹阅读和封面处理；剩余重点是按各子任务验收真实 provider、恢复/失败路径与完整工程门禁。
- manifest 子任务仍在 planning；worker、文件夹阅读、封面清理、UI/跨 provider 验证任务仍在进行。
- 扫描 UI 当前只保留失败重试入口；暂停/继续、手动增量/全量重扫按钮已按第92轮决定移除。不要把旧设计稿中的按钮要求重新当成当前需求。
- 具体门槛见[执行计划](../../.trellis/tasks/09-14-remote-cloud-scan/implement.md)及各子任务 evidence。

### P1：发布后体验整改

任务：[父任务](../../.trellis/tasks/08-30-post-release-feedback-remediation/task.json)

- 父任务 3 个子任务中 1 个已完成归档。
- 条漫快速下拉回跳修复已于 2026-09-23 完成并实机确认；2026-10-02 又接通 `WebtoonNavigationModel`，修复流式加载下未测页面按 0 高度导致的条漫跳页偏移，自动化回归已通过。
- 应用内更新交接：下载/校验/重试和入口统一已实现；MuMu 上 0.6.1→0.6.2 APK 覆盖安装实测通过。Windows 更新器已构建并通过自动化检查，桌面窗口/UAC smoke 尚待完成。
- 随机阅读反复命中少数漫画已修复：候选按 `sourceId` 精确绑定，并支持本地图片目录；静态分析通过，随机阅读运行时复验待后续确认。
- 阅读器恢复与跳页体验已补齐：打开阶段可见、失败可重试、Quark 登录过期可进入扫码恢复，缺失书源映射可触发刷新；条漫远跳使用高度估算和有限窗口读取。分页模式优先加载当前页，预取让位于可见页，原图先显示、AI 缓存后应用。
- 阅读器工具栏已加入与主页一致的骰子图标；随机候选排除当前漫画并按来源与路径核验。最新桌面 Debug 构建和静态检查已通过，用户实测仍待完成。
- Windows PDFium 已改为构建时自动准备和安装，避免应用打包后打开 PDF 才发现动态库缺失。
- 封面加载性能任务仍在 planning。

### P2：M8 Catalog-Only 通用识别闭环

任务：[M8 父任务](../../.trellis/tasks/08-08-m8-smart-scraping/task.json)

- 当前状态为 `in_progress`；解析器和自动物化基础已实现，真实样本验证仍是进入 canonical/provider 阶段前的门槛。
- M1–M6 子任务仍在 planning。
- **不要与已发布的 E 站导入混为一个状态**：E 站导入是 v0.6.2 的独立用户流程；M8 是只基于本地 catalog 的通用识别及后续 canonical/provider 工作。

## Waiting（等待验证）

- **远程扫描**：完成各云端 provider 的真实样本与失败恢复验收；缺设备或凭据时保持 pending，不以 fake contract 代替。
- **应用内更新**：Windows 安装向导/UAC（含取消后重试）桌面 smoke；Android 未知来源授权、系统安装确认与覆盖安装已在 MuMu 完成。
- **阅读器与随机阅读**：在当前 Windows Debug 应用中复验远距离条漫跳转、跳转后的页码映射、连续翻页加载和随机候选去重；最新性能优化尚无用户侧运行确认。
- **M8**：真实样本/100 本 truth set 验证，并明确解析覆盖、歧义和缺失原因。

## Backlog（待办）

### 已登记的用户体验任务

- [首次启动缓存引导](../../.trellis/tasks/08-01-first-run-cache-guide/task.json)
- [详情页信息自定义](../../.trellis/tasks/08-01-detail-info-settings/task.json)
- [详情页阅读统计](../../.trellis/tasks/08-01-detail-page-stats/task.json)
- [退出行为](../../.trellis/tasks/08-01-exit-behavior/task.json)
- [AVIF 支持](../../.trellis/tasks/08-02-avif-support/task.json)
- [封面加载性能](../../.trellis/tasks/08-02-cover-loading-perf/task.json)

### 状态与架构跟进

- 同步在 WebDAV 建连阶段失败时，目前会更新 `lastError`，但不会生成对应的 `sync_history` 尝试记录；按 ADR-027 补齐可检索的失败阶段、时间和错误信息。详细范围见[任务](../../.trellis/tasks/10-01-sync-connect-history/prd.md)。
- E 站导入剩余的 widget 渲染覆盖和旧数据兼容考古，作为发布后测试完善项；不影响 v0.6.2 已发布的主流程。
- 阅读器打开协调与全库随机选择已抽到 `book_open_coordinator.dart`、`random_read_selector.dart`；Repository / Use-case 边界仍不完整，按渐进收敛策略在触及功能时下沉逻辑，不安排大规模重写。
- 架构审计建议：明确旧 `downloader` 模块和书源客户端的职责；后续触及大模块时按职责拆分；先量测 SQLite 全局锁等待，再决定是否改变连接模型。当前不安排多 crate 重构。

## Done（v0.6.2 已交付）

- E 站元数据导入：唯一作品名命中可自动导入；歧义和未匹配结果不自动写入；字段只补空、不覆盖。
- 卷 / 话信息显示，并纳入同步与 `.rchpkg` 备份/恢复载荷。
- 条漫快速下拉回跳锚点补偿修复，50+ 页不等高条漫实机确认。
- 远程书源会话复用、后台扫描让路，以及可见的应用内更新下载与安装交接界面。
- 墓碑随行复活语义修复（ADR-030）。

细节见 [v0.6.2 发布说明](../releases/release_notes_v0.6.2.md) 和 [LOG 第131–134轮](LOG.md)。

## 已知安装兼容说明

Android v0.6.2 正式包的 `versionCode` 为 `100602`。若设备装有更高版本号的开发测试包（例如 `102602`），系统不允许直接覆盖；卸载会清除本地应用数据。此差异已在发布说明中记录，当前没有统一版本号口径的开发任务。

## 历史记录说明

旧 TODO 曾混入第81轮交接、逐轮调试细节和已交付事项。这些历史证据仍保存在 append-only 的 [LOG.md](LOG.md) 与对应 Trellis 任务/归档中；不要把旧报告或已归档任务里的历史状态当作当前看板。
