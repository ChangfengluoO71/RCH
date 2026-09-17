# 云端封面扫描与流式阅读提速 — 完整解决方案（待审阅）

> **日期**：2026-09-17（接手轮）。**范围**：代码勘察、外部资料调研、方案设计。**本轮未修改任何生产代码**，所有结论均可回溯到第 8 节的「文件:行」证据。
> **承接**：`docs/superpowers/specs/2026-09-14-remote-cloud-scan-design.md`、`docs/superpowers/plans/2026-09-17-unified-remote-cover-pipeline.md`。
> **修正**：`.trellis/tasks/09-14-remote-scan-ui-verify/research/2026-09-17-cover-pipeline-review.md` 中有三条结论已被后续提交修掉或定位有偏，第 1 节先纠正，避免重复劳动。
> **状态**：本文件为方案，尚未实施；复选框是审阅通过后的执行检查点。

---

## 0. 结论摘要

两个问题各有**一个主因**和若干放大因素，且**同源**：所有云端 I/O 共用一个"无优先级、持锁等待、粒度错"的调度层。

**问题一（封面）主因**：`remote_cover_job` 的 upsert **无条件保留旧状态**（`cover_store.rs:581`），导致任何一次落入 `failed` / `unsupported` / `blocked` 的封面任务**永远不会被后续扫描重试**；而对账逻辑把"文件丢失"改回 `pending` 后**没有任何人唤醒 worker**。于是"大量漫画没反应"是**持久化的终态**，不是偶发抖动。次要放大：单来源单线程串行 worker × 每本 ≈9 次被门控的请求 ≈ 2.7s/本（273 本 ≈ 12 分钟），加上卡片层 10.5s 轮询 + 8 次重复申请、排队也转圈、无进度条。

**问题二（读速）主因**：`WEB_RANGE_REQUESTS_PER_SEC = 4.0`（`cloud115.rs:1116`）把**针对 `webapi.115.com` 的 4 QPS 建议**（`问题记录_2026-08-08.md:121`）套用到了 **CDN 直链 Range** 上。单页按 256 KiB 预读（`source/mod.rs:100`）拆成 `ceil(页大小/256KiB)` 次 Range，1.5 MB 页 = 6 次 × 250 ms 固定间隔 = **门控下限 1.5 s/页**；3 MB 页 = 3 s。而 `RateGate::wait` **持锁 sleep**（`source/mod.rs:43-49`），前台优先级传不到底层；`downurl_lock` 又是**跨 pickcode 的全局锁**且覆盖限速等待 + 网络请求（`cloud115.rs:1366`），后台给 A 取链时读 B 最多等约 1.2 s。夸克更糟：`QuarkClient` **完全没有直链缓存**，adapter 路径**每次 `read_range` 都重新 `file/download`**（`api/source.rs:504`）。

**方案取向（已按你的选择固化）**：只出方案文档、不改代码；限速采取**通道解耦 + 阅读优先**——API 端点保持保守门控，CDN Range 独立门控并支持阅读插队与相邻 Range 合并。

---

## 1. 先纠正三条既往结论（本轮实际读码复核）

| 既往结论 | 本轮复核结果 | 证据 |
|---|---|---|
| "压缩包打开时逐页探测" | **并未修复**（本节原判为"已修"，P0-A 实测推翻）。`ZipBook::open` 确实改用了 `zip.name_for_index(i)`，注释也宣称为此改的；但项目自带测试 `opening_many_pages_does_not_fetch_every_local_header`（`document/zip.rs:190`）实测：**打开 40 页要 81 次 Range（≈2 次/页，线性于页数）**，断言 `<= 8` 直接失败。已在 P0-A 用 A/B 证明与新增埋点无关（摘掉埋点后同样 81）。 | `document/zip.rs:42-66`、`document/zip.rs:190`、`docs/reports/p0/2026-09-17-p0a-baseline.md` |
| — | **但 CB7/CBT/CBR 是"整包读入内存再解压"**：远程漫画打开 = **整本下载**，且发生在封面路径的 `open_document` 内。这才是压缩包侧真正的缺口。 | `document/sevenz.rs:18-28`、`document/tar.rs:18-24`、`document/rar.rs:20-31` |
| "磁盘页缓存也要排网络队列" | **阅读页已修**：`load_claimed` 先 `disk_get`，命中直接返回，之后才取 governor 许可；并有专门测试 `disk_hit_does_not_wait_for_network_permits`。 | `reader.rs:317-325`、`reader.rs:496` |
| — | **但封面磁盘缓存被"网络总开关"挡住**：`_maybeLoad` 在查磁盘之前就 `return`，所以关掉联网开关时，**磁盘上已有的封面也显示不出来**（显示"未缓存"）。这才是残留缺口，且它在 Dart 层而非 Rust 层。 | `comic_cover.dart:456`、`comic_cover.dart:700-731` |
| "115 取链缓存未命中时共用一把锁" | 成立。`downurl_lock: Mutex<()>` 是**全客户端一把锁**，覆盖 `gate.wait()` + HTTP POST，且只在同 key 时才是有效合并。 | `cloud115.rs:1366-1371`、`cloud115.rs:1373-1405` |
| "夸克重复取链" | 成立，且比 115 更严重：`QuarkClient` **既无直链缓存也无速率门**；115 至少有 `downlinks` + 5 分钟 TTL。 | `api/source.rs:503-506`、`api/source.rs:572-575`、`quark.rs:309-338`、`cloud115.rs:1104`、`cloud115.rs:1110` |

---

## 2. 问题一：封面扫描

### 2.1 现象 → 根因链

#### 2.1.1 主因：「任务状态不可恢复」——这是"大量漫画没反应"的持久化原因

```sql
-- cover_store.rs:576-582
ON CONFLICT(job_key) DO UPDATE SET
    demand_kind = ...,
    priority    = MAX(...),
    generation  = MAX(...),
    session_epoch = ...,
    state = remote_cover_job.state,   -- ← 无条件保留旧状态
    updated_at = excluded.updated_at
```

- 后果 A：任何封面任务一旦进入 `failed` / `unsupported` / `blocked` / `cancelled`，**后续任何一次全量重扫都不会把它拉回 `pending`**。用户反复重扫，计数依旧停在"失败 N / 不支持 N"。
- 后果 B：`ready` 但磁盘文件被清掉的任务同样保持 `ready`——只有恰好有人读它时才可能被对账发现（见 2.1.2）。
- 后果 C：`RemoteScanError::RangeUnavailable | Unsupported → CoverJobState::Unsupported`（`api/remote_scan.rs:162-166`）是**终态**。而 405/风控/网络错误在旧入口里曾被吞成 `false`（详见 2.1.5），于是"暂时性故障"会被永久记成"不支持"。
- 后果 D：唯一的恢复入口是 UI 的「重试」按钮 → `remote_cover_retry`，而它的 SQL 只捞 `state IN ('retry_wait','failed')`（`api/remote_cover.rs:511-521`），**不捞 `unsupported` / `blocked`**。
- 后果 E：`rebind_completed_generation_session`（重启后把已完成代际交给新会话）同样只更新 `state IN ('pending','running','retry_wait')`（`persistence.rs:538-545`）。

**首次扫描遇到 115 WAF 405 时**（`cloud115.rs:1409-1412` 会整会话冷却 60 s，`mark_web_waf_blocked`），一批任务会直接落 `failed/blocked`；若撞上 Range 判定的旧入口，还会落 `unsupported`。之后无论重扫多少次都补不回来。**这与"仍然存在大量漫画没反应"的现象完全一致。**

