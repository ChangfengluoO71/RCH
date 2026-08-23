# RCH 漫画阅读器 — 设计规范(SPEC) v2.0

> 本文档是 RCH 的**核心指导规范**。修订须经用户确认，详见第 13 节。

- 版本：v2.0（架构重塑）
- 最近更新：2026-08-23
- 修订摘要：新增第 9 节"智能刮削与元数据架构（M8）"；M8-M1 catalog-only 离线提案层已完成 after8 版 347 条本地/夸克样本验证，保持提案模式，不自动写入 canonical metadata

---

## 1. 项目愿景

RCH 是一款 **Windows 优先、面向多端（Windows / Android）** 的现代漫画阅读器。
核心体验：即点即读、本地优先、远程资源经缓存后阅读。内置端侧 AI 高清引擎。

---

## 2. 核心设计原则

1. **阅读器永远只操作本地资源，网络只是同步层，AI 只是处理层。**
2. **统一文档抽象**：所有格式经 `Document` trait 统一为"页面集合 + 元数据"，UI 不关心底层格式。
3. **本地缓存是核心**：第一次打开时整本下载到本地缓存，之后所有操作基于缓存。
4. **WebDAV 保守策略**：默认不读取封面、不解析压缩包、不读取第一页图片。仅当用户打开漫画时才下载。
5. **统一下载调度**：所有网络请求经 Downloader（队列 / 去重 / 并发限制 / 优先级 / 重试）。
6. **多级缓存分层**：raw / cover / thumb / ai / temp + SQLite 索引。
7. **AI 推理嵌入核心**：直接在 Rust 进程中推理（目标 ONNX Runtime），消除进程启动开销。当前过渡方案为 CLI 目录批量调用。
8. **AI 全流程基于缓存**：WebDAV → 本地缓存 → AI → AI 缓存 → 显示。
9. **书源可插拔**：统一 `BookSource` 接口。
10. **格式可插拔**：统一 `Document` trait，按扩展名分发。
11. **核心与界面分离**：Rust 引擎 + Flutter UI + FRB 桥接。
12. **流畅性是可验收指标**。

---

## 3. 四层系统架构

```
┌──────────────────────────────────────────────────────────┐
│ UI 层（Flutter）                                          │
│   书架 Library │ 阅读器 Reader │ 设置 Settings             │
├──────────────────────────────────────────────────────────┤
│ 桥接层 flutter_rust_bridge v2                             │
├──────────────────────────────────────────────────────────┤
│ Document Layer（Rust）                                    │
│   document/  统一格式抽象（ZIP/EPUB/PDF/… → 页面集合）    │
│   source/    BookSource 抽象                              │
├──────────────────────────────────────────────────────────┤
│ Cache Layer（Rust）                                       │
│   cache/        多级缓存（raw/cover/thumb/ai/temp）       │
│   downloader/   统一下载调度器                             │
│   db/           SQLite 状态索引                            │
│   ai/           AI Worker 管理 + Upscaler trait            │
└──────────────────────────────────────────────────────────┘
```

**核心规则：**

```
UI 永远只读 Document
Document 永远只读本地 Cache
网络只负责把资源同步到 Cache
AI 只处理 Cache 中的数据
```

---

## 4. 技术栈

| 层 | 选型 | 说明 |
|---|---|---|
| UI | Flutter（Windows + Android） | 一套代码两端 |
| 核心引擎 | Rust（cdylib） | IO/解压/解码/书源/AI |
| 桥接 | flutter_rust_bridge v2 | 代码生成 |
| AI 引擎 | CLI Worker（realesrgan-ncnn-vulkan.exe）+ 计划迁移 ONNX Runtime | librealesrgan-ncnn-vulkan → 目标 ort crate + DirectML |
| 数据库 | SQLite（rusqlite） | 漫画索引 / 缓存状态 / 阅读进度 / 书源能力 |
| 缓存 | 五级目录 + SQLite 索引 | raw / cover / thumb / ai / temp |

**关键 Rust 依赖：**
`tokio` `reqwest` `zip` `image` `serde` `anyhow` `tracing`
`pdfium-render` `sevenz-rust` `tar` `mobi` `unrar`
`rusqlite` `sha2`

---

## 5. 缓存体系

