# M8 智能刮削：Catalog-Only 通用识别闭环 — 实施计划

> 当前重新回到规划阶段。用户确认本版计划后，先启动 M8-M1；不提前实现 migration、Provider 或 UI。

## Dependency Chain

```text
M8-A0 Automation Coordinator & Sync Integration
   └─ M8-M1 Catalog-Only Name & Role Extraction
       └─ M8-M2 Canonical Identity & Migration
           └─ M8-M3 Optional Provider Enrichment
               └─ M8-M4 Candidate & Explainable Ranking
                   └─ M8-M5 Review, Confirmation & Sync-Dirtiness
                       └─ M8-M6 Corpus Validation
```

M8-A0 是横向前置能力：它不改变 SyncEngine 的 WebDAV 实现，只把现有启动/防抖/轮询/退避生命周期与 catalog-only 刮削统一编排。M8-M1 本身仍是可独立交付的通用版本；M8-M3 Provider 无结果不阻塞后续本地识别。

## M8-A0 — Automation Coordinator & Sync Integration

- [ ] 定义 `sync_transport`、`catalog_scrape`、`provider_enrichment` 三类任务、统一状态机、trigger、dedupe key 和 input revision。
- [ ] 复用现有 SyncEngine 的启动同步、2 秒防抖、60 秒 tick、429/503 冷却和指数退避；不得复制或放宽其远程 I/O 权限。
- [ ] 实现启动、catalog revision、sync completed、periodic tick、manual 五类触发，默认顺序为 sync → local catalog scrape → optional Provider。
- [ ] 为 `scrape_jobs` 增加最小调度字段，支持 revision supersession、重启恢复、取消和失败隔离；新旧 queued job 不重复执行。
- [ ] 把 catalog-only parser 接入本地任务通道，确保任务参数不包含 `ByteSource`、Downloader、远程 source session 或 sync handle。
- [ ] 设计并测试 `confirm_proposal → local transaction + sync-dirty → later sync`，确认期间 transport spy 必须为 0。
- [ ] 加入自动化状态面板所需的 pending/last-result/degraded 数据，但不在 A0 阶段承诺完整 Review UI。

## M8-M1 — Catalog-Only Name & Role Extraction

- [ ] 定义 `CatalogSnapshot`、`CatalogParseRequest`、`NameRoleProposal`、`RoleEvidence`、`RoleConflict` 和 `ParseState`。
- [ ] 实现文件名与默认 3 级祖先目录 tokenization，保留原文 span 和层级来源。
- [ ] 先识别卷/章/版本结构，再识别 provider/platform，再识别 author，最后识别 title；建立互斥角色约束。
- [ ] 实现 filename 与多级 ancestor 的重复/稳定关系评分，处理系列目录与分类目录。
- [ ] 持久化本地 scrape working proposal 或提供稳定 API；不写 canonical、不访问 ByteSource/Downloader/Provider/同步传输。
- [ ] 用真实命名 fixtures 验证标题/作者/提供者/章节分离、缺失值和冲突解释。

## M8-M2 — Canonical Identity & Migration

- [ ] 在 M8-M1 规则契约稳定后建立独立 ordered DDL migration ledger。
- [ ] 实现 works / external IDs / work links / provenance / sync-dirty；不添加 `book_metas.work_id`。
- [ ] 验证新旧库迁移、幂等、故障回滚、稳定 key、tombstone 与唯一关系。

## M8-M3 — Optional Provider Enrichment

- [ ] Provider 只接收 M8-M1 产生的本地 title/author query；实现 AniList/Bangumi async runtime、缓存和 typed failure。
- [ ] 验证 Provider 离线/限流/坏响应不会破坏本地 proposal，也不会触发远程书源回退。

## M8-M4 — Candidate & Explainable Ranking

- [ ] 合并本地规则 proposal 与可用 Provider 候选，保存逐项评分、规则和 provenance。
- [ ] 验证本地结果在 Provider 零结果时仍可展示；确认前 canonical 零写入。

## M8-M5 — Review, Confirmation & Sync-Dirtiness

- [ ] 提供候选审阅、手工修正、确认/拒绝和唯一 `confirm_proposal` 事务。
- [ ] 确认只写本地 canonical 与 sync-dirty；transport spy 证明没有 inline sync。

## M8-M6 — Corpus Validation

- [ ] 固化 100 本真实漫画的 catalog snapshot 与人工真值。
- [ ] 输出 title/author coverage、作者/提供者混淆矩阵、标题/章节混淆矩阵、缺失/拒绝原因和 Provider coverage。
- [ ] 产品复核报告后决定 M8 是否完成；低覆盖率优先改规则，不以增加 Provider 替代基础能力。

## Quality Gates

```powershell
cd app\rust
cargo test db::
cargo test
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
```

M8-M1 无 Dart API 时不跑 FRB codegen；增加 FRB 后再按跨层检查补充 `flutter analyze` 与生成文件校验。

M8-A0 额外要求：调度器单测覆盖启动顺序、2 秒合并、revision supersession、重启恢复、取消、失败隔离和 no-loop；集成测试覆盖同步完成触发刮削、无同步配置仍可刮削、确认不 inline sync，以及 RemoteOnly 的零书源 I/O。

## Stop Conditions

- M8-M1 需要读取文件内容、ByteSource、远程目录或 Provider 才能得到标题/作者时，停止并回到边界设计。
- 规则把 provider 当 author、chapter 当 title，或缺失字段被静默伪造时，不进入 M8-M2。
- 远程书源请求或确认内联同步出现时，阻断后续阶段。
- 若为了自动周期给 scraper 注入通用 `ByteSource`、触发远程目录刷新，或让 Provider/同步失败阻塞本地 proposal，立即停止并回到边界设计。