#### 2.1.2 第二因：对账会置 `pending`，但没有人唤醒 worker，也没有独立补齐

- `read_cached_cover`（`cover_service.rs:60-116`）在读到 `ready` 但磁盘文件缺失/损坏时，会把 `remote_cover_variant` 与 `remote_cover_job` 都改回 `pending`（第 104-112 行）。**这一步已经实现了你要的"数据库 ready 不能代替文件检查"**。
- 但唤醒 worker 的地方只有两处：`consume_staged_covers`（扫描期，`api/remote_scan.rs:1673`）与 `remote_cover_request`（卡片点开，`api/remote_cover.rs:444`、`544`）。
- `run_remote_cover_worker` 在**队列暂时为空时直接 `return`**（`api/remote_scan.rs:344`）。因此对账产生的 `pending` 会**永久躺在数据库里**，直到下一次扫描或某张卡片碰巧请求它。
- **当前不存在"独立于目录变化的缺封面补齐"**：缺封面只能被"目录重扫"或"UI 点击"触发。

#### 2.1.3 第三因：封面入队早，但消费是"单来源单线程串行 + 多重门控"

- 好消息：`remote_cover_job` 在**目录发现期**就已入队并唤醒 worker（`api/remote_scan.rs:690-701`），不是等整树发布。既往结论里的"要等全部遍历完成"只对 `consume_staged_covers` 这条**兼容通道**成立（`api/remote_scan.rs:1518`）。
- 坏消息：每个来源**只有一个 worker 线程**——`cover_workers()` 的 key 是 `{source_id}:{session}`（`api/remote_scan.rs:141-149`），而 `run_remote_cover_worker` 是**串行 while 循环**（`api/remote_scan.rs:295-507`）。
- 每个任务要依次支付：
  1. 账号预算 100 ms（`provider_budget.rs:81`）
  2. 115 API 门 1.5/s → 最多 667 ms（`cloud115.rs:1384`）
  3. `downurl` 取链（网络）
  4. Range 探针 1 次（4/s 门，`cloud115.rs:1509`）
  5. ZIP 中心目录读取（若干次 Range）
  6. 首页数据读取（若干次 Range，`read_ahead=256 KiB`）
- 粗算（115，单页 1.5 MB）：`1 + 1 + 1(CD) + 6(页) ≈ 9` 次请求；光 Range 门 = 8 × 250 ms = **2.0 s**，加取链门 0.67 s ≈ **2.7 s/本**。273 本串行 ≈ **12 分钟**，这是"看起来没反应"的体感来源。

#### 2.1.4 第四因：卡片层「排队也转圈 + 每卡片自己轮询 + 重复申请」

| 位置 | 行为 | 后果 |
|---|---|---|
| `comic_cover.dart:456` | `if (_remoteCoverNetworkPaused) return;` 在查盘之前 | 关闭联网时，**磁盘上已有的封面也不显示** |
| `comic_cover.dart:702-729` | `_future` 一旦建立就渲染 `CircularProgressIndicator` | **排队中也转圈**，与 `running` 无法区分 |
| `comic_cover.dart:663-680` | 请求后自行轮询 `30 × 350 ms` = 10.5 s | N 张可见卡 = N 条独立轮询 |
| `comic_cover.dart:489-503` | 失败后 `_scheduleUnifiedRetry` 8 × 900 ms 重复走 `_maybeLoad` → 再次 `requestCover` | **每卡片重复申请同一持久任务** |
| `remote_cover_repository.dart:146-167` | 已有 `remoteCoverStateLabel` 中文映射 | 卡片路径根本没用它，失败原因没落到 UI |
| `comic_cover.dart:284` | consumerId 含 `identityHashCode(this)` | 文件夹卡与代表漫画卡是**两个 consumer**，需求无法共享 |

#### 2.1.5 第五因：能力判定仍存在"终态污染"的旧入口

- `probe_checked` 已经能给出 typed error（`cloud115.rs:1504-1538`），但**兼容包装 `probe()` 仍然把任何错误压成 `(false, 0)`**（`cloud115.rs:1543-1548`；夸克同款 `quark.rs:352-357`）。
- 而**实际打开流程仍在用旧入口**：`open_cloud115_cookie_book` 的 `client.probe(&info.url)`（`api/source.rs:1277`）与 `open_quark_book` 的 `client.probe(&info.url)`（`api/source.rs:1519`）。
- 也就是说：**登录态失效、网络中断、405 拦截都会在打开路径上被显示成"直链不支持 Range，请改用整本下载策略"**。这既是错误文案问题，也是 `unsupported` 终态被误写的来源。

#### 2.1.6 第六因：进度口径与视觉

- **后端字段已齐备**：`RemoteScanStatusDto` 已含 `listing_phase / directories_checked / discovered_books / discovery_complete / ready_books / active_books / pending_books / retry_books / blocked_books / unsupported_books / failed_books / view_revision`（`api/remote_scan.rs:32-57`），且 `total=discovered_books`、`processed=ready_books` 的语义已被显式改写（`api/remote_scan.rs:1206-1211`）。
- **UI 已经拆成两行**（`remote_scan_status.dart:140-149`），`失败` 也没有被并进 `可用`（`refresh_status_counts` 把 `ready/running/pending/retry_wait/blocked/unsupported/failed` 分成 7 个独立计数桶，`api/remote_scan.rs:1119-1163`）。
- **仍缺的**：① 没有任何进度条；② 文案是"可用 X，处理中…"而非你要的 `可用 210 / 共 273 本`；③ 发现期的分母一直在变，UI 没有区分"发现中分母不稳定"与"发现完成后分母固定"。

### 2.2 你的 5 条收敛建议 → 代码落点

| 你的要求 | 当前状态 | 需要做什么 |
|---|---|---|
| **1. 以有效缓存为准**（查文件而非信任 DB `ready`） | `read_cached_cover` 已实现文件校验 + 置回 `pending`（`cover_service.rs:99-114`），但**没人唤醒**、且不覆盖"从未入队"的漫画 | 把校验抽成 `reconcile_ready_covers()`，校验项 = 文件存在 + 长度 + 可解码宽高；置回 `pending` 时清 `error_code`、`attempt=0`、`next_attempt_at=now`，**并在事务提交后唤醒 worker** |
| **2. 缺封面补齐独立于目录变化** | **不存在** | 新增 cover backfill sweep：只读 `library_index`（`deleted=0` 且 `ArchiveFile`/`ImageFolder`），不列任何目录；4 个触发点（见 2.3-B） |
| **3. 文件夹直接引用代表漫画** | 已基本成立：`catalog.rs:313-402` 的 `cover_state_for(representative_asset_id)`；`source_browser.dart:1568-1570` 容器取 `representativeAssetId`。文件夹不另起下载任务 ✅ | 补两点：① 无 unified view 时不要回落到快照判定为 `uncached`（`source_browser.dart:566-580`）；② consumer 按 assetId 去重 |
| **4. 卡片订阅任务状态** | 未实现（每卡片自轮询 + 重复申请） | 删掉 30×350 ms 与 8×900 ms；卡片只做"读盘 + 声明需求"；状态改由 `RemoteScanCoordinator` 按来源批量拉 |
| **5. 进度拆两行 + 进度条** | 两行已实现，**无进度条**，格式不符 | 加 `LinearProgressIndicator`；封面行改 `可用 210 / 共 273 本`；发现期分母标"仍在发现" |

### 2.3 方案设计

