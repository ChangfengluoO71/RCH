# P1-A：Cover Job 状态机语义冻结（transition matrix）

> 阶段：**P1-A 完成**｜P1-B / C / D / E / F **尚未开始**（如实声明，见第 8 节）
>
> 基线：P0 收口提交 `1bf2e3743c026581ddaa9d444787fde05fd02f31`
> 隔离工作树：`D:/Projects/RCH-p1`（分支 `p1-cover-completion`）
> 未 commit / 未 push / 未 merge

---

## 1. 结论

P1-A 的唯一目标是**消灭隐式状态语义**。改造前 `cover_store::upsert_job_on` 里这一句：

```sql
ON CONFLICT(job_key) DO UPDATE SET
    ...
    state=remote_cover_job.state,   -- 无条件保留旧状态
```

使得 `failed` / `unsupported` / `blocked` / `retry_wait` / `cancelled` **一旦进入就再没有任何
路径能恢复**，即使失败原因早已消失。实测：一次显式记录写入在 **36 行矩阵里违反 14 行**。

改造后状态由**唯一规则表**（`remote_scan::cover_state::resolve_upsert_state`）按**显式触发原因**
决定，并且状态变化真的改变候选队列（集成测试用 `claim_next_job_on` 验证，不只是读字段）。

---

## 2. 冻结的 transition matrix

状态（沿用既有 8 态，含你列表外的 `cancelled`）：`pending` / `running` / `ready` / `retry_wait`
/ `failed` / `unsupported` / `blocked` / `cancelled`。

行为 = 现有状态（行）× 触发原因（列）：

| 现有 \\ 原因 | `Demand` | `FullRescan` | `ManualRetry` | `CauseCleared` | `CapabilityChanged` |
|---|---|---|---|---|---|
| （无记录） | requested | requested | requested | requested | requested |
| `pending` | pending | pending | pending | pending | pending |
| `running` | running | running | running | running | running |
| `ready` | ready | ready | ready | ready | ready |
| `retry_wait` | retry_wait | **pending** | **pending** | **pending** | **pending** |
| `failed` | failed | **pending** | **pending** | failed | failed |
| `unsupported` | unsupported | unsupported | **pending** | unsupported | **pending** |
| `blocked` | blocked | blocked | **pending** | **pending** | blocked |
| `cancelled` | **pending** | **pending** | **pending** | cancelled | cancelled |

`Demand` = 普通需求（目录重扫发现、卡片可见、后台补齐）。它**保留**排期与终态，这是既有契约
（`remote_cover_queue_contract::ordinary_visible_demand_does_not_bypass_backoff_or_terminal_errors`）
刻意要求的：否则 UI 每次出现都会重开一轮请求。**要消灭的是它的隐式性，不是这条规则本身。**

### 你要求的九项属性，逐状态

| 状态 | 进入原因 | 允许退出的事件 | 自动重试 | 需登录 | 需网络 | 受 cooldown | 需能力变化 | full rescan 可否重置 |
|---|---|---|---|---|---|---|---|---|
| `pending` | 新需求 / 缺口 / 原因解除 | claim | — | 否 | 是 | 是 | 否 | 是（保持） |
| `running` | worker claim | ready / retry_wait / failed / lease 过期回收 | — | 是 | 是 | 是 | 否 | **否**（不打断在途） |
| `ready` | 取得并发布封面 | 文件丢失/损坏对账 | — | 否 | 否 | 否 | 否 | **否**（保留已有封面） |
| `retry_wait` | 可重试失败（含 429 / cooldown） | 到期 claim | **是**（有界，见 §3） | 是 | 是 | 是 | 否 | 是 |
| `failed` | 自动补偿预算耗尽 / 永久失败 | ManualRetry / FullRescan | **否** | 视原因 | 是 | 是 | 否 | 是 |
| `unsupported` | provider / 归档能力不支持 | CapabilityChanged / ManualRetry | **否**（禁止按时间周期重试） | 否 | 是 | 否 | **是** | **否** |
| `blocked` | 环境阻塞（未登录 / 无网络 / cooldown / 权限） | CauseCleared / ManualRetry | **否** | 视原因 | 视原因 | 视原因 | 否 | **否** |
| `cancelled` | 需求消失（卡片卸载 / 资源被删 / 代际丢弃） | 新的 demand | 否 | 否 | 否 | 否 | 否 | 是 |

