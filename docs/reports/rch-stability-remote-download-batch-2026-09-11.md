# RCH Stability & Remote Download Batch Report

日期：2026-09-11  
范围：飞书 RCH 项目组最新两条反馈，以及上轮工程剩余项的本轮实现与验收

## 结论

当前结论为 **YELLOW（可继续开发，不宜立即发版）**。

本轮代码和自动化回归已经形成可审查的工作区结果，Rust/Flutter 全量测试与 Flutter 静态分析通过；但没有 Android 真机、Windows 安装器/UAC 和真实网盘账号的现场证据，不能把本批次标为发布候选完成。版本、tag 和发布包均未变更。

## 1. 反馈与基线

飞书 `RCH项目组` 最新两条反馈为：

1. 2026-09-05：阅读完成后可在全局设置删除远程下载的漫画内容缓存，但保留封面预览；下次阅读重新下载，以缓解磁盘占用。
2. 2026-09-04：远程漫画整本下载速度慢，希望优化，并参考 aria2 一类的并发下载体验。

本轮还承接了上次的条漫页码稳定性、海报墙封面请求治理、更新下载/安装交接和 M8 边界收敛。

代码基线：分支 `local-ai/cover-quark-debug`，`HEAD=d112433`，精确 tag 为 `v0.5.7`，`master...HEAD` 无提交差异；`app/pubspec.yaml` 仍为 `0.5.7+100507`。工作区在本轮开始前已有大量未提交改动，本轮不重置、不覆盖、不自动提交。

## 2. 已完成的工程改动

### 条漫导航与阅读进度

- 新增 `WebtoonNavigationModel`：稳定页、视口页、待处理目标、generation token、单调的未知高度估算、实测高度和过期目标拒绝。
- `_go`、跳页和滚动观察统一经过模型；用户手势会取消旧的程序化目标，阅读进度只在目标完成或滚动稳定后提交。
- 修复 `animateTo` 发出的 `ScrollEndNotification` 早于 Future 完成的竞态；现在不会提前清除仍有效的程序化目标。
- 自动化：条漫模型/手势回归通过；真实 Windows/Android 上 50+ 页且高度差异明显的人工冒烟仍待补。

### 远程整本下载与请求治理

- 增加不含凭据、路径和签名 URL 的 `DownloadIdentity`、`TransferMetrics` 和文件粒度 `DownloadCoordinator`；同一版本同一文件只保留一个 worker，消费者可以加入或脱离，失败任务可重试。
- HTTP 下载采用小规模 Range pilot（2–4 段），要求探测和每个分段都满足 `206 + Content-Range + Content-Length + ETag`；任一不确定即删除 `.part` 并退回一次串行下载。
- 403、429、416、超时、Range 不可靠和短读都归入有限、脱敏的降级原因；不使用 aria2 外部进程或 RPC。
- 串行和分段路径均写入 `.part`，flush/sync 后原子提交；版本变化或校验失败不会暴露半成品。
- 当前真正接入该分段 pilot 的是 WebDAV；百度、115、夸克、SFTP 仍走各自的安全串行路径，因此不能宣称“所有网盘均已并发加速”。

受控本机假 HTTP 服务、20 MiB 数据的最后一次实际运行记录如下（吞吐为 B/s；这是协议/实现基准，不是真实网盘速度）：

| 模式 | elapsed | throughput | first byte | bytes | fallback |
| --- | ---: | ---: | ---: | ---: | --- |
| sequential（无版本） | 32 ms | 655,360,000 | 9 ms | 20,971,520 | `version_unavailable` |
| segments-2 | 37 ms | 566,797,838 | 10 ms | 20,971,520 | `range_unreliable` |
| segments-4 | 38 ms | 551,882,105 | 7 ms | 20,971,520 | none |

本机并发和测试服务调度会使 4 段偶发安全降级；这组数字不构成真实服务提速承诺。下一步应在真实 WebDAV/115/夸克/百度账号、100 MiB 和 300 MiB 文件上分别记录串行、2 段和降级耗时、首字节、峰值内存和失败率。

### 阅读完成后的远程缓存清理

- `deleteRemoteDownloadCacheAfterFinish` 为全局显式开关，默认关闭；只有稳定到最后逻辑页、最后一个阅读 lease 释放时才触发。
- 仅清理远程来源的 page/raw 内容缓存，保留封面、元数据、阅读历史、来源配置和原始文件；清理失败保留待重试状态，不把异常抛回 Reader。
- 当前为进程内 lease 协调，应用崩溃或强杀时不会凭空推断“已读完”；AI 派生缓存仍按后续任务处理。

### 封面请求与降级

