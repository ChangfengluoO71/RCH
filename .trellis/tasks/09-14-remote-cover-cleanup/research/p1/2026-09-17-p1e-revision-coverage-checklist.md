# P1-E 实施：durable revision coverage 内部 checklist

> `D:/Projects/RCH-p1`｜分支 `p1-cover-completion`｜HEAD `1bf2e3743c026581ddaa9d444787fde05fd02f31`（未变）
> **本轮对生产代码零修改**｜未 commit / push / merge / reset / stash

## 结论（先说）

> ① **`bump_view_revision_on` 的 coverage 不完整**：它只有 **2 个调用者，全在 listing/scan 持久化路径**
> （`persistence.rs:719`、`:1069`），**9 个 cover-job 状态迁移函数一个都没有 bump**。
> ⇒ 必须按你 §1 在对应 durable transition 的**同一事务内**补 bump。
>
> ② **仓库不存在任何 Rust→Dart stream pattern**（`grep -rn StreamSink src/` 为空，Dart 侧同样为空）
> ⇒ 按你 §3 必须实现一个**窄的 cover revision stream**。
>
> ③ 这**不构成** `P1_E_NOTIFICATION_ARCHITECTURE_EXPANSION`：按你 §22，只需要"增加现有 FRB 通道的一个
> 窄 source-level revision + 一个 Dart 消费层"，不需要长期后台连接 / 通用 event bus / 重做 FRB transport。

---

# 1. `bump_view_revision_on` coverage checklist（你的 §1）

**定义**：`src/remote_scan/cover_store.rs:324`
（`INSERT INTO remote_view_revision(source_id,revision,listing_generation,updated_at) …`）

**全部生产调用者（2 处）**：

| 调用点 | 所在路径 | 语义 |
|---|---|---|
| `persistence.rs:719` | listing / scan 持久化 | scan 侧 view 变化 |
| `persistence.rs:1069` | 同上（另一条持久化函数，事务内） | 同上 |

**九个必需 transition 的覆盖映射**：

| # | transition | durable 变更函数（`cover_store.rs`） | 当前是否 bump revision |
|---|---|---|---|
| 1 | job create → `pending` | `upsert_job_on`（`:582`） | ❌ **否** |
| 2 | claim → `running` | `claim_next_job_on`（`:1086`）/ `..._for_source_on`（`:1131`）/ `..._for_source_session_on`（`:1181`，生产 claim 路径） | ❌ **否** |
| 3 | → `ready` | `mark_job_ready_owned_on`（`:416`）、`mark_job_state_on`（`:357`）、`mark_job_state_owned_on`（`:384`） | ❌ **否** |
| 4 | → `retry_wait` | `mark_job_failure_owned_on`（`:745`）→ `cover_job_failure_state` 的短退避分支 | ❌ **否** |
| 5 | → `failed` | `mark_job_failure_owned_on`（`:745`）terminal 分支 | ❌ **否** |
| 6 | → `unsupported` | `mark_job_state_*` / upsert 的 capability 分支 | ❌ **否** |
| 7 | → `blocked` | `mark_job_state_*`（blocker 写入） | ❌ **否** |
| 8 | ready-missing reconcile → `pending` | P1-B 的对账 UPDATE（`state='pending'`） | ❌ **否** |
| 9 | long-compensation reconcile → `pending` | `reconcile_cover_compensation_for_source_on`（P1-C） | ❌ **否** |
| — | `retry_wait` 到期后再次执行 | 经 #2 的 claim / `lease_job_on`（`:1250`） | ❌ **否** |

**⇒ 9/9 全部缺失。** 现有 revision 只反映 scan/listing 变化，**不反映 cover job 变化**
（这也解释了为什么现有 500ms scan poll 停止后就再也没有任何覆盖通知 —— 与 emission coverage 的 E-B 判定一致）。

## 1.1 补齐方案（按你 §1 的原子性硬要求）

对上述每个 durable 变更函数，在**同一事务内**、**state mutation 之后、COMMIT 之前**调用
`bump_view_revision_on(conn_or_tx, source_id, generation, now)`：

```
BEGIN TX
  mutate cover state
  bump durable view revision      ← 必须同事务
COMMIT
  ↓
emit CoverRevisionEvent           ← 事务提交成功之后
```

* **禁止** `commit state → 后补 revision`（crash 会永久漏 revision）。
* **禁止** `bump revision → state transaction 失败`（已与上面同事务解决）。
* **不要在每个调用者散点 emit**；优先放在最靠近 durable transition 的公共生产路径
  （`cover_store.rs` 的这些函数本身就是公共收敛点）。
* 事务内被多路径调用的 helper 不提前发事件；**在成功提交后的外层统一 emit**（你 §4）。

**实现注意**：这些函数有 `_on(conn, …)` 与 `_owned_on(…)` 两种（后者可能自持事务，见 P1-A 的
`is_autocommit`/`owned_tx` 处理）。补 bump 时必须沿用同一 `conn`/`tx` 句柄，不能另开事务
（否则会重现 P1-A 遇到过的 `cannot start a transaction within a transaction`）。

# 2. Wake-up transport（你的 §3/§4）

`grep -rn "StreamSink" src/` → **零命中**；Dart 侧同样无 stream pattern。

⇒ 按 §3 实现**一个**窄流（唯一用途）：

```
CoverRevisionEvent {
    source_id,
    view_revision,     // version token，不是 authoritative job state
    asset_id?,         // transition 天然知道时才带；不知道不反查
}
```