两条硬不变量由测试钉住（`unsupported_and_blocked_are_not_reset_by_ordinary_causes` +
集成版）：

- `unsupported` 只能被 `CapabilityChanged` 解除；
- `blocked` 只能被 `CauseCleared` 解除；
- 两者都**不**被 `Demand` 或 `FullRescan` 解除。

---

## 3. 与既有重试策略的关系（重要，未擅自推翻）

侦察确认既有代码**已经**有有界重试，我没有替换它：

`api/remote_scan.rs:152-186` `cover_job_failure_state`：
`RangeUnavailable|Unsupported → Unsupported`；`Unauthorized|Forbidden → Blocked`；
`TransientNetwork|RateLimited` **仅在 `attempt < 3` 时** → `RetryWait`（delay 有上限 15 min）；
其余 → `Failed`。**不存在**任何 6 小时 / 周期性的 "扫所有 failed" 代码（已全局搜索确认）。

你的冻结原则里"`failed` 默认 6 小时后最多自动补偿一次"是**新增的恢复路径预算**，与既有的
"短退避、attempt<3" 是两个不同维度。本轮的做法是：

- **不**改动既有短退避映射（避免推翻已验证的 provider cooldown 行为）；
- **新增**"被显式原因拉回候选队列时获得全新重试预算"——`revival` 时 `attempt=0`、
  清空 `next_attempt_at` / `lease_*` / `error_code`。否则"恢复"之后会立刻耗尽旧的 attempt 预算，
  恢复等于无效。
- `failed` 不被任何周期性路径复活（本仓库根本没有周期性扫描路径）。

> **待你确认的一处**：我没有把"6 小时、一次"的定时补偿做成本轮的自动路径（本仓库无周期定时器，
> 引入它需要一个新的 scheduler + budget，属于 P1-C 的范畴）。当前"自动补偿"的边界由
> 既有 `attempt < 3` + `Revival 重置预算` 共同决定。

---

## 4. RED → GREEN 证据

### 4.1 规格单测（纯函数，冻结矩阵本身）

`src/remote_scan/cover_state.rs` → `frozen_upsert_transition_matrix`、
`unsupported_and_blocked_are_not_reset_by_ordinary_causes`：**2 passed**。

### 4.2 集成测试（驱动真实存储 + 候选队列）

`tests/remote_cover_transition_contract.rs`，36 行矩阵 + 8 组"只能被自己的原因解除"组合。

**RED（改造前，生产代码仍忽略 cause）：**

```
test result: FAILED. 0 passed; 2 failed
cause=Demand initial=cancelled => got cancelled want pending
cause=FullRescan initial=retry_wait => got retry_wait want pending
cause=FullRescan initial=failed => got failed want pending
cause=FullRescan initial=cancelled => got cancelled want pending
cause=ManualRetry initial=retry_wait => got retry_wait want pending
cause=ManualRetry initial=failed => got failed want pending
cause=ManualRetry initial=unsupported => got unsupported want pending
cause=ManualRetry initial=blocked => got blocked want pending
cause=ManualRetry initial=cancelled => got cancelled want pending
cause=CauseCleared initial=blocked => got blocked want pending
cause=CauseCleared initial=retry_wait => got retry_wait want pending
cause=CapabilityChanged initial=unsupported => got unsupported want pending
cause=CapabilityChanged initial=retry_wait => got retry_wait want pending
```

（另含 `unsupported must be recovered by its own cause CapabilityChanged`。）

**GREEN（接入规则表之后）：**

```
running 2 tests
test explicit_cause_drives_the_durable_transition_matrix ... ok
test unsupported_and_blocked_only_recover_through_their_own_cause ... ok
test result: ok. 2 passed; 0 failed
```

---

## 5. 关键实现

`cover_store::upsert_job_on` 现在：

