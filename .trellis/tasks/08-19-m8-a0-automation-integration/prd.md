# M8-A0 自动化调度与同步集成 PRD

## Goal

让刮削成为一个可恢复、可观察、可合并到现有自动同步周期的后台流程，同时保持严格的 local-first 边界：刮削只消费本地 SQLite 中已经存在的 catalog snapshot；它不能刷新 WebDAV、SFTP、115、夸克、百度网盘或任何远程书源。

## Product outcome

应用启动并完成数据库、目录树和同步配置加载后，系统自动运行一个协调周期：按既有策略处理同步，再针对本地 catalog 的新增/变化生成刮削 proposal；用户确认后才写入 canonical work，确认事务只标记 sync-dirty，下一次正常同步再负责传输。没有同步配置时，catalog-only 刮削仍然可以独立工作。

## Requirements

### R0-1 / Shared automation runtime

- 复用现有 `SyncEngine` 的启动、2 秒本地变更防抖、60 秒轮询、限流和退避语义，但由一个协调器统一调度任务状态、去重、顺序和生命周期。
- 任务类型至少包括 `sync_transport`、`catalog_scrape`、`provider_enrichment`；Provider 是可选的低优先级任务。
- 任务必须可持久化、可恢复、可取消，并有 `queued/running/succeeded/degraded/retry_wait/failed/cancelled` 状态。

### R0-2 / Trigger and ordering

- **单一调度器所有权**：当前 `SyncEngine` 已有启动、60 秒 Timer 和 2 秒防抖；接入协调器后不能并行保留第二套 Timer。协调器成为唯一触发所有者，`SyncEngine` 降为可调用的同步执行器/状态适配器，现有 UI `syncNow`、`setAutoSync` API 保持兼容。
- 启动：数据库和 catalog 已加载后，先按现有设置安排同步；同步周期完成后只读取本地 SQLite 的 catalog revision delta 并安排刮削。
- catalog 变化：由已持久化的 catalog/index revision 触发 2 秒防抖；不得因为发现字段缺失而刷新远程书源。
- 事件分类：catalog/index revision 只触发 `catalog_scrape`；canonical sync-dirty 只触发 `sync_transport`；proposal、candidate、evidence 和 Provider cache 的写入不得触发任一远程同步事件。
- 同步完成：pull/apply 产生的本地 catalog 变化进入同一刮削队列；不得重复处理同一 `book_key + catalog_revision + rule_version`。
- 周期 tick：沿用现有前台 60 秒 tick；同步和刮削各自判断是否到期，不能让刮削触发额外的远程 revision/stat 请求。
- 手动操作：允许“立即刮削”（本地-only）和“运行完整周期”（同步完成后刮削）；两者均不能把刮削变成书源扫描。

### R0-3 / Lane isolation

- `catalog_scrape` 任务只能接收 `CatalogSnapshot`，不得持有 `ByteSource`、Downloader、远程 source session、远程 URL 或 sync transport handle。
- `sync_transport` 保留现有 WebDAV sync state、pull/apply/push 和轻量 remote revision 检查；这些能力不能被 scraper 复用。
- `provider_enrichment` 只能访问 AniList/Bangumi 元数据 API 和本地 provider cache；Provider 失败不得阻塞同步或本地 proposal。
- 三条通道最多各自一个 active job；协调器负责顺序和预算，不把三种 I/O 抽象合并成一个万能 client。

### R0-4 / Confirmation and sync

- 自动刮削只生成或更新 working proposal，不自动确认、不静默覆写 canonical metadata。
- `confirm_proposal` 完成本地 SQLite transaction 后返回，并发出 canonical-dirty 事件；它不 inline 调用 `sync_now`、sync actor 或 WebDAV。
- 只有 confirmed canonical 数据和已有 sync-dirty 记录进入同步通道；scrape working state、候选、证据和 Provider cache 不进入同步快照。

### R0-5 / Failure and recovery

- 刮削解析失败、上下文缺失和角色冲突进入可解释的 `degraded/failed` 结果；不得重试成远程目录请求。
- Provider 离线、超时、限流或无结果只影响 enrichment 状态；本地 proposal 保持可用。
- 同步失败继续使用现有 30 秒到 15 分钟退避和 429/503 冷却；同步失败不能阻塞本地刮削，刮削失败也不能取消同步。
- 应用退出/进入后台时持久化未完成任务；重新启动后按 job key 去重并从可安全重试的状态恢复。当前仍以应用前台生命周期为有效运行范围，不承诺被系统杀死后继续后台执行。

## Acceptance criteria

- [ ] 启动、catalog 变化、同步完成、周期 tick 和手动入口都能产生正确的任务，不产生重复 job。
- [ ] 同步完成后新增/变化 catalog 自动生成 proposal；没有 WebDAV 配置时 catalog-only 刮削仍可用。
- [ ] `RemoteOnly` asset 的自动刮削期间，书源请求、`ByteSource::read_at`、HEAD/stat/PROPFIND、下载和封面读取均为 0；允许的 I/O 只有本地 SQLite、provider cache 和显式 Provider API。
- [ ] Provider 失败不影响 proposal、阅读、书架和同步状态。
- [ ] 确认 proposal 后只有本地 canonical transaction + sync-dirty，下一次正常同步才可能联网。
- [ ] 重启、重复 tick、同步与刮削交错、失败退避和取消均有可复现测试。

## Out of scope

- 远程书源目录刷新、预下载、远程文件内容解析和 OS 级后台服务。
- 自动确认、自动重命名/移动文件、sidecar 写入。
- 把 AniList/Bangumi 结果强行写成 canonical，或把 Provider HTTP 塞进 Downloader/ByteSource。