#### A. 以有效缓存为准（要求 1）

```
reconcile_ready_covers(source_id, limit) -> ReconcileReport
  对每个 (asset_id, content_revision, selection_revision, profile) 中 state='ready' 的记录：
    1. 读 blob 文件：不存在 / 长度为 0 / 长度与 remote_cover_blob.byte_size 不符 → 失效
    2. 解码头部（不解全图）：宽高与 remote_cover_variant 不符 → 失效
    3. 失效 → UPDATE state='pending', error_code=NULL, attempt=0,
                     next_attempt_at=0, lease_owner=NULL, updated_at=now
  事务提交后：wake_remote_cover_worker(source_id, session)   ← 关键补齐
```

- 约束：文件 I/O **在数据库锁之外**（沿用 `cover_service.rs:99-101` 的既有约定）；一次只对账一批（建议 200），避免长事务。
- 幂等：对账不改变 `content_revision` / `selection_revision` / `profile`，只改状态机。

#### B. 缺封面补齐独立于目录变化（要求 2）

```
cover_backfill_sweep(source_id) -> 入队数量
  从 library_index 只读查询（不列目录、不建会话）：
    SELECT id, content_fingerprint FROM library_index
     WHERE source_id=? AND deleted=0
       AND ((entry_type='file' AND asset_kind='ArchiveFile')
            OR (entry_type='dir'  AND asset_kind='ImageFolder'))
  对每条：
    无对应 ready variant，或 reconcile 判定失效，或 job.state ∈ {failed} 且
      updated_at 早于 backfill_retry_after（建议 6h）→ upsert pending（priority=background）
```

触发点（全部**只入队**，实际 I/O 由 worker 在同一套门控下执行）：

1. 应用启动后延迟 30 s 一次（避免与冷启动关键路径争抢）；
2. 每次扫描（`publish_staged_generation` 成功）之后一次；
3. 每 10 分钟一次定时补齐（仅在存在有效会话时）；
4. 用户显式入口「补齐缺失封面」（UI 按钮 → `remote_cover_retry(source_id, [])` 语义扩展）。

**必须遵守的边界**（你的补充要求，"缺缓存就排队"不得再次造成请求风暴）：

- 联网总开关：`current_cover_fetch_enabled()` 在**每次 claim 边界**复查（已实现，`api/remote_scan.rs:300-302`），补齐新增的触发点也必须先查；
- 登录状态：`remote_provider_adapter` 失败 → `blocked/authExpired`，**不重试**（已实现，`api/remote_scan.rs:385-397`）；
- 限流：账号预算 `provider_budget` + provider 门控照旧；补齐任务一律 `priority=background`，**不得**提升为 `visible`；
- Range 降级：只有**有协议证据**的不支持才落 `unsupported`（见 E）；
- 单来源串行 worker 不得因为补齐而变成"每 10 分钟又打一次全库"——补齐只对"确实缺封面"的条目入队，命中率应为 0 请求。

#### C. 文件夹直接引用代表漫画（要求 3）

现状已满足"文件夹跟随代表漫画、不另起任务"。补齐两点：

1. **判定来源**：`_detectRemoteFolderKind`（`source_browser.dart:566-580`）当前在无 unified view 时回落到 `FolderSnapshotStore` / 阅读记录，无则判 `uncached` → 卡片**根本不提交需求**。
   改为：只要目录在 `library_index` 中已是 `ImageFolder`、或已推导出 `representative_asset_id`，就直接按代表漫画走；快照只用于"尚无索引"的过渡期。
2. **consumer 去重**：`_remoteConsumerId` 去掉 `identityHashCode(this)`，改为 `cover:{sourceId}:{assetId}`。
   - 文件夹卡与代表漫画卡共享同一个需求 → "文件夹转圈 = 代表漫画转圈、等待/失败/就绪同步"天然成立（**这正是你要的第 3 条**）。
   - Dart 侧维护 `assetId → 卡片引用计数`；计数归零才 `release`。这样文件夹卡先 dispose 不会撤掉代表漫画的需求，也不会误解锁。（Rust 侧 `ConsumerRegistry` 已按 consumer→keys 存储，`cover_service.rs:16-51`，无需改表。）

#### D. 卡片订阅任务状态（要求 4）

**删除**：
- `_loadUnifiedRemoteCover` 的 `30 × 350 ms` 轮询（`comic_cover.dart:663-680`）；
- `_scheduleUnifiedRetry` 的 `8 × 900 ms` 重复申请（`comic_cover.dart:489-503`）。

**替换为"按来源批量刷新"**：

```
Rust 新增（只读、不发网络）：
  remote_cover_states(source_id, asset_ids) -> Vec<RemoteCoverStateDto>

Dart：RemoteScanCoordinator 已有每来源 500 ms 的 progressPollInterval
     （remote_scan_coordinator.dart:68, 404-441）
  1. 一次轮询里读取 status（含 view_revision）
  2. view_revision 未变 → 直接返回，不 setState
  3. revision 变化 → 只对"当前可见"的 assetId 批量 remoteCoverStates()
  4. 一次 setState 刷新全部可见卡片
```

卡片只做两件事（都在本地队列里）：
1. `remoteCoverRead`（**纯磁盘，永不发网络**）；
2. 没有 → `remoteCoverRequest`（**只提交需求，不等网络**）+ attach consumer。

**视觉规则**：

| 状态 | 卡片显示 |
|---|---|
| `running` | **转圈**（唯一转圈状态） |
| `pending` | "等待获取"（不转圈） |
| `retry_wait` | "稍后重试" |
| `blocked` | "等待授权" |
| `unsupported` | "暂不支持局部读取" |
| `failed` | 中文失败原因（经 `remoteCoverStateLabel` / `_messageFor` 映射） |
| `ready` | 图片；有旧图时**先显示旧图**并叠加刷新提示（`is_previous_revision`） |

排队中的卡片**不允许**到期自动重复申请。手动重试走 `remoteCoverRetry`（一次来源级调用，不是每卡片一次）。

#### E. 进度拆两行 + 进度条（要求 5）

```
第 1 行（目录发现）：
   发现中   → 目录：已检查 38 个 / 已发现 273 本        [不定长 LinearProgressIndicator]
   完成后   → 目录：已检查 142 个 / 共 273 本           [确定值 = checked/directories_total]
第 2 行（封面）：
   封面：可用 210 / 共 273 本                          [LinearProgressIndicator(value=ready/total)]
            处理中 1 · 等待 48 · 待重试 12 · 不支持 2 · 失败 0
```

- `可用` **只计 `ready`**；`failed` / `unsupported` / `blocked` 单独列出，**不得**计入可用，也不并入"等待"（`refresh_status_counts` 已分桶，`api/remote_scan.rs:1119-1163`）。
- 分母只在 `discovery_complete == true` 时固定；发现期显示"已发现 N 本（仍在发现）"，不显示百分比。
- 同本多 profile 不重复计"本"（现有实现已按 `asset_id` 取最新状态去重，`api/remote_scan.rs:1131-1139`，保持）。

#### F. 状态机（收敛后）

| 状态 | 进入条件 | 自动重试 | 卡片 |
|---|---|---|---|
| `pending` | 新入队 / 补齐 / 对账失效 | 是 | 等待获取 |
| `running` | worker 持租约 | — | 转圈 |
| `ready` | 文件原子发布 + 校验通过 | 否 | 显示 |
| `retry_wait` | 网络 / 限流 / 405 / 5xx，且 `attempt < 3` | 到点重试（+ jitter） | 稍后重试 |
| `blocked` | 登录失效 / 开关关闭 | 会话恢复后 | 等待授权 |
| `unsupported` | **有协议证据**证明 CDN 不支持 Range | 仅新证据 | 暂不支持局部读取 |
| `failed` | 解码失败 / 存储失败 / 重试预算耗尽 | 补齐扫描 6h 后重试**一次** | 中文失败原因 |

