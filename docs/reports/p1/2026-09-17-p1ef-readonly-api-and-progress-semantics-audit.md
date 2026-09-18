# P1-E/F 实施审计：只读 state API、F 聚合语义、既有 revision

> `D:/Projects/RCH-p1`｜分支 `p1-cover-completion`｜HEAD `1bf2e3743c026581ddaa9d444787fde05fd02f31`（未变）
> **本轮对生产代码零修改**｜未 commit / push / merge / reset / stash

## 状态

> E/F **实现未开始**（未改一行生产代码）。本轮完成的是你 §9 / §14 / §15 明确要求先核对的三个**硬前提**，
> 以及一个可能显著降低 E 工作量的发现（§3）。

---

# 1. §9 —— 是否存在真正的只读 asset durable-state API

| 候选 | 位置 | 结论 |
|---|---|---|
| `cover_state_for(...)` | `src/remote_scan/catalog.rs:313` | ❌ **private，未导出**（既无 FRB 入口，也不是 `pub`）——但它正是"给定 source+asset 读 durable state"的内部实现。**最小做法：加一个只读 FRB 包装，复用此函数与 `RemoteCoverStateDto`** |
| `remote_cover_request(...)` | `src/api/remote_cover.rs:236` | ❌ **会 enqueue**（创建/zombie job）⇒ 按你 §9 明确规定**不可**当 reader 复用 |
| `remote_cover_read(...)` | `:457` | ❌ 返回**像素**（`PageImage`），不是状态 |
| `remote_cover_retry(...)` | `:489` | ❌ 显式写操作 |
| `remote_view_revision(source_id)` | `:560` | ⚠️ 只读，但返回**source 级 revision**，不是 asset 状态（见 §3，对本任务另有价值） |
| `remote_directory_view(...)` | `:89` | ❌ 目录视图，非单 asset 状态 |

**结论**：**不存在**可直接复用的"sourceId + assetId → 当前 durable cover state"只读 API ⇒
按 §9 必须新增一个**最小** Rust/FRB read-only API（DB read only / no session / no provider / no enqueue /
no wake / no mutation），返回可表达
`no job / pending / running / retry_wait / ready / failed / unsupported / blocked`；
**优先复用现有 `RemoteCoverStateDto`**（不另建平行状态 enum），内部直接走 `catalog.rs:313 cover_state_for`。

# 2. §14 / §15 —— `refresh_status_counts` 的真实 SQL / 逻辑语义

位置：`src/api/remote_scan.rs`（`fn refresh_status_counts`）。

## 2.1 `discoveredBooks`（分母）

```sql
SELECT COUNT(DISTINCT id) FROM library_index
 WHERE source_id=?1 AND scan_generation=?2 AND deleted=0
   AND ((entry_type='file' AND asset_kind='ArchiveFile')
        OR (entry_type='dir' AND asset_kind='ImageFolder'))
```
再取 `max(staged_cover_tasks)`：
`status.discovered_books = indexed_books.max(staged_books)`。

⇒ ✅ **是当前 source 的 durable eligible comic total**（按 `scan_generation` 限定，已排除 `deleted=1`，
且只算 ArchiveFile / ImageFolder 两种 asset_kind）。
**结论：`discoveredBooks` 可作为 F 的 denominator，无需修改**（§15 第一项成立）。

## 2.2 七个桶

```sql
SELECT asset_id,state,updated_at FROM remote_cover_job
 WHERE source_id=?1 AND generation=?2
```
逐行按 `asset_id` 取 **`updated_at` 最新** 的那条（`latest_by_asset`），再落到 `counts[index]`，
`"ready" => Some(0)`（即 index 0 = ready，其余 6 桶按同一 7 元数组映射：
active/pending/retry/blocked/unsupported/failed）。

⇒ ❌ **`readyBooks` 只是 `job.state == 'ready'`，完全没有验证 cover 字节/文件是否存在。**
**结论：按 §15/§16，`readyBooks` 不能当 `available`；需要在 Rust 只读聚合新增 `availableBooks`**
（复用 P1-B 既有 "ready but file missing" 判定逻辑；**统计函数绝不修改 DB**，
真正 reconcile 仍由现有 P1-B 路径负责）。

## 2.3 第三个必须回答的问题：no-job item 落哪个桶

桶来自 `remote_cover_job` 行；而 `discoveredBooks` 来自 `library_index`。
⇒ **eligible 但没有 job 行的漫画会被计入分母、却不落在任何桶**。

因此现状下：

```
available + waiting + running + failed + unsupported + blocked + other  <  total
```

**这正是你 §18 要求"必须明确归属、不允许 silently disappear"的缺口。**
最小修法（按你 §18 推荐）：把 no-job eligible items 计入 **waiting**（它仍是"需要补齐的 cover"），
并由测试钉死；若现有 7 元聚合结构难以无歧义表达，则新增字段（`noJobBooks` 或 `staleReadyBooks`）
并在 invariant 中归入 `other`。**以最小字段变化为准，实施时再定。**

# 3. 可能显著降低 E 工作量的发现：per-source revision 已经以 durable 形式存在