**不得**携带 `pending/ready/failed/error_code` 等状态。**不得**引入 generic event bus / scheduler /
per-asset notifier map / ref-count registry / background polling。

**生命周期（§2/§6）**：应用级只建**一个** stream subscription

```
Rust cover revision stream
        ↓
RemoteScanCoordinator（唯一消费层；widget 不直接持 Rust sink）
        ↓
sourceId → ValueNotifier<int revision>   // 只用于 wake-up，不保存 state
        ↓
mounted ComicCover / 进度聚合
```

**emission 顺序（§4）**：`BEGIN TX → mutate → bump revision → COMMIT → emit`。
send 失败时：DB mutation 不回滚、worker 不失败、job state 不变 —— UI wake-up 是 best-effort。
（FRB v2 的 Rust→Dart `StreamSink` 正是"一次建立、持续发送"的窄流形态。）

# 3. Missed-event recovery（你的 §5，不用 polling）

在以下时机**各读一次** `remote_view_revision(source_id)`：

* coordinator 首次开始观察该 source；
* stream reconnect / resubscribe；
* source/session 重新 attach；
* widget/source context 切换到此前未观察的 source。

若 durable revision > Dart 已知 revision → 触发**一次** local refresh（cards 重读 durable asset state
＋ view aggregate 重读），然后记录新 revision。**禁止**用 `Timer.periodic → remote_view_revision()`
或拿现有 500ms scan poll "顺便发现" revision（§2）。

# 4. 只读 asset-state API（你的 §7）

给 private `catalog.rs:313 cover_state_for(...)` 增加**最薄 FRB read-only wrapper**
（命名随仓库风格，语义 `remote_cover_state(source_id, asset_id) -> Option<RemoteCoverStateDto>`）：
read only / no enqueue / no wake / no session / no provider / no mutation；`None` = no job；
**优先复用现有 `RemoteCoverStateDto`**。

# 5. 本轮状态（如实）

| # | 项 | 状态 |
|---|---|---|
| 1 | `bump_view_revision_on` coverage checklist（§1/§25.1） | ✅ **本轮完成**（9/9 缺失，结论见 §1） |
| 2 | 流 pattern 存在性核对（§3） | ✅ **本轮完成**（不存在，需新增窄流） |
| 3 | 在 9 个 transition 同事务内补 bump | ❌ 未开始 |
| 4 | 窄 cover revision stream（Rust `StreamSink` + emit after commit） | ❌ 未开始 |
| 5 | 只读 asset-state FRB wrapper | ❌ 未开始 |
| 6 | Dart coordinator bridge（`ValueNotifier<int>` + missed-event recovery） | ❌ 未开始 |
| 7 | 删除 `30×350ms`（`comic_cover.dart:852-868`）与 `8×900ms`（`:583-585`） | ❌ 未开始 |
| 8 | E RED（E-1~E-6）与 state→UI matrix | ❌ 未开始 |
| 9 | F：`is_cover_material_available` 纯函数 + `availableBooks/waitingBooks/otherBooks` + no-job/stale-ready 归桶 + F RED（F-1~F-11） | ❌ 未开始 |
| 10 | codegen / generated diff audit / P1 总门禁 | ❌ 未开始 |

**本轮零生产代码修改** ⇒ `P1_D2_PASS` 证据不受影响；**`P1_PASS` 未声明**；未进入 P2。

# 6. 门禁与完整性

| 项 | 结果 |
|---|---|
| 生产代码修改 | **零**（仅只读 grep） |
| `git diff --check` / staged | clean / **0** |
| `P1_RECOVERY_INTEGRITY_PASS` | 未受影响（零文件改动） |
| branch / HEAD | `p1-cover-completion` / `1bf2e37`（未变） |
| 临时产物 | 无 |
| baseline exception（保留，未修） | `cargo clippy` `src/reader.rs:277`；全仓 `flutter analyze` 121 issues；完整 `flutter test` 的 6 个既有编译/装配失败 |

## Remaining risks

1. **9 个 transition 的 bump 必须在同一事务内**；`_on` 与 `_owned_on` 两种签名必须沿用同一 tx 句柄，
   否则会重现 P1-A 的 `cannot start a transaction within a transaction`（该问题在 P1-A 已用
   `is_autocommit`/`owned_tx` 解决，必须沿用同一模式）。
2. 新增 Rust→Dart stream 是本仓库**首个**此类通道 ⇒ codegen 后需审核 generated diff，
   并确认 web 平台（`frb_generated.web.dart`）无新增异常。
3. 只读 asset-state wrapper 必须走 `cover_state_for` 的**纯读**路径，绝不能复用 `remote_cover_request`
   （会 enqueue）。
4. F 的 `availableBooks` 必须复用**纯** validity 判断（file/cache check only, no DB mutation）；
   若 P1-B 现有 helper 会做 `ready → pending`，必须先抽出纯函数，否则统计会改写 DB。
5. `discoveredBooks = max(current-generation indexed eligible, staged cover tasks)`——
   分母**不等于** `library_index` 行数（扫描中 staged 可能更大），差额自然归 `noJobBooks → waitingBooks`。
6. 115-web / Quark 的 raw-key `present→absent` 仍 known unrecoverable（P1-D2 冻结）。
7. **Release Gate PENDING**（真实 115/Quark 校验、P0 S1–S5、无新增 403/405/429、CDN rate/burst/in-flight
   实测、20/100/300 MiB Range 证据）—— P1 完成 ≠ 可发布。