**关键语义变化**：`upsert_job_on` 的 `state = remote_cover_job.state` 必须替换为显式收敛规则：

```
ON CONFLICT(job_key) DO UPDATE SET
  state = CASE
    WHEN remote_cover_job.state = 'ready'       THEN 'ready'      -- 已就绪不动
    WHEN remote_cover_job.state = 'running'     THEN 'running'    -- 在途不动
    WHEN remote_cover_job.state = 'blocked'     THEN 'blocked'    -- 等会话，不动
    WHEN excluded.content_revision <> remote_cover_job.content_revision
                                                THEN excluded.state -- 内容变了，重取
    ELSE excluded.state                                            -- 其余按新需求收敛
  END,
  error_code = CASE WHEN excluded.state='pending' THEN NULL
                    ELSE remote_cover_job.error_code END,
  attempt    = CASE WHEN excluded.state='pending' THEN 0
                    ELSE remote_cover_job.attempt END,
  ...
```

同时 `remote_cover_retry` 的 SQL 要扩到 `state IN ('retry_wait','failed','unsupported')`，并保留"不影响 `running` / 不跨会话发布"的既有约束。

并把 `open_*_book` 的能力判定从 `probe()` 换成 `probe_checked()`，把 405 / 网络错误映射为 `retry_wait` 而不是 `unsupported`（`api/source.rs:1277`、`api/source.rs:1519`）。

#### G. 补齐扫描的并发与预算上限

- 补齐任务 `priority=background`，与 `visible` 需求共享同一队列（`upsert_job_on` 已有 `priority=MAX(...)`，`cover_store.rs:578`）。
- 单次补齐入队上限建议 500 条/轮，避免一次性写入几十万行。
- 每来源 worker 的**并发从 1 提到 2**（受全局 governor `capacity=3`、`后台≤2` 约束，`reader.rs:16`、`reader.rs:124-141`），但**必须在 P0 的"许可拆细"之后**才能提，否则两个封面会同时长期占满后台槽。

---

## 3. 问题二：流式阅读变慢

### 3.1 符号与量化模型

- `R` = 单页字节数，`A` = 预读块 `READ_AHEAD = 256 KiB`（`source/mod.rs:100`）
- 单页 Range 请求数 `n = ceil(R / A)`（`SourceReader::read` 只在窗口耗尽时发新请求，`source/mod.rs:137-161`）
- 115 CDN 门间隔 `g = 1/4 = 250 ms`（`cloud115.rs:1116`）
- 115 API 门间隔 `= 1/1.5 ≈ 667 ms`（`cloud115.rs:1134`）

| 页大小 | `n` | Range 门控下限 `n×g` |
|---|---|---|
| 0.8 MB | 4 | **1.0 s** |
| 1.5 MB | 6 | **1.5 s** |
| 3.0 MB | 12 | **3.0 s** |

预取半径 3（`reader.rs:15`）→ 每次翻页 fan-out 最多 6 个后台页 ≈ `36 × 250 ms` 的门控排队；受 governor 限制（后台 ≤ 2）实际并发 2。

### 3.2 退化机制清单（逐条给证据与定量）

#### (1) CDN Range 被 API 的 4 QPS 罩住 —— 门控放错层

- `range_gate` 只作用于 `read_range_url` 与 `probe_checked`（`cloud115.rs:1509`、`cloud115.rs:1558`），而这两个函数请求的是 `info.url`，即 **CDN 直链**，不是 `webapi.115.com`。
- `问题记录_2026-08-08.md:121` 的原话是"限速参考 p115client（4 QPS）留余量"，上下文是**列表/API 请求**（同页第 94、95 行的现象都是 `webapi` 端点 405）。
- 结论：**保护 API 的门被误用到了 CDN 上**。CDN 侧应当限制的是"总在途 + 突发"，而不是"每 250 ms 才准发一个"。

#### (2) `RateGate::wait` 持锁 sleep —— 优先级传不下去

```rust
// source/mod.rs:39-50
pub fn wait(&self) {
    let mut last = self.last.lock().unwrap();   // ← 持锁
    ...
    std::thread::sleep(self.interval - elapsed); // ← 持锁睡觉
    *last = Instant::now();
}
```

- 等待者在互斥量上排队，**无优先级、无公平保证、不可取消**。
- 外层 `RequestPriority::Foreground`（`reader.rs:24-29`）到此完全失效——即使 governor 立刻给了前台许可，它仍要排在已经"抢到"门锁的后台请求之后，每个 250 ms。
- 这是"**外层优先级没有贯通到底层请求**"的准确落点。

#### (3) `downurl_lock` 全局单锁跨网络

```rust
// cloud115.rs:1360-1371
let _miss_guard = self.downurl_lock.lock().unwrap();   // 全客户端一把锁
if let Some(info) = self.cached_downurl(pick_code) { return Ok(info); }
self.fetch_downurl(pick_code)                          // 内含 gate.wait() + HTTP POST
```

- 锁覆盖：`gate.wait()`（最多 667 ms）+ 一次 POST（约 200–500 ms）→ **最多约 1.2 s**。
- 后台给漫画 A 取链时，阅读打开漫画 B 在**同一个锁**上等满这段时间。pickcode 不同本可以并发，却被串行化。
- 这不是 singleflight（只有同 key 合并才有意义），而是"用一把大锁换正确性"。
- 附带小问题：`downlinks` 容量满时用 `downlinks.keys().next()` 淘汰（`cloud115.rs:1340-1344`），HashMap 顺序任意 → **不是 LRU**。

#### (4) 外层优先级不贯通：`Cover` 许可粒度过粗

```rust
// api/remote_scan.rs:908-931
let _permit = governor.acquire(RequestPriority::Cover)?;   // ← 许可从这里开始
let capabilities = adapter.capabilities(&read_path, &read_fingerprint)?;  // 取链 + Range 探针
let document = crate::document::open_document(source, document_name)?;     // ZIP 中心目录多次 Range
let bytes = document.page_bytes(page)?;                                    // 首页数据多次 Range
crate::decode::decode_cover(&bytes, ...)                                   // 解码
// ← 许可到这里才释放
```

- 一个封面任务占住全局 3 个 I/O 槽之一，**跨越探测 + 压缩包解析 + 首页读取 + 图像解码**，期间在 Range 门上反复插队数秒。
- 注释（第 908-911 行）说明这是**有意为之**——防止扫描绕过前台预留。这个意图是对的，**但工具选错了**：应该把许可**拆细到"真正发网络的那一刻"**，而不是"整段任务"。
- 目录扫描同款：`Scan` 许可覆盖整个 `list_complete` 分页循环（`engine.rs:328-335`）。

#### (5) 夸克/百度 adapter 路径每次 `read_range` 重新取链

```rust
// api/source.rs:503-506
RemoteSessionClient::Quark(c) => {
    let info = c.downlink(&provider_path).map_err(scan_error)?;  // ← 每次 Range 都重新取链
    c.read_range_url(&info.url, offset, &mut bytes)
}
// api/source.rs:572-575（capabilities 里又一次）
```

