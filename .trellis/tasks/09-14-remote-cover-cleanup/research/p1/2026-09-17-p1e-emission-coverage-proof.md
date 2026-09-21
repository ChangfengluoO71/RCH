# P1-E 实施第 1 步：现有 per-source notifier 的 emission coverage 证明

> `D:/Projects/RCH-p1`｜分支 `p1-cover-completion`｜HEAD `1bf2e3743c026581ddaa9d444787fde05fd02f31`（未变）
> **本轮对生产代码零修改**｜未 commit / push / merge / reset / stash

## 判定

> ## **Case E-B —— 覆盖不完整**
>
> 现有 `statusFor(sourceId)` / `viewStateFor(sourceId)` **不足以**承载 asset 级 durable state 消费：
> **scan 进入终态后 monitor 会停止，后台 worker 的 cover job 转变不会再更新任何 per-source notifier。**
> 因此需要一个**窄的 source-scoped cover revision notification**（见 §3）。
>
> 按你的 §22，这**不构成** `P1_E_NOTIFICATION_ARCHITECTURE_EXPANSION` 型 STOP ——
> 需要的只是"在现有 coordinator refresh path 增加一次调用 / 增加窄 source-level revision"，
> 你已明确允许。

---

# 1. 唯一发射源（production emission sites）

`_statuses` / `_viewStates` 的写入**只有**这三处（`remote_scan_coordinator.dart`）：

| 发射点 | 位置 | 触发条件 |
|---|---|---|
| `_setStatus(sourceId, status)` | `:482-490` | 写入 `_statuses` **并**用 `RemoteScanViewState.fromStatus(...)` 写 `_viewStates` |
| `_setControlState(sourceId, state)` | `:475-480` | 控制态变化（pause/resume/cancel）→ 转调 `_setStatus` |
| `_onSettingsChanged()` | `:492-508` | 设置变化时对**已存在的** status 重算 `viewState` |

`_setStatus` 的调用者只有：

1. **`_pollProgress`**（`:453-468`）—— 由 `_startProgressMonitor` 建立的
   **`Timer.periodic(progressPollInterval = 500ms)`**（`:437-446`，`:100`/:`148`）驱动；
   每次 poll 后 `if (!_shouldMonitorProgress(status)) _stopProgressMonitor(sourceId);`（`:461-463`）。
2. scan 启动 / 结束路径（`_nativeStart*` 的 future 完成）。
3. `_onSettingsChanged`（`:492-508`）。

# 2. 为什么这是 E-B（覆盖不完整）

覆盖矩阵（cover job 转变 × 现有 notifier 是否刷新）：

| cover job 转变 | 发生时机 | 现有 notifier 是否刷新 | 说明 |
|---|---|---|---|
| `pending → running` | scan 期间或 scan 之后 | ⚠️ 仅在 monitor 存活时 | 依赖 500ms poll |
| `running → ready` | **常在 scan 终态之后**（worker 继续 drain 队列） | ❌ **不刷新** | monitor 已随 `_shouldMonitorProgress == false` 停止 |
| `running → failed / retry_wait` | 同上 | ❌ 不刷新 | 同上 |
| worker 从 `retry_wait` 到期后再次执行 | 6h 级/退避级，可能在任意时刻 | ❌ 不刷新 | 无 monitor |
| `ready → pending`（P1-B 字节缺失对账） | 任意时刻（卡片读路径触发） | ❌ 不刷新 | 该路径不经过 coordinator |
| scan 未在运行（用户只是浏览书架） | 任意时刻 | ❌ 不刷新 | `_startProgressMonitor` 未被调用 |

**结论**：把 `statusFor`/`viewStateFor` 当作 asset 状态的唯一来源，会在"scan 已结束但封面仍在后台补齐"这一
**最常见**场景下失去通知 —— 这恰恰是 P1-E 要解决的场景。

**附带发现（影响 E-9/F-9）**：现有 `viewStateFor` 的刷新机制本身就是 **500ms `Timer.periodic` 轮询**
（`:439`）。因此"复用现有 notifier"**不能**满足你 §4 的"不得保留等价 timer"与 E-9/F-9 的
"no polling" 断言 —— 复用它会把轮询留在链路上。

# 3. 最小设计（按你的 §1 Case E-B 允许范围）

```
Rust worker 的 cover job 状态迁移点（claim→running / →ready / →failed / →retry_wait / →pending）
        ↓  只发"事实"，不带 authoritative state
source-scoped cover revision（source_id [+ 可选 asset_id] 用于减少无关刷新）
        ↓  复用现有 coordinator refresh path（不新建 event bus / 不建永久 asset map）
coordinator 重新读取该 source 的聚合（既有 remote_scan_status / refresh_status_counts）
        ├─ E：可见 ComicCover 收到事件 → 用**自己的 canonical assetId** 重读 durable state → render
        └─ F：viewStateFor(sourceId) 聚合随之刷新（E/F 共享同一条通知）
```

- payload **不带** lifecycle state（E3）：consumer 收到后**仍重读 durable state**；
  事件丢失不破坏正确性，下一次 refresh 仍从 durable state 恢复。
- **优先级**（按你 §1）：如果能直接让现有 `viewStateFor(sourceId)` 在 transition 后 refresh，
  **就那样做**，优于新增第二种 notifier；若能复用既有 FRB 入口/事件通道的一个 payload，优先复用。