- 新设置/缺失字段默认启用远程封面，显式 `false` 仍保持关闭；加载顺序为缓存优先，再进入有界、去重、可取消的可见封面调度器。
- Range 不可用时继续使用占位封面并按书源会话只提示一次；不会静默把整本下载当封面降级。
- 自定义封面只有在安全局部读取不可用时才询问“是否下载整本”，拒绝即零下载；确认后沿用现有整本下载入口。
- Rust 层严格验证 `206 + Content-Range`，覆盖 200、416、缺失/畸形/不匹配响应以及 403/429 分类。

### 更新下载与安装交接

- 更新包先写 `.part`，完成后再原子替换；已校验包在平台交接失败时保留，可重试且不会触发破坏性重下。
- 并发点击共享同一个下载 Future；Windows 使用参数化启动参数，Android 保留 APK 并展示可重试失败状态。
- 自动化覆盖手动下载、Windows/Android handoff、UAC/未知来源失败恢复、URL/token 脱敏和已下载包复用。

## 3. M8 边界与剩余工程

M8 继续保持 **Catalog-first / local-only**：读取本地 SQLite catalog snapshot 的文件名和上级目录，生成可解释 proposal；RemoteOnly 不访问 ByteSource、远程书源、Downloader、Provider 或同步传输。现有 scraper、proposal、同步编排和 Rust 规则回归已纳入自动化验证，但 M8 主任务仍为 `in_progress`，等待真实样本/人工真值和 canonical 阶段验收。

线上智能刮削、E-site 元数据抓取、115 自动化不进入本轮主程序，保留为未来插件边界；插件需要另行定义授权、脱敏输入、结果合同和 canonical 确认门。

### 未提交/未完成任务规划盘点

当前 Trellis 任务状态中，`in_progress` 为：父整改批次、条漫稳定性、海报墙封面、更新交接和 M8 主任务。仍处于 `planning` 的工程任务为：

- P0：M8-M1 Catalog-only 识别、M8-M2 canonical identity/migration、M8-M4 本地候选排序、M8-M5 review/confirmation、M8-M6 corpus validation。
- P2：AVIF 支持、详情页信息显示、详情页统计、退出行为选择、首次启动缓存目录引导。
- P3：M8-M3 在线 Provider/智能刮削插件规划（已明确延期，不是本轮依赖）。

这些规划文档不应在本批次通过改名或勾选伪装成已完成；先完成本报告列出的设备、真实 provider 和本地 corpus 关卡，再分别启动对应任务。

主要未完成项：

- P1：真实网盘 Range/签名 URL/限流行为与性能基准；Windows 安装器/UAC；Android APK 安装确认和未知来源权限；50+ 页条漫真机冒烟。
- P2：其余 provider 的文件级 single-flight/metrics 接入，100/300 MiB 内存和取消测试；下载器与 Reader 解压/读取的联合占用治理；缓存清理在崩溃恢复和多窗口场景的设备验证。
- P2：M8 100 本本地 catalog corpus、proposal 覆盖率和人工真值复核；在此之前不推进在线插件实现。

## 4. 验证证据

- `cargo test --workspace --locked -j 1 -- --test-threads=1`：Rust 库 265 passed、2 ignored；`tests/reader_l1_hit_deadlock.rs` 1 passed；doc-tests 0。
- `cargo test downloader::tests -- --test-threads=1`：12 passed；`cargo test source:: -- --test-threads=1`：48 passed；`cargo test reader:: -- --test-threads=1`：3 passed。
- `flutter test --no-pub`：105 passed；新增的条漫、封面、缓存清理、更新交接和设置用例均通过。
- `flutter analyze --no-pub`：No issues found。
- `git diff --check`：退出码 0，仅报告工作区换行符转换提示。
- `cargo fmt --all -- --check`：仍报告工作区既有的多处格式差异；没有在共享脏工作区执行全量重排，避免覆盖其他未提交改动。
- 设备探测仅发现 Windows desktop、Chrome、Edge；`adb` 不在 PATH，没有 Android 真机/模拟器可用。因此设备项只能记为 `AUTOMATED PASS / DEVICE VALIDATION PENDING`。

## 5. Git 与发布判断

本轮没有创建 commit、分支、tag 或新版本号；建议后续按“下载器核心 → WebDAV 接入 → 缓存清理 → 封面 → 更新 → 条漫”分组提交，生成文件与各子任务保持可回滚边界。`.pi/`、`graphify-out/` 和 `auth-feishu-qr-20260906.png` 均未操作。

在真实 provider、Windows 安装器、Android 安装确认和条漫真机冒烟补齐前，发布判断保持 **YELLOW：可继续开发，暂不发版**。