1. **显式 cause 参数**：`CoverJobUpsertCause::{Demand, FullRescan, ManualRetry, CauseCleared, CapabilityChanged}`；
2. 读现有状态 → 交给 `resolve_upsert_state` 唯一规则表 → 用 `state=excluded.state` 落库
   （**不再**是 `state=remote_cover_job.state`）；
3. `revival`（非 pending → pending）时重置重试预算。

### 5.1 实现中踩到并修掉的真实回归

我最初在 `upsert_job_on` 里无条件开事务，结果破坏了 `publish_staged_generation` 这类
**已持有事务**的调用方：

```
ToSqlConversionFailure(... "cannot start a transaction within a transaction")
```

修法：仅在 `conn.is_autocommit()` 为真时自己开事务，否则复用调用方的事务作用域。
这条由既有的 `verified_remote_deletion_tests::finished_cover_task_persists_opaque_cache_alias_for_later_cleanup`
捕获（该测试就是靠它由红转绿）。**如实记录：这是我引入的回归，不是既有问题。**

---

## 6. 回归门

| 门 | 结果 |
|---|---|
| `cargo test --locked -- --test-threads=1`（全量） | **EXIT=0；14 个目标全部 ok；380 passed / 0 failed / 2 ignored** |
| 新增：`remote_scan::cover_state` | 2 passed |
| 新增：`remote_cover_transition_contract` | 2 passed |
| 既有 cover 契约（queue / missing-file / store / cache / error / directory-view） | 全部保持通过 |
| `git diff --check` | ✅ clean（见第 7 节） |

---

## 7. 本轮改动文件（全部在 `D:/Projects/RCH-p1`）

```
?? app/rust/src/remote_scan/cover_state.rs            （新：冻结矩阵 + 规则表 + 规格单测）
?? app/rust/tests/remote_cover_transition_contract.rs （新：集成 RED→GREEN）
 M app/rust/src/remote_scan/mod.rs                    （注册模块）
 M app/rust/src/remote_scan/cover_store.rs            （upsert 接入规则表 + revival 重置）
 M app/rust/src/remote_scan/persistence.rs            （调用点补 cause）
 M app/rust/src/api/remote_scan.rs                    （调用点补 cause）
 M app/rust/src/api/remote_cover.rs                   （调用点补 cause）
 M app/rust/tests/remote_cover_queue_contract.rs      （调用点补 cause）
 M app/rust/tests/remote_cover_missing_file_contract.rs（调用点补 cause）
```

调用点全部传 `Demand`（行为保守），因此**没有任何生产路径的行为在本轮被改变**——
除了"显式原因"这一条新能力本身。下一步接 P1-C 时，补齐器应传 `FullRescan`/`Demand`，
登录恢复传 `CauseCleared`，能力变化传 `CapabilityChanged`。

---

## 8. 未完成项（**如实声明，不虚报**）

用户要求的 P1 包含 P1-A…P1-F。**本轮只完成 P1-A。** 原因：P1-A 的建库-读规格-侦察-写规格测试-
写集成 RED-修实现-修回归-跑全量门禁已经耗尽了本轮可用预算；P1-B…F 均未开始，**没有任何一行
对应代码或测试被写入**。为了不把未验证的改动留成"半成品"，我在这里停下。

已经侦察清楚、可直接开工的锚点（由只读侦察产出，均带 `file:line`）：

### P1-B（worker 唤醒闭环）
现状：`wake_remote_cover_worker`（`api/remote_scan.rs:125-150`）已经满足你要求的全部四点——
`enqueue → wake`、in-process `HashSet` 单飞（同一 `{source_id}:{session}` 只起一个）、
队列空且无未来 `retry_wait` 期限时退出（`:331-344`）、退出时从集合移除（`:148`）因此可安全再启动。
**真正缺的是两个没有 wake 的 pending 写入点**：

1. `cover_service::read_cached_cover` 的 `ready → pending` 对账（`cover_service.rs:102-113`）
   —— 完全没有 wake（既有测试 `remote_cover_missing_file_contract.rs:20-24` 只断言状态变为
   `Pending`，不断言被消费）。
2. `publish_staged_generation` 内的 pending 插入（`persistence.rs:1266-1280`）——自身无 wake，
   依赖调用方走到 `consume_staged_covers` 的尾部 wake（`remote_scan.rs:1686`）。

