# M8 智能刮削：Catalog-Only 通用识别闭环

## Goal

先交付一个不依赖漫画内容、不依赖在线 Provider、对本地与未缓存远程漫画都可用的通用识别器：只读取 SQLite 中已经存在的文件名和上级目录名，识别漫画名、作者、提供者/平台、卷章关系，输出可解释的标题/作者结果。作者或标题缺失是合法结果，不强行猜测。

这个初级版本本身就可以作为可用产品能力；Provider、canonical 作品库和跨端同步是后续增强，不得成为基础识别器的前置条件。

## R0 自动化流程与同步集成

刮削不是一次性的按钮功能，而是接入现有自动同步生命周期的本地任务。系统复用 `SyncEngine` 的启动、防抖、轮询、退避和生命周期语义，由协调器统一编排三条隔离通道：

```text
AutomationCoordinator
 ├─ sync_transport       → 现有 SyncEngine / WebDAV 同步状态
 ├─ catalog_scrape       → SQLite catalog snapshot / 本地 proposal
 └─ provider_enrichment  → AniList/Bangumi + provider_cache（可选）
```

### R0.1 自动触发

- **启动**：数据库、目录树和同步配置加载完成后，先按既有设置安排同步；同步周期完成后读取本地 catalog revision delta，自动生成刮削任务。
- **目录变化**：已持久化的 catalog/index revision 发生变化后 2 秒防抖并合并任务；刮削不主动刷新远程书源。
- **同步完成**：pull/apply 带来的新目录或变化记录进入同一刮削队列；同一 `book_key + catalog_revision + rule_version` 只处理一次。
- **周期 tick**：沿用现有前台 60 秒 tick。同步通道按原有 remote revision/退避策略运行；刮削通道只扫描本地 SQLite 待处理项，不因 tick 产生远程书源请求。
- **手动入口**：提供“立即刮削”（local-only）和“运行完整周期”（先同步、后刮削）；两者都不能变成远程书源扫描。

### R0.2 执行顺序与确认门控

1. `sync_transport` 完成当前同步周期（没有同步配置时跳过）。
2. 协调器读取本地 catalog 变化，入队 `catalog_scrape`，只产生/更新 working proposal。
3. Provider enrichment 作为可选低优先级任务，失败不影响本地 proposal、阅读或同步。
4. 用户在 Review UI 中确认后，`confirm_proposal` 只提交本地 canonical transaction 并标记 sync-dirty；不在事务内调用 WebDAV。
5. 既有同步自动流程在后续正常周期或本地变更防抖后发送 confirmed canonical 数据。

自动刮削不自动确认、不静默覆写 canonical metadata；working proposals、候选、证据和 Provider cache 不进入同步快照。

### R0.3 任务状态与失败隔离

- 任务状态统一为 `queued/running/succeeded/degraded/retry_wait/failed/cancelled`，支持去重、取消、重启恢复和进度观察。
- 同步保留现有 429/503 冷却与 30 秒至 15 分钟退避；同步失败不能阻塞本地刮削。
- 本地解析的缺失字段、角色冲突和上下文不足进入可解释的 `degraded` proposal，不重试成远程目录请求。
- Provider 离线、超时、限流和无结果只影响 enrichment 状态；Provider failure 不得回退到远程漫画文件。
- 当前自动化仍以应用前台生命周期为有效运行范围；退出/挂起时保存队列，恢复后补跑到期任务，不承诺 OS 级后台服务。

## Product Flow

```text
应用启动/同步完成/目录 revision 变化
        ↓
AutomationCoordinator 去重、排队和防抖
        ↓
已有 catalog snapshot（文件名 + 上级目录名）
        ↓
规则化 token / 章节与平台噪声清理
        ↓
比较文件名与最多 N 级祖先目录的关系
        ↓
输出漫画名 / 可选作者 / 可选提供者 / 卷章与证据
        ↓
本地预览或保存为 scrape working proposal
        ↓（后续阶段）用户确认 → canonical work / work_link → sync-dirty（下个同步周期发送）
```

## Requirements

### R1 / M8-M1 — Catalog-Only Name & Role Extraction

- 输入只允许本机 SQLite 中已有的 catalog snapshot：文件名、路径、上级目录名、已有 size/mtime/etag 和用户 metadata；不读取文件字节。
- 默认比较当前文件名及向上 3 级祖先目录，层数可配置；不得为了补充路径信息刷新远程目录。
- 识别并区分 `title`、可选 `author`、可选 `provider/platform`、`volume/chapter`；作者名和提供者名不能互相吞并，漫画名和章节名不能互相吞并。
- 输出每个字段的来源文本、祖先层级、规则命中、置信度和冲突/缺失原因；允许 author/title/provider 为空。
- 生成一个可以独立运行的 catalog parse job/proposal 结果，不调用 ByteSource、Downloader、Provider 或 sync transport。

