# 云端阅读与海报墙请求治理：实施计划

## 同批执行边界

本任务与 `08-30-update-download-install-handoff` 组成 `cover-and-update` 实施批次。两项可以使用同一工作分支、同一全量验证轮次，但不共享状态机、平台通道、测试替身或提交；本任务的 Range 安全边界、Rust/FRB 生成和真实书源验证仍必须独立完成。

## 前置关卡：基线与假件

- [ ] 在不接触真实凭据的 fake `ByteSource`/provider 场景中记录：请求数、Range 块数、并发、取消、缓存命中、429/超时/Range 不支持结果和首屏耗时。
- [ ] 增加 `206 + Content-Range`、`200`、`416`、缺失/不匹配 `Content-Range` 的协议假件；确认 `Accept-Ranges` 单独出现不等于可用。
- [ ] 覆盖缓存命中、未命中、快速滚动、当前页与预取竞争、封面竞争五种固定场景；将基线与目标阈值写入任务 `research/` 或验证记录。
- [ ] 以一份可脱敏的真实书源手工验证清单补充协议差异，不能把 Cookie、token、完整下载 URL 写入仓库。

## 步骤 1：缓存身份和安全读取边界

- [ ] 定义并测试稳定封面缓存身份；迁移所有远程封面读/写/清理路径，消除 raw 本地路径与远程路径混用。
- [ ] 实现按资源版本的 Range 能力/负能力缓存；目录刷新、size/mtime 改变或 TTL 到期后才重新探测。
- [ ] 为所有远程来源补齐 cache-only 封面读取；关闭开关时只使用该路径。
- [ ] 为 remote-partial-only 封面策略添加 Range/随机访问不支持测试，断言不调用 `download_to_raw_cache`，也不读取 `200` 的完整响应体。

## 步骤 2：设置与 Flutter 调度

- [x] 在 `AppSettings` 添加默认开启、JSON 往返兼容的“云端封面加载”字段，并在“远程书源”全局设置中展示开关。2026-09-07：缺失字段迁移为 `true`，显式保存的 `false`/`true` 均保持不变；回归测试已覆盖。
- [ ] 将 `ComicCover` 改为设置变更可响应的缓存优先流程：关闭时 cache-only，开启时可见项进入调度器。
- [ ] 实现按稳定 key 去重、订阅者计数、离屏等待取消、按书源并发/退避，并为调度逻辑写独立测试。
- [ ] 在书源浏览页聚合 `rangeUnavailable`，同书源单次浏览只提示一次；单卡继续使用既有占位，不能产生 Toast/SnackBar 风暴。

## 步骤 3：阅读优先级与 Rust 接口

- [ ] 在 Rust 定义封面获取策略和安全的分类结果；更新所有提供方的封面 API，生成 FRB 绑定。
- [ ] 为当前页、预取和封面建立共用的自定义阻塞优先级预算，并为当前页保留许可；验证当前页优先于预取、预取优先于封面，且已开始 blocking I/O 不被错误地宣称为可中断。
- [ ] 限制 `Reader` 后台预取对预算的占用；同页前台/预取去重语义保持不变。
- [ ] Rust API 改动后关闭正在运行的 RCH，再执行 `app/codegen.ps1`，以同步 Dart 绑定和 release DLL。

## 步骤 3.5：自定义封面显式下载确认

- [ ] 为 `CoverEditorPage` 建立缓存优先、局部读取优先的打开路径，禁止它直接按 `auto` 触发整本下载。
- [ ] Range 不可用时返回带来源、文件名和可得大小的类型化结果，Flutter 显示“下载整本/取消”确认框。
- [ ] 只有确认后才调用既有下载打开路径；拒绝后零下载，确认后显示下载进度并允许返回。

## 步骤 4：回归、性能与人工验证

- [ ] 自动化验证关闭开关为零新增远程封面请求、缓存封面仍显示、同 key 不重复请求、离屏任务可取消、各失败分类有界退避。
- [ ] 自动化验证 Range 不支持不会下载 raw 整本、`200` 全量响应不会被消费，且封面失败不会阻塞当前阅读页。
- [ ] 自动化验证自定义封面拒绝下载为零网络整本下载，确认下载只发起一次并显示进度。
- [ ] 使用步骤 0 的同一场景出具前后请求数、首屏和滚动数据；只有达到记录的阈值才进入发布评审。
- [ ] 对 WebDAV、SFTP、百度、115、115 Cookie、夸克按可用账号做一次脱敏手工冒烟；无账号的提供方明确标为待验证，不伪造通过。

## 验证命令

```powershell
cd app/rust
cargo test

cd ..
.\codegen.ps1   # 仅 Rust API 改动后；先关闭 RCH
flutter analyze
flutter test
```

## Execution update (2026-09-11)

- [x] Remote cover fetching defaults to enabled for new/missing settings while preserving explicit `false`; the setting is persisted and exposed in global remote-source controls.
- [x] Added bounded visible-cover scheduling (dedupe, FIFO cap, cancellation of pending work), cache-first loading, and one notice per source-browser session when Range is unavailable.
- [x] Safe cover policy keeps the placeholder on Range failure; custom covers ask for an explicit whole-book download only after the safe partial path is unavailable.
- [x] Added strict 206/Content-Range validation and bounded 403/429/416 downgrade categories; no cover path silently falls back to a whole-book download.
- [x] Automated verification: cover scheduler/consent/settings suites passed; `cargo test source:: -- --test-threads=1` passed (48 tests); `flutter analyze --no-pub` passed.
- [ ] Real provider smoke and cross-provider request/first-paint measurements remain pending; no credentials or URLs are stored in the repository.

## 回滚点

- 步骤 1 失败：保留基线与测试，回退到当前 cache-only 行为。
- 步骤 2 失败：关闭全局开关即可停止新远程封面请求，不影响已有缓存。
- 步骤 3 失败：先回滚请求预算层，保留安全的 Range-only 封面策略和调度回归测试。
- 任一真实书源出现协议变化：该来源降级为占位+退避，不以全书下载兜底。