```
<userdata>/RCH/
├── cache/
│   ├── raw/            # 整本漫画原始文件（下载后存储）
│   ├── cover/          # 封面缓存（按质量/裁剪分）
│   ├── thumb/          # 缩略图缓存
│   ├── ai/             # AI 超分结果（按模型/倍率分）
│   └── temp/           # 临时文件（CB7/CBR 解压中间产物）
├── library.json        # 书源/阅读记录/元数据（已有，兼容旧版本）
└── database.db         # SQLite（新）：漫画索引 / 缓存 Hash / 书源能力 / ETag
```

**缓存策略：**
- raw：整本存储，Hash 命名，支持增量校验
- cover：L1 内存 LRU + L2 磁盘，WebDAV 默认不生成
- AI：按原图 Hash + 模型名 + 倍率存储，不覆盖原图

---

## 6. 下载器规范

建立统一下载调度中心（`downloader/`），所有网络请求必须经过它：

- 队列管理（FIFO + 优先级插队）
- 请求去重（同一 URL + Range 合并）
- 并发限制（WebDAV 默认 ≤2 并发）
- 下载优先级：当前阅读页 > 下一页预取 > 封面生成 > 后台缓存
- 重试策略：429 退避 / 401 停止 / 超时重试 3 次
- 网络状态检测

---

## 7. WebDAV 保守策略

| 操作 | 默认行为 |
|---|---|
| 浏览目录 | 只列文件名 / 大小 / 修改时间 |
| 封面 | 不请求（显示占位图标） |
| 解析压缩包 | 仅在用户点击打开时 |
| 读取第一页 | 仅在用户点击打开时 |
| 下载 | 整本下载到 raw/，后续全走本地 |
| Range 支持 | 自动探测，不支持则整本下载回退 |

---

## 8. AI 超分架构

```
page_bytes → Rust ai/ 模块 → CLI 批量推理 / ONNX Runtime 直接推理 → ai/ → 阅读器显示
```

- Phase 1: CLI 单次调用 + ai/ 缓存（已完成）
- Phase 2: CLI 目录批量模式 — `super_resolve_batch()` 一次调用处理整本（已完成）
- Phase 3: ONNX Runtime 直接推理（模型已转 ONNX，待 ort crate 稳定后切换）
- 结果独立缓存，不覆盖原图
- 默认关闭，用户右键触发（单页） 或详情页按钮触发（整本）

---

## 9. Smart Scraping and Metadata Architecture (M8)

> The old M8.1-M8.9 plan is superseded. The approved first slice is a general, catalog-only recognizer with one automatic sync integration lane. Provider enrichment and canonical materialization remain later stages.

### 9.1 P0 invariants

- **Local-first scraping:** the scraper consumes only persisted SQLite catalog text: filename, ancestor directory names and already-indexed sibling names.
- **Scraping never reads remote book sources:** no WebDAV/SFTP/115/Quark/Baidu source adapter, ByteSource, Range read, stat/HEAD, directory refresh, or comic download is allowed during scraping.
- **Content gate:** uncached RemoteOnly assets have no local content handle; LocalFile and FullyCached assets may be used by later evidence extractors. The first slice does not inspect bytes at all.
- **Role separation:** title, chapter/volume, author and provider/platform are separate proposal fields. `creators[]` keeps circle and artist distinct; compatibility `authors[]` contains only person creator roles. Resource labels never become creators or providers. Ancestor relationships are compared up to four levels; conflicts are visible and never silently resolved.
- **Publication/resource separation:** `work_title` is distinct from filename-derived `publication_title_raw`; `resource_edition`/`censorship` describe the acquired file, while canonical publication edition is deferred. Translation labels set `translation_state`; `translation_method` is populated only by explicit machine/human evidence.
- **Sequence fidelity:** `前編`/`前篇`/`後編`/`后篇` are `sequence_kind=part` with `part` and `sequence_label`, not `special` or `chapter_title`. Chapter ordering uses a structured `ChapterOrderKey` and never a fractional chapter number.
- **Working state vs canonical state:** automatic scraping writes only local working proposals, evidence and job status. Canonical works/links are written only by a later confirmation transaction.
- **Sync boundary:** confirmation must only mark sync-dirty and never call sync inline.
- **Network boundary:** AniList/Bangumi metadata APIs may be added later as optional providers; they are not remote book-source access and must not block local proposals.
- **Single scheduler owner:** AutomationCoordinator owns startup, local-change debounce, periodic trigger and the sync-then-scrape order. SyncEngine remains the sync executor and retry/cooldown adapter.