| 组件 | 位置 |
|---|---|
| durable 表 `remote_view_revision` | `src/remote_scan/cover_store.rs:87` |
| 迁移注册 | `cover_store.rs:234` |
| bump 函数 `bump_view_revision_on(conn, source_id, listing_generation, ...)` | `cover_store.rs:324`（`INSERT INTO remote_view_revision(source_id,revision,listing_generation,updated_at)`） |
| 读取 `view_revision(conn, source_id)` | `cover_store.rs`（`catalog.rs:465` 在读） |
| **FRB 只读入口** | `src/api/remote_cover.rs:560 pub fn remote_view_revision(source_id) -> Result<i64, String>` |
| 已并入 scan DTO | `src/api/remote_scan.rs:57 view_revision`，在 `:1395-1396` 由 `cover_store::view_revision(...)` 填充 |

⇒ E 的"source-scoped cover revision"**不一定需要新增 stream**：
如果 `bump_view_revision_on` 已经在 cover job 的 durable 迁移点被调用，那么
**既有 durable revision 就是 E 所需的"事实"**，Dart 侧只需把它变成 wake-up
（例如 coordinator 在 transition 通知到达时推进一个 `ValueNotifier<int>`，
card/aggregate 据此重读），从而**无需新增 FRB stream**。

**待确认（实施第一步）**：`bump_view_revision_on` 的**调用者集合**是否覆盖你 §4 要求的全部 transition：
job created→pending / claim→running / →ready / →retry_wait / →failed / →unsupported / →blocked /
ready-missing reconcile→pending / long-compensation reconcile→pending / retry_wait 再执行后的 claim。
若覆盖不全，再按 §1 补窄发射点（仍然不是 general event bus）。
**这不是 STOP 条件**（§22 明确允许"增加现有 FRB stream/event 的一个 payload"或"窄 source-level revision"）。

# 4. 未实施项（与 §24 的差距）

| # | 项 | 状态 |
|---|---|---|
| 1 | E notification RED（E-1/E-2/E-3/E-4/E-5/E-6 及原 E-1~E-11） | ❌ 未开始 |
| 2 | `bump_view_revision_on` 调用者覆盖性核对（§3 待确认项） | ❌ 未开始 |
| 3 | 窄 cover revision（stream 或复用既有 revision + coordinator wake-up） | ❌ 未开始 |
| 4 | 只读 asset state API（FRB 包装 `cover_state_for`） | ❌ 未开始（已确认**必须新增**） |
| 5 | Dart coordinator bridge（source revision `ValueNotifier<int>`，只 wake-up 不存 state） | ❌ 未开始 |
| 6 | 删除 `30×350ms`（`comic_cover.dart:852-868`）与 `8×900ms`（`:583-585`） | ❌ 未开始 |
| 7 | E state→UI matrix（§12 冻结表） | ❌ 未开始 |
| 8 | F 语义修正：`availableBooks`（§16）+ no-job/ stale-ready 归桶（§17/§18） | ❌ 未开始 |
| 9 | F RED（F-1~F-10 + F-11 scan-terminal event-driven） | ❌ 未开始 |
| 10 | codegen / generated diff audit / P1 总门禁 | ❌ 未开始 |

**本轮零生产代码修改** ⇒ `P1_D2_PASS` 证据不受影响；**`P1_PASS` 未声明**；未进入 P2。

# 5. 门禁与完整性

| 项 | 结果 |
|---|---|
| 生产代码修改 | **零**（仅只读 grep/sed/read） |
| `git diff --check` / staged | clean / **0** |
| `P1_RECOVERY_INTEGRITY_PASS` | 未受影响（零文件改动） |
| branch / HEAD | `p1-cover-completion` / `1bf2e37`（未变） |
| 临时产物 | 无 |
| baseline exception（保留，未修） | `cargo clippy` `src/reader.rs:277`；全仓 `flutter analyze` 121 issues；完整 `flutter test` 的 6 个既有编译/装配失败 |

## Remaining risks

1. **§2.3 的不变量缺口是现存缺陷**：eligible-but-no-job 漫画当前落在分母之外的所有桶之外
   （silently disappear）。F 必须显式定义其归属（建议 `waiting`），并由测试钉死。
2. **§2.2 的 `readyBooks` 无字节校验**：`availableBooks` 必须新增且复用 P1-B validity 判定；
   统计层只读、绝不触发 reconcile mutation。
3. E 的发射点覆盖性（§3 待确认项）决定是否真的需要新 stream；若 `bump_view_revision_on` 已覆盖
   全部 transition，可复用既有 durable revision，避免新增传输通道。
4. `discoveredBooks` 以 `scan_generation` 限定；若卡片所在 generation 与 status 的 generation 不一致，
   numerator/denominator 可能不同代 —— F 实施时需明确以**当前展示 generation** 为准。
5. 115-web / Quark 的 raw-key `present→absent` 仍 known unrecoverable（P1-D2 冻结）。
6. **Release Gate PENDING**（真实 115/Quark 校验、P0 S1–S5、无新增 403/405/429、CDN rate/burst/in-flight
   实测、20/100/300 MiB Range 证据）—— P1 完成 ≠ 可发布。