- `QuarkClient` **无直链缓存**，但**有速率门**：`gate: RateGate::new(2.0)`（`quark.rs:160`），在 `request()` 入口生效（`quark.rs:192`），即**每次 `downlink` 都被 2/s 门控**。
  > **更正（P0 执行期复核）**：本节原写"夸克既无直链缓存也无速率门"——**"无门控"这一半是错的**。正确表述是：夸克有 2/s 的 API 门且它对 `downlink` 生效，缺失的只有直链缓存与合并。这使夸克比原估计更慢：adapter 路径每次 `read_range` = 一次被 2/s 门控的 `file/download`（≥500 ms 下限）+ 一次无门控的 CDN GET。对图片文件夹类漫画（走 adapter 逐页 `read_range`），这构成 **≥500 ms/页**的人工下限。
- 一次封面 = `capabilities` 1 次 + 每次 Range 各 1 次 `file/download` POST ⇒ **3~6 次取链/本**（这就是"夸克重复取链"）。
- 百度同款（`api/source.rs:561-562` 与 `491-494`）。
- 阅读路径本身因 `QuarkFile.dlink` 有内存缓存（`quark.rs:535-542`）而不重复，但**远程目录 / 封面 / 图片文件夹**全部走 adapter，全部重复。
- 夸克无门控 → 既慢又无保护，容易触发 `drive.quark.cn` 侧限流。

#### (6) CB7 / CBT / CBR 远程 = 整本下载（且在 Cover 许可内）

```rust
// document/sevenz.rs:18-21（tar.rs:18-21、rar.rs:20-23 同款）
let len = src.len() as usize;
let mut data = vec![0u8; len];
src.read_exact_at(0, &mut data)?;   // ← 远程源 = 整本下载到内存
```

- 打开一本 500 MB 的 CB7 = 下载 500 MB；封面路径也走 `open_document`（`api/remote_scan.rs:929`），于是"给一本 CB7 取封面"会下载整本，且**全程占着 Cover 许可**。
- 附带缺陷：临时文件名只带 pid —— `rch_7z_{pid}`、`rch_7z_out_{pid}`（`sevenz.rs:27,31`）、`rch_cbr_{pid}{ext}`（`rar.rs:30`）。**同进程并发打开两个 7z / CBR 会互相覆盖**，产生随机解码失败。

#### (7) 磁盘命中的残余阻塞（Dart 层）

- 阅读页已修（`reader.rs:319-322`）。
- 但 `_CoverLoadQueue.scheduler` 只有 4 个并发槽（`comic_cover.dart:144-147`），**磁盘命中的封面读也占槽**，排在网络封面之后。
- 且 `_maybeLoad` 在联网开关关闭时**根本不进入加载**（`comic_cover.dart:456`），磁盘命中也显示不出来（同 1 节表格最后一行）。

### 3.3 对照测量方案（先量化，再调参数）

> 你的要求："这些是明确的退化机制，但各自造成多少延迟，还需要同一本漫画的对照测量。" 下面这套测量**独立于任何改造**，可作为 P0 的第一步。

#### 统一埋点（Rust 侧）

| 指标 | 采集点 |
|---|---|
| `range_requests{provider}` | `read_range_url` 入口 |
| `range_gate_wait_ms{provider}` | `RateGate::wait` 调用前后计时 |
| `downurl_requests{provider}` | `downurl` / `downlink` 入口（区分缓存命中） |
| `downurl_lock_wait_ms` | `downurl_lock` 获取前后 |
| `governor_wait_ms{priority}` | `BlockingRequestGovernor::acquire` 前后 |
| `page_latency_ms{index}` | `Reader::get_page` 全程 |
| `page_cache_hit{index}` | `load_claimed` 的 `disk_get` 命中 |
| `cover_jobs{state}`、`cover_requests_per_book` | worker 循环 |
| `open_ms`、`first_page_ms` | `open_*_book` 至首个 `get_page` 返回 |

暴露方式：新增只读 FRB `perf_counters_snapshot() -> PerfSnapshotDto`（进程内累加，不持久化，不含 URL/Cookie）。

#### 对照流程

1. 同一 115 书源、同一本漫画（≥30 页、单页 ≥1 MB），先**清理该书的 `page/`、`raw/` 缓存**。
2. **场景 A（无后台负载）**：打开 → 顺序翻 10 页。记录 `open_ms`、`first_page_ms`、每页 `page_latency_ms`、`range_requests`。
3. **场景 B（有后台封面负载）**：先触发一次扫描使封面队列在跑，重复场景 A。
4. **场景 C（同场景、门关闭）**：把 `WEB_RANGE_REQUESTS_PER_SEC` 置 0（**仅测量用，不作为发布默认值**），重复场景 A。
5. 夸克同法（并额外记录 `downurl_requests`）。
6. **判据**：
   - `B − A` = "后台封面压住阅读"的实际量（当前预期 P95 增量数百 ms ~ 秒级）；
   - `A(4/s) − A(∞)` = "Range 门自身的代价"（理论值 `n × 250 ms`）；
   - 若 `A(∞)` 明显更快且**未出现 405/429 与失败计数上升** → 支持"通道解耦 + 提高 CDN 上限"；
   - 若出现 405 → 立即回退到 4/s，仅保留 P0 的优先级与合并收益。
7. **输出**：`docs/reports/` 下一份对照报告，含原始计数与百分位，不含任何 URL / Cookie / token（沿用既有脱敏约定，`api/source.rs:738-760` 的测试已在守护这一点）。

### 3.4 方案设计：通道解耦 + 阅读优先

#### A. 三条通道分开门控（核心）

| 通道 | 端点 | 现状 | 目标 |
|---|---|---|---|
| **API**（列表 / 取链 / 能力探测） | `webapi.115.com`、`drive.quark.cn` | 115: 1.5/s；夸克: 无 | 115 保持 1.5/s；夸克**新增**保守门（按实测定 1–2/s） |
| **CDN Range** | `info.url` 直链 | 115: **4/s**；夸克: 无 | 115: **可配置高上限（建议默认 20/s）**；夸克同款 |
| **本地缓存**（页 / 封面） | 磁盘 | 无门 | **明确不经过任何门**（含不占 Dart 并发槽） |

理由：CDN 直链与 `webapi` 的 WAF 面不同；对 CDN 的正确保护是"总在途上限 + 突发整形 + 405 快速失败 + 单次恢复探测"，而不是 250 ms 固定间隔。
可选整形：**令牌桶（突发 10、速率 20/s）** 比固定间隔更贴近真实负载，且不会让单页 6 个请求被拉长成 1.5 s。

#### B. 让优先级贯通到每一次网络请求

1. **优先级透传**：`RequestPriority` 通过显式参数（或 thread-local）传到 provider 客户端：
   `read_range_url(url, offset, buf, priority)` / `gate.acquire(priority)`。
   调用方已有优先级信息（`reader.rs:302` 前台、`reader.rs:384` 预取、`api/remote_scan.rs:914` 封面、`engine.rs:330` 扫描）。

2. **`RateGate` 改为优先级等待器**：
   ```
   struct PriorityRateGate {
       next_at: Mutex<Instant>,
       queues: [VecDeque<Waiter>; 4],   // Foreground / Prefetch / Cover / Scan
       changed: Condvar,
   }
   ```
   - 用 `Condvar::wait_timeout` 替代"持锁 sleep"；
   - **前台到达时若 `now >= next_at` 立即放行**——不打断已在途请求，但抢占"下一个名额"；
   - 等待**可取消**（扫描取消、卡片离屏、阅读会话结束）；
   - 同优先级内 FIFO，避免饿死（见外部对标里的 starvation guard）。