### R2 / M8-M2 — Canonical Identity & Migration

- 在初级识别器验证后建立独立 ordered DDL migration、`works`、`work_external_ids`、`work_links`、provenance 和 sync-dirty。
- 不向 `book_metas` 添加 `work_id`；初级 proposal 不得直接写 canonical work。

### R3 / M8-M3 — Optional Provider Enrichment

- AniList/Bangumi 只接收 R1 产生的文字 query，作为可选补充和候选来源。
- Provider 失败不得降低 R1 本地识别能力，也不得回源远程书源。

### R4 / M8-M4 — Candidate & Explainable Ranking

- 将本地规则 proposal 与可用 Provider 候选统一为可解释候选；本地结果永远可单独呈现。
- 排名必须保留 title/author/provider/chapter 的逐项依据和规则版本。

### R5 / M8-M5 — Review & Confirmation

- 用户可以审阅、修正或拒绝初级识别结果；`confirm_proposal` 才能物化 canonical work/link。
- 确认只完成本地 SQLite transaction 与 sync-dirty 标记，不 inline 调用同步传输。

### R6 / M8-M6 — Corpus Validation

- 使用 100 本真实漫画验证初级规则的标题覆盖率、作者覆盖率、作者/提供者混淆、标题/章节混淆、缺失和拒绝原因。
- Provider coverage 作为附加指标，不作为基础识别器可用性的前置条件。

## Acceptance Criteria

- [ ] 启动、catalog revision 变化、同步完成、周期 tick 和手动入口都会产生正确的自动化任务，并按 `book_key + revision + rule_version` 去重。
- [ ] 同步完成后新增/变化 catalog 自动生成 proposal；没有同步配置时 catalog-only 刮削仍可独立运行；同步失败不阻塞本地 proposal。
- [ ] 自动任务在重启、重复 tick、同步与刮削交错和取消后可恢复，且不会形成 catalog→scrape→sync→scrape 的无限循环。
- [ ] `confirm_proposal` 只完成本地 canonical transaction + sync-dirty；transport spy 证明确认期间没有调用 `sync_now`、sync actor 或 WebDAV。
- [ ] 对 LocalFile、FullyCached 和 RemoteOnly catalog snapshot 均可产生 proposal；RemoteOnly 不产生任何远程书源请求。
- [ ] 文件名无作者、无标题或目录层级不足时，返回可解释的缺失状态，不伪造字段。
- [ ] 带 `[作者]`、`作者 - 标题`、`平台/作者/标题/卷章`、多级系列目录等 fixtures 能输出角色证据和祖先关系。
- [ ] 章节/卷标记被剥离后不会成为漫画名；平台/提供者 token 不会被写入 author，显式 author token 不会被写入 provider。
- [ ] 同一 catalog snapshot 与规则版本重复运行结果稳定、无远程 I/O、无 canonical 写入。
- [ ] 100 本 corpus 产生可复现的 title/author coverage、角色混淆矩阵、缺失/拒绝原因和 Provider coverage 报告。
- [ ] Rust 测试、Windows 端到端预览/保存流程通过；Android 不阻塞初级识别器。

## Out of Scope / Follow-ons

- 初级版本不读 ComicInfo/OPF/MOBI、不读页数/封面、不做 pHash/OCR/CLIP、不计算内容 hash。
- 初级版本不要求 AniList/Bangumi 返回结果，不爬在线漫画站。
- `M8.1` Provider Expansion、`M8.2` Advanced Evidence、`M8.3` Metadata Taxonomy、`M8.4` Discovery、`M8.5` Export & Interop 均后置。
- 不自动重命名、移动文件或写 sidecar。

## Frozen Decisions

- **Automation Coordinator First**：刮削接入现有 SyncEngine 的启动/防抖/轮询/退避生命周期，但同步传输、catalog-only 刮削和 Provider HTTP 保持三条隔离任务通道；不把 `SyncEngine` 改成万能网络客户端。
- **Sync then Scrape**：每个自动周期默认先完成现有同步，再根据本地 catalog delta 生成刮削 proposal；无同步配置时直接运行 local-only 刮削。
- **Proposal, not Auto-Confirm**：自动流程只生成 working proposal，canonical 写入仍必须经过用户确认；确认后的 sync-dirty 由下一次既有同步周期处理。
- **Catalog-Only First**：首个可交付版本只依赖 filename + ancestor directory names，未缓存远程漫画同样可识别。
- 规则引擎优先于网络刮削；Provider 是 enrichment，不是基础识别器的依赖。
- 默认比较 3 级祖先目录，并记录层级关系；该值可配置但必须进入 rule version。
- title、author、provider、chapter 是互斥角色标签；冲突时报告冲突或留空，不静默混淆。
- RemoteOnly 不得传入通用 ByteSource；确认不触发 sync transport。