- **不新增**：`Map<assetId, ValueNotifier>`、永久 per-asset registry、ref-count subscription cache、新 event bus（§0）。
- **E4 按你 §2 修订后的验收**：folder 与 comic 对同一 asset 使用**同一 durable identity + 同一 source-level
  notification**；允许各自做一次纯本地 durable-state read；**不要求** per-asset 共享缓存。
  必须保证：不重复 `requestCover`、不重复建 job、不重复 provider request、无各自 polling、状态最终一致。
- **E-10 按修订版**：同一 asset 的两个 consumer 在同一 source notification 后读到**一致**的 durable state，
  且无重复 enqueue/network 副作用（而非"底层 notifier 只能建立一次"）。
- **性能边界（你的 §10）**：先保持"只有 mounted/visible widget 响应"；一个 source 下 N 个不可见漫画
  没有 listener，因此不产生 N 次读。若实测出现 visible-card DB storm，再单独评估 batch asset-state query。

# 4. 卡片的推荐状态流（你的 §3，冻结）

```
readCover(asset)
  ├─ HIT → display
  └─ MISS → requestCover(asset) 仅此一次
              → 收到 durable job state
              → 订阅/复用 source notification
              → 后续 notification → 重读 durable state(asset)
                   ├─ ready → readCover 一次 → display
                   └─ 其它 → 按 §6 matrix 渲染
```

`requestCover` **只负责首次确保任务存在**；此后状态变化**禁止**通过再 request / timer / polling 获取。
job 已存在时由既有 dedup 负责。人工 retry 属另一条显式操作，不受此限。

# 5. 待办（未实施）

| # | 项 | 状态 |
|---|---|---|
| 1 | emission coverage 证明（你的 §21.1） | ✅ **本轮完成**（Case E-B 判定） |
| 2 | E RED（E-1…E-11） | ❌ 未开始 |
| 3 | 窄 source-scoped cover revision notification（Rust 发射点 + 复用 coordinator refresh path） | ❌ 未开始 |
| 4 | 删除 `30 × 350ms`（`comic_cover.dart:852-868`）与 `8 × 900ms`（`:583-585`） | ❌ 未开始 |
| 5 | asset durable-state read API | ❓ **尚未核对是否已存在**（你的 §5 要求优先复用现有 `RemoteCoverStateDto` 读取；`requestCover` 会返回它但**会 enqueue**，因此需要一个只读变体——若不存在则新增一个 DB-read-only API） |
| 6 | E state→UI matrix（§6）与 ready→readCover + P1-B reconcile 语义（§7） | ❌ 未开始 |
| 7 | subscription lifecycle + E-11（§8） | ❌ 未开始 |
| 8 | F 的 `refresh_status_counts` 语义审计（`readyBooks` 是否已验证字节存在 / `discoveredBooks` 是否 durable eligible total / no-job 落哪个桶） | ❌ 未开始 |
| 9 | F 语义校准 + F RED | ❌ 未开始 |
| 10 | P1 总门禁 | ❌ 未开始 |

**本轮未改一行生产代码**，因此 `P1_D2_PASS` 的既有证据不受影响；P1-E/F 未完成，**`P1_PASS` 未声明**。

# 6. 门禁与完整性

| 项 | 结果 |
|---|---|
| 生产代码修改 | **零**（仅只读 grep/read） |
| `git diff --check` / staged | clean / **0** |
| `P1_RECOVERY_INTEGRITY_PASS` | 未受影响（零文件改动） |
| branch / HEAD | `p1-cover-completion` / `1bf2e37`（未变） |
| 临时产物 | 无 |
| baseline exception（保留，未修） | `cargo clippy` `src/reader.rs:277`；全仓 `flutter analyze` 121 issues；完整 `flutter test` 的 6 个既有编译/装配失败 |

## Remaining risks

1. **E 的通知发射点必须落在 Rust worker 的 job 状态迁移处**，否则 E-B 的缺口依旧（这是 E 的核心工作量）。
2. 现有 `viewStateFor` 由 **500ms `Timer.periodic`** 驱动；若 E/F 要满足"no polling"，必须让
   transition 通知成为主路径，并避免继续依赖该 timer 作为状态同步手段（timer 是否保留需与 §4 的
   "不得保留等价 timer" 对齐后再定）。
3. F 的 `available` 语义取决于 `readyBooks` 是否已验证 cover 字节存在（§11/§12 的审计尚未做）；
   若未验证，需要 Rust 只读聚合新增 `availableBooks`（复用 P1-B 的 validity helper，统计层不改 DB）。
4. `ready but bytes missing` 的归桶（§13）：优先入 `waiting`，并在报告显式写明
   "waiting includes stale-ready items whose cover bytes are missing"；若现有聚合无法无歧义加入，
   则允许新增 `staleReadyBooks` 并用 `other` 解释不变量。
5. 115-web / Quark 的 raw-key `present→absent` 仍为 known unrecoverable（P1-D2 冻结）。
6. **Release Gate PENDING**（真实 115/Quark 校验、P0 S1–S5、无新增 403/405/429、CDN rate/burst/in-flight
   实测、20/100/300 MiB Range 证据）—— 不得因 P1 完成而误报可发布。