3. **许可拆细**：删掉 `fetch_remote_cover_image_with_dimensions` 外层的长 `Cover` 许可（`api/remote_scan.rs:912-915`），把许可移到 provider 客户端的每次 Range / API 调用内部（短持有）。
   - 保留原意图：扫描/封面仍然不能绕过前台预留；
   - 收益：封面任务在"探测 → CD 解析 → 读页 → 解码"之间**主动让出**，前台翻页可以插进去。
   - `engine.rs:328-331` 的 `Scan` 许可同样改为**每次 `list_page` 一次**，而不是整个分页循环一次。

#### C. 取链合并按文件，不再全局串行

```rust
// 目标形态：按 pickcode 的 singleflight
inflight: Mutex<HashMap<String, Arc<OnceLock<Result<DownloadInfo>>>>>;

fn downurl(&self, pick_code: &str) -> Result<DownloadInfo> {
    if let Some(info) = self.cached_downurl(pick_code) { return Ok(info); }
    let slot = {
        let mut map = self.inflight.lock().unwrap();
        map.entry(pick_code.to_string())
           .or_insert_with(|| Arc::new(OnceLock::new()))
           .clone()
    };  // ← map 锁立刻释放，不同 pickcode 不互相阻塞
    if let Some(done) = slot.get() { return done.clone(); }
    let result = self.fetch_downurl(pick_code);   // 每 key 只跑一次
    let _ = slot.set(result.clone());
    self.inflight.lock().unwrap().remove(pick_code);
    result
}
```

- **账号级限流与冷却继续共享**（`gate`、`waf_cooldown_until` 不动），只是不再跨 key 串行。
- 同款改造加到 `QuarkClient::downlink`（`quark.rs:309`）与 `BaiduClient::dlink`（`baidu.rs:379`）——**一并解决"夸克重复取链"**。
- 夸克直链 TTL 建议短（60–120 s，签名易变；参考 AList #7240 的观察：签名变化后旧链有时仍可用，但不要依赖），并保留 403 → `invalidate` → 重取一次。
- `downlinks` 的淘汰策略从"HashMap 任意键"改为 LRU（`cloud115.rs:1340-1344`）。

#### D. 减少请求数量

1. **合并相邻小 Range**：把预读块做成"**起始小、顺序读时倍增**"（rclone 的 `--vfs-read-chunk-size` 思路）。
   - 在 `ByteSource` 上新增 `preferred_read_ahead()`：本地保持 256 KiB，远程起始 512 KiB、顺序命中时倍增到 2 MiB 上限。
   - 收益：1.5 MB 页从 `n=6` 降到 `n=1~2`；代价是多读几百 KB（CDN 直链通常划算）。
   - 可选（P2）：需要"CD 尾 + 首页头"两段时，用**单次多段 Range**（`multipart/byteranges`，RFC 9110）合并为一个请求。
2. **压缩包索引复用**：`ZipBook::page_bytes` 每页 `self.archive.clone()`（`zip.rs:88`）→ 每次重新寻址 + 读本地文件头。中心目录已共享，本地头读取在预读块提高后自然缓解；封面场景可选"只读 CD 尾部 + 首个 entry"。
3. **已读区块复用**：给每本书加一个 `Arc<Mutex<RangeCache>>`（区间 → 字节，容量受限），命中区间直接返回，避免"翻回上一页"重新请求。
4. **磁盘/内存命中的封面不占并发槽**：`_CoverLoadQueue` 先异步查一次内存与磁盘（`remoteCoverRead` 是纯本地），命中直接完成，不进入 4 槽队列（`comic_cover.dart:144-147`）。

#### E. 阅读会话活跃时压制后台

- 在 `BlockingRequestGovernor` 增加"阅读活跃窗口"：`Reader::get_page` 前后打时间戳，最近 `T`（建议 2 s）内有前台活动时：
  - **冻结新的 `Cover` / `Scan` 许可发放**（已在途的等它结束，不抢占）；
  - 目录发现降档：`engine.rs` 的目录循环在活跃窗口内每处理 N 个目录 `sleep` 一次（或直接暂停取下一个目录）。
- **必须设上限**（否则用户长时间停留在阅读页会导致封面永久停滞）：活跃窗口最多连续压制 60 s，之后放行一批；`T` 与上限都可配置。

#### F. 格式侧

- **封面路径禁止整本下载**。短期方案：封面阶段对 CB7/CBT/CBR 直接返回带中文原因的 `unsupported`（不下载、不排队）。
- 中期可选：CBT（tar）是顺序格式，可只读前 1 个 entry 实现"伪流式"；CB7 / CBR 需要真正的随机访问实现，成本高，**建议明确列为 Out of Scope**（或改用网盘原生缩略图，见 P2）。
- 临时文件名加唯一后缀（时间戳 / 随机），修掉同进程并发互踩（`sevenz.rs:27,31`、`rar.rs:30`）。

---

## 4. 外部对标（可复用清单）