### 9.2 Automatic flow

```
startup / local catalog change / periodic tick / manual action
  -> AutomationCoordinator deduplicates and runs one cycle
  -> existing SyncEngine performs a lightweight revision check or sync
  -> SQLite library_index supplies filename + ancestor directory names
  -> catalog-only parser emits title/author/provider/volume/chapter proposal + evidence
  -> scrape_jobs and scrape_proposals are persisted for review
  -> (later) optional Provider enrichment
  -> (later) user confirmation writes canonical works/work_links and sync-dirty
```

No sync configuration is required for the catalog-only pass. A sync failure does not erase or block local proposals.

### 9.3 First implementation milestones

| Milestone | Scope | Status |
|---|---|---|
| M8-A0 | AutomationCoordinator, one scheduler owner, sync-before-scrape integration, persistent jobs/proposals | Implemented; automation behavior still needs dedicated runtime verification |
| M8-M1 | `catalog-rules-v3`: filename + up to four ancestor levels + indexed siblings; role-separated semantic proposal and evidence | **Frozen after after8 347-row validation; proposal-only** |
| M8-M2 | Ordered DDL migration; works, external IDs and work links | Later |
| M8-M3 | Optional AniList/Bangumi provider runtime | Later |
| M8-M4 | Candidate ranking and explainability | Later |
| M8-M5 | Review, confirmation and sync-dirty transaction | Later |
| M8-M6 | 100-book corpus validation | Required after manual rule validation |

### 9.4 Acceptance gates

- A RemoteOnly catalog entry with no raw cache produces a proposal with zero remote book-source requests and zero ByteSource reads.
- A local or fully cached entry is still handled by catalog-only logic without source I/O.
- Repeated ticks and restart recover from persisted jobs; sync failure does not block the scrape lane.
- Provider failure, when added, leaves local proposals and the reader usable.
- Manual review must verify title/chapter separation, author/provider separation, ancestor depth and missing-author behavior before provider or canonical work begins.

## 10. 格式支持矩阵

| 格式 | 引擎 | 状态 |
|---|---|---|
| ZIP / CBZ | `zip` + `flate2` | 已完成 |
| EPUB | `zip` + OPF spine 自研 | 已完成 |
| Folder | 目录枚举 | 已完成 |
| CB7 | `sevenz-rust` | 已完成 |
| CBT | `tar` | 已完成 |
| PDF | `pdfium-render` | 已完成（需 pdfium.dll） |
| CBR / RAR | `unrar` | 已完成（需 unrar.dll） |
| MOBI / AZW / AZW3 | `mobi` crate | 已完成 |
| AVIF | `libavif` | 未来 |

---

## 11. 里程碑

| 里程碑 | 内容 | 状态 |
|---|---|---|
| **M1** 核心阅读闭环 | 书源 + ZIP/CBZ 流式 + 三模式 + 书架 | 基本完成 |
| **M2** AI 高清引擎 | Worker 架构 + Upscaler trait | **Phase 1+2 已完成，Phase 3 待 ONNX Runtime** |
| **M3** 格式扩展 | PDF/EPUB/CB7/CBT/Folder | **已完成** |
| **M4** 复杂场景 | 智能拼页/旋转/裁边 | 待规划 |
| **M5** 格式+书源扩展 | MOBI/CBR/SMB/SFTP/网盘 | 格式已完成 |
| **M6** Android 适配 | 手机/平板 | 待规划 |
| **M7** 标签系统 | 标签筛选书架 | 数据模型已有 |
| **M8** 智能拓展 | 智能刮削 + AI 元数据融合 | **已规划**（任务 08-08-m8-smart-scraping，见 §9） |
| **M9** 缓存基础设施 | 下载器 + SQLite + 五级缓存 | **新增，待实施** |

---

## 12. 非目标

- 不做在线漫画站爬虫/聚合
- 不向用户原始收藏目录写入 sidecar 元数据文件（metadata.json 仅作可选导出）
- 不做账号体系与第三方服务器云同步（多端同步由用户自有 WebDAV / 网盘同步盘完成，本地优先，敏感凭据不入同步包）

---

## 13. 变更约束

1. SPEC 为最高设计指导，修订须经用户确认。
2. 每轮施工更新 LOG + LOG-INDEX。
3. 用户确认后更新 README 反映现状。