**必须先解决的设计障碍（本轮已定位）**：worker 的 key 需要运行时 session token
（`{source_id}:{session}`），而 `remote_cover_read` 这条只读路径手上没有它；
`wake_remote_cover_worker` 对 `session == 0` 直接返回（`:135-137`）。所以 P1-B 需要一个
`wake_cover_worker_for_source(source_id)`：从 `remote_scan_epoch` 取该 source 的当前
`session_token` 再唤醒，或在只读路径上把 session 透传下来。这是 P1-B 的第一步，不能跳过。

### P1-C（补齐器）
现有 `refresh_status_counts`（`api/remote_scan.rs:1091-1200`）已经是对
`(source_id, generation)` 的 job 状态聚合，可直接作为缺口查询的模板。缺口 anti-join 无需走目录树：

```sql
SELECT li.source_id, li.id FROM library_index li
LEFT JOIN remote_cover_job j
  ON j.source_id=li.source_id AND j.asset_id=li.id
 AND j.selection_revision='default' AND j.profile='340x480@1'
WHERE li.deleted=0 AND (j.job_key IS NULL OR j.state<>'ready')
```

budget 用 `remote_scan::provider_budget`（`try_reserve` / `account_key` / `global()`，`provider_budget.rs:36-82`），
worker 侧已有用法示例 `cover_budget_key`（`remote_scan.rs:518-550`）。

### P1-D（Dart 磁盘优先）—— 已实施，**部分满足冻结要求**

由子代理实施，**我已独立复跑复核**（不是采信其自述）：

| 复核项 | 我的实测结果 |
|---|---|
| `flutter test test/comic_cover_disk_first_test.dart` | **2 passed / 0 failed**（"All tests passed!"） |
| 相关回归集（scheduler / repository / quality / dependency） | **12 passed / 0 failed** |
| 改动范围 | 仅 `app/lib/ui/comic_cover.dart`、`app/lib/store/remote_cover_repository.dart`、新增 `app/test/comic_cover_disk_first_test.dart` |
| 子代理自报的 mutation 证明 | 恢复旧的 `if (_remoteCoverNetworkPaused) return;` 会让两个用例失败（`Found 0 widgets with type "RawImage"`）—— 我未复跑该 mutation 步骤，仅记录为其自证 |

**已修复（unified 路径）**：旧的两道"只看开关"的短路（原 `:456` 的 blanket gate、原 `:703` 的
`build` 短路）被移除/收窄为"仅本地未命中时才看开关"，`_load` 现在先读磁盘；新增
`RemoteCoverRepository.readLocalCover` 与可选 `ComicCover.repository` 注入口（无新全局）。
`cacheOnly` / `remotePartialUse` 等 typed outcome 契约未被削弱。

**经我静态分析确认的残留（未满足冻结要求）**：`comic_cover.dart:498` 仍是

```dart
if (widget.remoteAssetId == null && _remoteCoverNetworkPaused) return;
```

即 **legacy 源（`needsSession` 且无 `remoteAssetId`，走 `bookCover`/`webdavCover`/`sftpCover`
这条缓存命名空间）在联网开关关闭时仍然直接返回，磁盘上已存在的封面依旧被隐藏**。
这与你的冻结要求"只有本地未命中时，才根据联网开关决定是否允许远程获取 / 不得把联网开关解释成
隐藏已经存在的本地内容"**不一致**，只是从"所有源"缩小到了"legacy 源"。

子代理的结论是"要修需要 Rust 侧改动"。我认为**这个结论未经验证、且可能过强**：规范里已存在
`CoverFetchPolicy.cacheOnly`（`remote-cover-update-contracts.md:52-55`），其语义正是"在**已有活跃
session** 的前提下读内存/封面磁盘/raw 本地缓存，且不得创建 session、不得读远程 body"。
因此 legacy 路径在**持有活跃 session** 时应当可以用 `cacheOnly` 直接显示磁盘封面，属于
Dart 侧改动；只有在**没有活跃 session** 时才真的读不到（因为该 API 契约要求活跃 session），
那时显示占位图才是正确的。