| 做法 | 来源 | 用到本项目哪里 |
|---|---|---|
| Range 读取器内建 **chunk 合并 + 区间缓存** | [rattler `async_http_range_reader`](https://github.com/baszalmstra/rattler/blob/a8e022d031da57933b22ae3a7e0a0c7f50548d6a/crates/async_http_range_reader/src/async_http_range_reader.rs) | `SourceReader` + 每本书 `RangeCache`（3.4-D） |
| `--vfs-read-chunk-size`（渐进增大）+ `--vfs-read-chunk-streams`（并发块） | [rclone VFS 调优](https://rcloneview.com/support/blog/mount-performance-tuning-rcloneview)、[rclone 论坛：vfs-read-chunk-streams](https://forum.rclone.org/t/the-new-parameter-vfs-read-chunk-streams-for-vfs/47677/13) | 预读块"起始小 / 顺序倍增"（3.4-D1） |
| 单次请求多段 Range（`multipart/byteranges`） | [RFC 9110 / HTTP Range 草案](https://datatracker.ietf.org/doc/html/draft-ietf-httpbis-p5-range-21) | P2：合并"CD 尾 + 首页头" |
| **按 key 合并并发调用**（singleflight） | [Go `x/sync/singleflight`](https://pkg.go.dev/golang.org/x/sync/singleflight)、[tower-resilience 的 request coalescing](https://docs.rs/tower-resilience-coalesce/0.9.3/i686-pc-windows-msvc/tower_resilience_coalesce/) | 取链合并（替换 `downurl_lock`，3.4-C） |
| 连接 / 限速按账号**资源池化** | [p115client Resource Pooling](https://deepwiki.com/ChenyangGao/p115client/5.3-resource-pooling) | `provider_budget` + provider 客户端分层（已有雏形） |
| 405 属 WAF 层，需**退避**而非多端点重试放大 | [AlistGo/alist #7786](https://github.com/AlistGo/alist/issues/7786) | 115 405 → 整会话冷却 + 单次恢复探测（`mark_web_waf_blocked` 已有雏形） |
| 直链签名变化后旧链可能仍可用 | [AlistGo/alist #7240](https://github.com/AlistGo/alist/issues/7240) | 夸克/百度直链 TTL 不要过短，403 才失效 |
| 优先级队列需**防饿死**（starvation guard） | [优先级命令队列的 starvation guard PR](https://github.com/openclaw/openclaw/pull/75299) | 阅读活跃窗口必须设上限（3.4-E） |
| 指数退避 + 随机抖动 | [AWS 架构博客：Exponential Backoff And Jitter](https://aws.amazon.com/blogs/architecture/exponential-backoff-and-jitter/) | `retry_wait` 的 `next_attempt_at` 加 jitter，避免冷却结束后一起重试 |
| 离线优先：Repository 统一入口，先本地后远程 | [Flutter 官方 offline-first](https://docs.flutter.dev/app-architecture/design-patterns/offline-first) | 卡片"先读盘、再声明需求"（2.3-D） |
| 高并发分块读取需**独立流**，否则位置读被串行化 | [HADOOP-16241 S3AInputStream ranged read on dedicated stream](https://issues.apache.org/jira/browse/HADOOP-16241) | 佐证"全局单锁 + 共享连接会串行化位置读"（3.2-(3)） |

> 说明：以上均为**模式参考**，不要求引入对应语言/依赖。RCH 的约束（同步 `reqwest::blocking`、无 `governor`/`moka` 依赖、FRB cdylib）决定了实现要走"自研轻量优先级门 + singleflight 表"的路线，而不是引入运行时框架。

---

## 5. 分阶段实施清单

### P0 —— 读速（不碰封面架构，风险最低，可独立验收）

- [ ] **P0-1** `RateGate` 改为 Condvar + 四优先级等待队列（**保留 4/s 默认值**，只加插队与可取消）。
- [ ] **P0-2** `downurl_lock` → 按 pickcode singleflight；`downlinks` 淘汰改 LRU。
- [ ] **P0-3** 夸克 `downlink` / 百度 `dlink` 加直链缓存 + singleflight（含 403 失效重取一次）。
- [ ] **P0-4** `ByteSource::preferred_read_ahead()` + 远程"起始小 / 顺序倍增"预读。
- [ ] **P0-5** 埋点 `PerfCounters` + `perf_counters_snapshot()` FRB。
- [ ] **P0-6** 对照测量（3.3 节），产出 `docs/reports/` 报告。
- [ ] **P0-7** （测量结论支持后）115 CDN 门从固定 4/s 改为**令牌桶 + 可配置上限**（默认 20/s）；夸克新增保守门。
- [ ] **P0-8** `open_*_book` 能力判定改 `probe_checked`，405/网络 → 明确中文错误（不再说成"不支持 Range"）。
- [ ] **P0-9** CB7/CBT/CBR 临时文件唯一化（修同进程并发互踩）。

**验收**（同书同场景，`docs/reports/` 报告为证）：
- "有后台封面负载"下 `page_latency_ms` P95 相对基线下降 **≥ 50%**；
- 每页 `range_requests` 相对基线下降 **≥ 60%**；
- `open_ms` / `first_page_ms` 不劣化；
- **无新增 405 / 429**，封面失败计数不上升；
- `cargo test` 串行全绿（含既有 governor / reader 契约测试）。

### P1 —— 封面闭环（你的 5 条建议）

- [ ] **P1-1** `upsert_job_on` 状态收敛规则（2.3-F），`remote_cover_retry` 扩到 `unsupported`。
- [ ] **P1-2** `reconcile_ready_covers()`（文件 + 长度 + 解码头部）并在提交后**唤醒 worker**。
- [ ] **P1-3** cover backfill sweep（只读索引、4 个触发点、background 优先级、边界检查）。
- [ ] **P1-4** `Cover` / `Scan` 许可拆细到"每次网络请求"（依赖 P0-1 的优先级门）。
- [ ] **P1-5** 封面阶段格式闸门：CB7/CBT/CBR 不整本下载。
- [ ] **P1-6** `remote_cover_states(source_id, asset_ids)`（只读、不发网络）+ 卡片改按来源批量订阅。
- [ ] **P1-7** 删除 `30×350ms` 轮询与 `8×900ms` 重复申请；`pending` 不转圈；失败显示中文原因。
- [ ] **P1-8** consumer 按 `assetId` 去重 + 引用计数（文件夹与代表漫画共享需求）。
- [ ] **P1-9** 无 unified view 时不再把已索引目录判成 `uncached`。
- [ ] **P1-10** 进度条 + `可用 210 / 共 273 本`；发现期分母标注"仍在发现"。
- [ ] **P1-11** （P1-4 完成后）每来源 worker 并发 1 → 2。

**验收**：
- `FolderSnapshotStore` 为空、未进入任何子目录，根目录能逐步显示图片文件夹与压缩包容器的封面；
- 同一 asset 只产生 **1 个** 任务、**1 次**提取（`extraction_count == 1`）；
- 关闭联网开关后，**磁盘已有封面仍能显示**；
- 卡片重建 20 次不新增 provider 请求；
- `ready` 但文件被手工删除后，**无需用户点击**即可自动回到 `pending` 并被重新提取；
- 一次全量重扫后，历史 `failed` 的任务能被补齐（不依赖 UI 按钮）。

### P2 —— 精细化（可延后）

- [ ] **P2-1** 阅读活跃窗口联动目录发现降频（含 60 s 压制上限）。
- [ ] **P2-2** 网盘原生缩略图作为封面兜底（115/夸克的缩略图字段）。
- [ ] **P2-3** `multipart/byteranges` 合并"CD 尾 + 首页头"。
- [ ] **P2-4** 每本书区间缓存（跨页复用）。
- [ ] **P2-5** CBT 伪流式（只读首个 entry）。
- [ ] **P2-6** 把 P0/P1 的契约沉淀进 `.trellis/spec/backend/`。

---

## 6. 风险、边界与「不做什么」

**回滚边界**
- 每个阶段独立：P0 全是"减延迟"，回退 = 恢复常量与锁形态，不涉及 schema。
- P1 涉及状态机语义变更：回归路径 = `upsert_job_on` 恢复"保留旧状态" + 停用 backfill 触发点；**不删库、不清缓存**。
- 若 115 CDN 上限提高后出现 405：立即把 `WEB_RANGE_REQUESTS_PER_SEC` 调回 4，仅保留优先级门与请求合并的收益（P0-1/2/4 仍然有效）。

**不做什么**
- **不**移除或放宽 API 侧限速（`gate = 1.5/s`、`waf_cooldown`）。
- **不**给出未经真实账号验证的"安全 QPS"；CDN 上限以对照测量为准。
- **不**做 CB7 / CBR 的真正随机访问（成本高、收益有限），只保证"封面不整本下载"。
- **不**让补齐扫描绕过联网开关 / 登录态 / 账号预算 / Range 证据要求。
- **不**在 UI 层自行递归列目录（既往评审已否决该路线）。
- **不**改动删除保护、tombstone、`baseline` 证明规则（`persistence.rs:1535-1548` 的两阶段语义）。
- **不**改 SPEC、不动 Reader BookKey / 历史阅读记录、不引入新 crate（P2 若确需再单独确认）。
- 遵守 `CLAUDE.md`：本轮只出方案，**实现前需你确认**；每阶段先更新 `LOG.md` / `LOG-INDEX.md`，验证通过后再更新 `README.md`。

---

## 7. 需要你确认的点（审阅时一并答复即可）

1. **P0 是否先单独做一轮**（含对照测量），确认读速回来后再动封面？
2. **115 CDN 门的目标形态**：令牌桶（突发 10 / 速率 20/s）还是先保持固定间隔只加优先级？（我建议前者，但需 P0-6 的实测支持）
3. **CB7 / CBT / CBR 的封面**：接受"显示暂不支持局部读取（中文原因）"，还是要求引入网盘原生缩略图？
4. **补齐扫描的周期与触发点**（启动后 30 s / 每次扫描后 / 每 10 分钟 / 手动）是否符合预期？是否需要"仅 Wi-Fi"之类的额外约束？
5. **`failed` 自动重试上限**：6 小时后重试一次，还是要求永不自动重试（只手动）？

---

## 8. 证据索引（文件:行）

**封面扫描**

| 结论 | 位置 |
|---|---|
| upsert 无条件保留旧状态 | `app/rust/src/remote_scan/cover_store.rs:576-582` |
| 会话/代际匹配的 claim（重启后旧代际不可认领） | `app/rust/src/remote_scan/cover_store.rs:729-791` |
| 对账把 ready 改回 pending | `app/rust/src/remote_scan/cover_service.rs:60-116`（改状态在 99-114） |
| consumer 注册表 | `app/rust/src/remote_scan/cover_service.rs:16-56` |
| `RangeUnavailable/Unsupported → Unsupported`（终态） | `app/rust/src/api/remote_scan.rs:152-186`（162-166） |
| 发现期即入队 + 唤醒 worker | `app/rust/src/api/remote_scan.rs:671-702` |
| 每来源单线程 worker；key = `{source}:{session}` | `app/rust/src/api/remote_scan.rs:125-150` |
| 队列空即退出 | `app/rust/src/api/remote_scan.rs:331-345` |
| 每次 claim 复查联网开关 | `app/rust/src/api/remote_scan.rs:295-302` |
| 账号预算 100ms | `app/rust/src/remote_scan/provider_budget.rs:77-82` |
| `consume_staged_covers` 仅在整树发布后调用 | `app/rust/src/api/remote_scan.rs:1501-1518`、`1602-1675` |
| 状态 DTO 字段齐备 | `app/rust/src/api/remote_scan.rs:32-57` |
| 7 桶计数（失败不计成功） | `app/rust/src/api/remote_scan.rs:1079-1211` |
| 目录展示查询 + 代表封面状态 | `app/rust/src/remote_scan/catalog.rs:313-402` |
| 目录视图入口 | `app/rust/src/api/remote_cover.rs:88-118` |
| 卡片请求：非阻塞 + 只读磁盘 | `app/rust/src/api/remote_cover.rs:235-470` |
| 重试 SQL 只捞 retry_wait/failed | `app/rust/src/api/remote_cover.rs:496-545` |
| 重启后已完成代际重绑（只含 pending/running/retry_wait） | `app/rust/src/remote_scan/persistence.rs:478-553` |
| 卡片 10.5s 轮询 | `app/lib/ui/comic_cover.dart:660-681` |
| 卡片 8×900ms 重复申请 | `app/lib/ui/comic_cover.dart:486-503` |
| 联网开关挡住磁盘命中 | `app/lib/ui/comic_cover.dart:446-464`、`700-731` |
| 排队也转圈 | `app/lib/ui/comic_cover.dart:702-729` |
| consumerId 含实例哈希 | `app/lib/ui/comic_cover.dart:284-285` |
| Cover 并发槽 4（含磁盘命中） | `app/lib/ui/comic_cover.dart:144-147` |
| 中文状态映射存在但未被卡片使用 | `app/lib/store/remote_cover_repository.dart:146-167` |
| 文件夹取代表漫画 assetId | `app/lib/ui/source_browser.dart:1565-1571` |
| 无 unified view 时回落快照判定 | `app/lib/ui/source_browser.dart:551-580` |
| 目录视图 180ms 合并刷新 + 增量比对 | `app/lib/ui/source_browser.dart:464-507` |
| 状态面板两行文案 | `app/lib/ui/remote_scan_status.dart:140-149` |
| 来源级 500ms 轮询（可复用为批量订阅） | `app/lib/store/remote_scan_coordinator.dart:64-70`、`404-441` |

**流式阅读**

| 结论 | 位置 |
|---|---|
| `WEB_RANGE_REQUESTS_PER_SEC = 4.0` | `app/rust/src/source/cloud115.rs:1113-1116` |
| 115 API 门 1.5/s | `app/rust/src/source/cloud115.rs:1134` |
| 405 → 整会话冷却 60s | `app/rust/src/source/cloud115.rs:1097-1100`、`1173-1179` |
| 115 直链缓存 + TTL + 任意键淘汰 | `app/rust/src/source/cloud115.rs:1104-1111`、`1322-1352`（淘汰在 1340-1344） |
| `downurl_lock` 全局锁跨网络 | `app/rust/src/source/cloud115.rs:1360-1371`、`1373-1405` |
| 取链门控在锁内 | `app/rust/src/source/cloud115.rs:1384-1385` |
| `probe_checked` 走 range_gate | `app/rust/src/source/cloud115.rs:1503-1538` |
| `read_range_url` 走 range_gate | `app/rust/src/source/cloud115.rs:1550-1598` |
| `probe()` 把错误压成 false | `app/rust/src/source/cloud115.rs:1540-1548`、`app/rust/src/source/quark.rs:351-357` |
| `RateGate::wait` 持锁 sleep | `app/rust/src/source/mod.rs:18-51` |
| `READ_AHEAD = 256 KiB` + 窗口逻辑 | `app/rust/src/source/mod.rs:99-162` |
| 优先级枚举与队列语义 | `app/rust/src/reader.rs:19-158` |
| governor 容量 3 / 队列 64 / 后台 ≤2 | `app/rust/src/reader.rs:16-17`、`112-142` |
| 预取半径 3 | `app/rust/src/reader.rs:14-15`、`366-387` |
| 磁盘命中不排网络（已修） | `app/rust/src/reader.rs:316-325`、`490-520` |
| Cover 许可跨探测+解压+解码 | `app/rust/src/api/remote_scan.rs:870-936`（许可 908-919） |
| Scan 许可跨整个分页循环 | `app/rust/src/remote_scan/engine.rs:325-359` |
| 夸克 adapter 每次 read_range 重取链 | `app/rust/src/api/source.rs:488-511`（503-506） |
| 夸克 capabilities 也重取链 | `app/rust/src/api/source.rs:551-581`（572-575） |
| 百度同款 | `app/rust/src/api/source.rs:491-494`、`560-563` |
| 打开路径仍用旧的 `probe()` | `app/rust/src/api/source.rs:1277`、`app/rust/src/api/source.rs:1519` |
| 夸克无直链缓存 / 无门控 | `app/rust/src/source/quark.rs:143-172`、`308-338` |
| 夸克文件级 dlink 缓存（仅阅读路径受益） | `app/rust/src/source/quark.rs:521-568` |
| CB7 整包下载 + pid 临时名 | `app/rust/src/document/sevenz.rs:16-46` |
| CBT 整包下载 | `app/rust/src/document/tar.rs:16-37` |
| CBR 整包下载 + pid 临时名 | `app/rust/src/document/rar.rs:16-35` |
| ZIP 已改用 `name_for_index`（**但实测未生效，见第 1 节**） | `app/rust/src/document/zip.rs:42-66` |
| ZIP 每页 `archive.clone()` | `app/rust/src/document/zip.rs:81-93` |
| 4 QPS 建议的原始上下文是 API | `docs/project/问题记录_2026-08-08.md:94-95`、`121` |

---

**规划完成状态**：本文件只新增方案，**未修改任何生产代码**；第 5 节复选框全部未完成。执行入口为 P0-5（埋点）+ P0-6（对照测量），用数据决定 P0-7 的门控参数，再进入 P1。