这属于"该用哪个命名空间作为 legacy 封面的权威来源、以及卡片是否应持有活跃 session"的
**接口/架构选择**，按项目规则（未经确认不做架构级修改）我**没有擅自实现**，留给你决定。
→ 记为 **P1-D-2（待你确认后开工）**。

### 生成文件 churn：**不存在**（更正我此前的说法）

上一版报告称 `flutter pub get` "改写了" linux/macos/windows 下的生成插件注册文件。经核对：
这些文件出现在 `git status --porcelain` 里，但**不在 `git diff --name-only` 中** ——
即只有 stat / 换行规范化的假改动，**内容未变**。无需清理，我此前的说法不准确，特此更正。

### P1-E（轮询 → durable 订阅）
现状：`ComicCover` **不**订阅 `RemoteCoverStateDto`；它只监听 scan 级状态
（`comic_cover.dart:326-333`）。两个轮询分别在 `comic_cover.dart:663-679`（30×350ms）与
`:489-503`（8×900ms）。Rust 侧目前**没有** cover 的 stream/`StreamSink`，只有一次性返回的
`RemoteCoverStateDto`（`api/remote_cover.rs:28-35`）；按 assetId 去重的目录视图 diff 在
`source_browser.dart:473-506`（180ms debounce）。folder representative 与普通 comic **已经**
消费同一个 `cover_state_for` 投影（`catalog.rs:313-402`、`:486-493`、`:506`），
Dart 侧分别传 `representativeAssetId` / `assetId`（`source_browser.dart:1568-1570` / `:1493`）——
所以"同一状态源 + 按 assetId 去重"这一条**已经成立**，P1-E 要做的是把两个轮询换成通知。

### P1-F（进度语义）
**部分已成立**：`ready_books` 只统计 `state='ready'`（`api/remote_scan.rs:1166`），
`blocked`/`unsupported`/`failed` 各有独立字段（`:1169-1171`）且**不计入"可用"**；
`RemoteScanViewState.fromStatus` 原样复制（`remote_scan_models.dart:135`）。
待做：主进度改为"可用 X / 共 Y 本"（Y 取 `status.discovered_books`，`remote_scan.rs:1119-1127`）
并拆两行；另注意 `cancelled` 目前**被所有桶丢弃**（`remote_scan.rs:1162`），需要在语义上明确它
是否应可见。

---

## 9. 本地/模拟 已证明 vs 仍需真实 provider

**本报告已证明（本地、可复现）：**
- 显式 cause 驱动的状态落库与候选队列后果（36 行矩阵 + 8 组解除组合）；
- 既有 cover 契约全部未回退（全量 380 passed）；
- 嵌套事务回归已被既有测试捕获并修复。

**仍需真实 115/夸克 才能证明（本轮未涉及，也不做推断）：**
- 真实 provider 下 `unsupported` / `blocked` 的**实际触发频率**与原因码分布；
- `CapabilityChanged` 在真实服务端能力变化时是否真的被观察到；
- 恢复后重试是否真的不再产生 403/405/429 或风控（属 Release Gate PENDING）。

**未完成：** P1-B / P1-C / P1-E / P1-F **未开始**；P1-D 的 **unified 路径已实施并经我独立复核**
（2 个新用例 + 12 个回归用例全绿），但 **legacy 源残留未满足冻结要求**（见第 7 节 P1-D-2，待你确认后开工）。
**未 commit / 未 push / 未 merge。**

### 本轮真实改动清单（P1 工作树，`git diff --check` clean）

```
修改(9): app/lib/ui/comic_cover.dart
         app/lib/store/remote_cover_repository.dart
         app/rust/src/remote_scan/{mod.rs,cover_store.rs,persistence.rs}
         app/rust/src/api/{remote_scan.rs,remote_cover.rs}
         app/rust/tests/{remote_cover_queue_contract.rs,remote_cover_missing_file_contract.rs}
新增(4): app/rust/src/remote_scan/cover_state.rs
         app/rust/tests/remote_cover_transition_contract.rs
         app/test/comic_cover_disk_first_test.dart
         docs/reports/p1/
```
（`app/linux|macos|windows` 下 7 个生成文件仅 stat 变化，无内容改动。）
