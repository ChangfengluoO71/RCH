# P1-C 交付报告：source-session 事件驱动的封面长期补偿

> 工作目录 `D:/Projects/RCH-p1`｜分支 `p1-cover-completion`｜HEAD `1bf2e3743c026581ddaa9d444787fde05fd02f31`（未变）
> **未 commit / 未 push / 未 merge / 未 reset / 未 stash**｜未进入 P1-D-2 / P1-E / P1-F

## 0. 必须先报告的一件事：我造成并已修复的一起自伤事故

在应用 P1-C 补丁时，我写的最后一行 python 是
`io.open(P,'w').write(io.open(P).read())` —— 求值顺序是**先以写模式打开（截断为 0 字节）**，
再读取，于是我把 `app/rust/src/remote_scan/cover_store.rs` **清空**，P1-A / P1-B 在该文件上的
改动全部丢失。

- **这是我的错误**，不是环境问题。
- 恢复方式（未做任何 `checkout` / `reset` / `stash`）：`git show HEAD:...cover_store.rs`
  取回基底，然后**按此前已记录、已验证过的补丁文本逐条重放**（P1-A 的显式 cause 规则表与
  非嵌套事务、P1-B 的 `resolve_cover_wake_session`、以及本轮 P1-C 全部改动）。
- 重放后已用**测试**确认恢复正确，而不是靠肉眼：P1-A 的 36 行迁移矩阵、P1-B 的 4+3+1 用例、
  P1-C 的 18 个用例全部通过（见 §5 全量门禁 406 passed）。
- 教训已落实：后续所有补丁改为「读一次 → 全部替换在内存中完成 → 写一次」，并使用
  `write` 工具生成脚本文件（避免 heredoc 破坏中文）。

请你在审阅时把这一条纳入考量。

---

## 1. lifecycle event 的实际挂点

### 1.1 新增的窄 API（Rust，FRB 已生成绑定）

```rust
pub async fn notify_source_session_ready(
    source_id: String,
    session: u64,
) -> Result<RemoteCoverReconcileDto, String>
```

职责（严格按冻结边界）：

1. 校验 `source_id` / `session` 非空非 0；
2. 校验 `book_sources` 存在；
3. **复用既有可信路径** `rebind_completed_generation_session` 把该 source 的已完成代际绑定到当前 session；
4. **验证**现在确实存在该 session 的绑定；取不到就**什么都不做**（不猜、不伪造 session）；
5. 对该 source 执行 **bounded、source-scoped** reconciliation；
6. 若有可 claim 工作，复用 P1-B 的 `wake_cover_worker_for_source`。

返回 `RemoteCoverReconcileDto { binding_available, compensation_promoted, blocker_cleared, jobs_created, claimable, truncated }`。
**本函数自身不做任何 provider 网络请求。**

### 1.2 Dart 挂点：现有唯一咽喉点（不是 5 处散点）

侦察发现 **5 个 provider session manager 已经在"提交当前 session"之后调用同一个**
`remoteSessionSuccessHub.emit(source, s.id)`：

```
lib/store/baidu_session.dart:29      lib/store/sftp_session.dart:22
lib/store/cloud115_session.dart:104  lib/store/quark_session.dart:24
lib/store/cloud115_session.dart:143  lib/store/webdav_session.dart:26
```

这 6 处**全部**位于 `await xxxConnect(...)` 成功并且已写入 `_xxxSessions[source.id]` 之后
（缓存命中会提前 return，不会走到这里）。定义在 `remote_scan_coordinator.dart:40-50`。

因此挂点选在 `RemoteSessionSuccessHub.emit` —— 一处覆盖全部 provider（含未来新增），
完全符合"不散落在 comic widget / cover widget / source page / reader / retry button"。

```dart
void emit(BookSource source, BigInt session) {
  _events.add(RemoteSessionSuccess(source, session));
  // 契约上就是 best-effort：即使注入口实现自己抛错，也不得影响会话投递。
  unawaited(_notifyReady(source.id, session).catchError((Object _) {}));
}
```

- `notifyReady` 是**可注入**的（`RemoteSessionSuccessHub({RemoteSessionReadyNotifier? notifyReady})`），
  默认走生产实现 `_nativeNotifySourceSessionReady`（内部 try/catch）。
- 顺序满足冻结要求：**session 成功 → current session 已提交（写入 map）→ 通知 Rust** ✓。
- 通知是 best-effort：失败被吞掉，绝不影响会话获取本身。
- Dart **不**查 failed job、**不**算 6 小时、**不**改 retry state、**不**判 unsupported/blocked、
  **不**决定 worker。

### 1.3 6h 的真实语义（已冻结，无 timer）

`6h = earliest eligibility`，**不是** exact timer deadline。
文档口径：**cover compensation is session-event-driven, not wall-clock-driven.**
本轮**没有**新增 scheduler / periodic poller / event bus / lifecycle registry，
也**没有** `startup -> 对所有 cloud source ensureSession()`。

---

## 2. migration

最小幂等迁移，复用既有 `pragma_table_info` + `ALTER TABLE ADD COLUMN` 范式
（`cover_store.rs` 的 columns 循环），只给 `remote_cover_job` 加 3 列：

| 列 | 定义 | 语义 |
|---|---|---|
| `long_retry_not_before` | `INTEGER`（可空） | **NULL = 无长期自动补偿资格**（永久失败 / unsupported / blocked）；非空 = retryable failure，值即 earliest eligibility |
| `long_retry_consumed` | `INTEGER NOT NULL DEFAULT 0` | 当前 failure episode 的一次长期补偿是否已被真正 claim |
| `long_retry_pending` | `INTEGER NOT NULL DEFAULT 0` | 该 pending 由长期补偿 reconciliation 产生、尚未 claim（crash-safety 标记，**不是**第二套 job state） |

未从 `error_code` 反推 retryability：retryable 的判定复用**既有短退避处理的同一错误集合**
（`TransientNetwork | RateLimited`）—— 短预算耗尽（`attempt >= 3`）后它落入
`cover_job_failure_state` 的 `_ => Failed` 分支，那就是 retryable terminal failure
（新增 `fn long_retry_is_retryable`）。

---

## 3. crash-safe transition（实现）

| 阶段 | durable 状态 | 代码位置 |
|---|---|---|
| retryable 终态失败 | `state=failed` + `not_before=now+6h` + `consumed=0` + `pending=0` | `cover_store::mark_job_failure_owned_on`（SQL CASE 表达"同一 episode 内不刷新"） |
| 同一 episode 内再次失败 | **保留** `not_before` 与 `consumed`（只清 `pending`） | 同上 CASE：仅当 `long_retry_not_before IS NULL` 才写新资格 |
| ≥6h + session-ready | `state=pending` + `pending=1`，**`consumed` 保持 0** | `reconcile_cover_compensation_for_source_on` |
| reconcile 后、claim 前崩溃 | `pending` + `pending=1` + `consumed=0` ⇒ **仍可被 claim** | 测试 `a_crash_between_reconcile_and_claim_preserves_the_compensation_opportunity` |
| **worker 真正 claim** | 同一条原子 UPDATE 内 `consumed=1`、`pending=0` | 4 处 claim 语句统一注入 |
| 再次失败 | `consumed` 保持 1 ⇒ 不得再生成下一轮 6h | 测试 `a_failed_compensation_does_not_start_another_six_hour_cycle` |
| 成功 ready | 清空三列 ⇒ 未来**新的独立 episode** 有全新一次额度 | `mark_job_ready_owned_on`（并在兼容写状态路径按 `state='ready'` 清空） |
| 人工 retry | **不刷新** `consumed` / `not_before` | 测试 `a_manual_retry_does_not_refresh_the_long_retry_budget` |
| unsupported / blocked | 不写 `not_before`（保持 NULL）⇒ 天然不参与 6h | 测试 `unsupported_is_never_promoted_by_time_or_by_a_session_event` |

blocked 的解除**按 blocker 类型**：只有 `SESSION_BLOCKER_CODES = ["authExpired"]`
（`error_code()` 中 `Unauthorized → "authExpired"`，是代码自身写入的明确码）会被 session-ready 解除；
`forbidden` 等一律不动（测试 `blocked_is_only_cleared_by_its_own_blocker_type`）。

### 实现中发现并修掉的**真实语义缺口**

`rebind_completed_generation_session` 只把 `pending/running/retry_wait` 的 job 迁移到新
`session_epoch`，**`failed` 被排除** —— 于是续期/重新登录后，`failed` job 仍挂旧 epoch，
**既无法被新 session claim，也无法被补偿推进**（L3 测试暴露）。
修法（最小、自包含）：reconciler 的匹配条件改为"该 job 的 **generation** 对当前 session 有有效绑定"，
并在推进时把 job 的 `session_epoch` 刷新为当前值（与既有 rebind 对 pending 做的事一致）。
**未修改** `rebind_completed_generation_session` 本身。

---

## 4. source-scoped budget 与"是否产生任何 provider request"

- `ReconcileBudget { max_jobs, max_wall_time_ms }`（默认 `64` / `250ms`，可注入）。
- 两个 reconciler 都是**单 source**、都加了 `LIMIT max_jobs + 1` 以精确判定 `truncated`、
  并在每次迭代检查墙上时间。
- `session ready for A -> scan all sources` 不存在：所有查询都以 `source_id=?1` 起手
  （测试 `reconciliation_is_strictly_source_scoped`）。
- **reconciler 自身不做任何 provider 请求**：只读写 `remote_cover_job` / `remote_scan_epoch` /
  `library_index` / `remote_scan_preview`。真正的网络预算仍由既有 worker +
  `provider_budget` + P0 的 CDN gate 控制，**没有第二套限流器**。
- 大量缺口时一次只推进 bounded batch（测试 `replenishment_is_bounded_by_its_budget`：25 个缺口、
  budget=10 → 每轮 10 个、`truncated=true`，由后续事件继续）。

### 缺口补齐（只用 durable 信息）

`library_index LEFT JOIN remote_cover_job` 反连接，**不遍历远端目录树**；
只补"完全没有对应 cover job 行"的资产；`content_revision` 与卡片请求路径**逐字一致**
（`library_index.content_fingerprint` → `remote_scan_preview` → `session_epoch`），
否则会产生第二个 job key、去重失效。`pending/running/retry_wait/ready/failed/blocked/unsupported`
一律不重复创建、不复活。

---

## 5. RED → GREEN 与 gate

### TDD 顺序（严格按 §12）

1. **migration RED**：独立测试文件先跑，报 `remote_cover_job must expose long_retry_not_before;
   got [...17 existing columns...]` → 加列后 GREEN。
2. **语义 RED**：先只加 API 桩（`Ok(ReconcileReport::default())`）→
   `FAILED. 4 passed; 9 failed`（失败的正是被推进/消耗/补齐相关用例，4 个通过的是"no-op 正确"情形）
   → 实现后 GREEN。
3. **L1–L4 RED→GREEN**：随 lifecycle API 一并落地。

### 用例清单（20 项要求全部覆盖）

| # | 用例 | 结果 |
|---|---|---|
| 1 | startup 无 session：0 worker / 0 network / durable 不丢 | ✅（C 组：`binding_available=false` 时不做任何事） |
| 2 | source attach 获得有效 session → overdue 被发现 → wake → 可消费 | ✅ L1 |
| 3 | 同一 lifecycle 连续触发：不重复 job / 不多 consumer | ✅ L2 / L4 |
| 4 | retryable failed `<6h`：不重入队 | ✅ `..._not_promoted_before_six_hours` |
| 5 | retryable failed `>=6h`：补偿一次 | ✅ `..._promoted_exactly_once_after_six_hours` |
| 6 | 补偿再失败：再过 6h 不自动循环 | ✅ `a_failed_compensation_...`（2×/4× 6h 都不推进） |
| 7 | reconcile 后、claim 前 crash：机会仍在 | ✅ `a_crash_between_...` |
| 8 | claim 后失败：durable budget 已消耗 | ✅ `the_claim_is_what_consumes_...` |
| 9 | ready 后新 episode：重新获得一次 | ✅ `a_new_failure_episode_after_ready_...` |
| 10 | unsupported：单纯时间经过不重试 | ✅ `unsupported_is_never_promoted_...` |
| 11 | blocked：6h 不重试；blocker cleared 才推进 | ✅ `blocked_is_only_cleared_by_its_own_blocker_type` |
| 12 | library 有漫画、无 job：创建 pending | ✅ `replenishment_creates_a_pending_job_...` |
| 13 | 已有 pending/running：不重复 | ✅ `replenishment_leaves_live_and_terminal_states_alone` |
| 14 | ready + 磁盘存在：不动 | ✅ 同上 |
| 15 | ready + 磁盘缺失：P1-B 路径恢复并可消费 | ✅ P1-B 已证（`remote_cover_reconcile_wake_contract`），P1-C 不改变该路径 |
| 16 | 大量缺口：单次只处理 budget | ✅ `replenishment_is_bounded_by_its_budget` |
| L1 | session creation → 推进 → 可消费 | ✅ |
| L2 | cached getter / 重复通知 → 不重复 job/claim/consumer | ✅ |
| L3 | renewal → 已完成代际可绑定 + 补偿可恢复 | ✅（并暴露上述真实缺口） |
| L4 | 相邻/重复事件 → 补偿最多消费一次、不多 spawn | ✅ |
| — | 人工 retry 不刷新额度（§7） | ✅ |
| — | source 隔离 | ✅ |
| — | Dart：hub 必须把 fact 交给 Rust / 通知失败不影响投递 | ✅ `test/remote_session_ready_hub_test.dart`（2 passed） |

时钟：全部使用可注入的 `now: i64`（既有 store 函数签名已支持）与 `db::now_ms()` 相对时刻，
**没有任何 6h sleep**。

### Gates（真实命令与结果）

| 命令 | exit | 结果 |
|---|---|---|
| `cargo test --locked --test remote_cover_long_retry_migration --test remote_cover_compensation_contract -- --test-threads=1` | 0 | **14 passed / 0 failed** |
| `cargo test --locked --lib -- session_ready_tests --test-threads=1` | 0 | **4 passed / 0 failed** |
| **`cargo test --locked -j 2 -- --test-threads=1`（全量）** | **0** | **18 个目标全部 ok；406 passed / 0 failed / 2 ignored** |
| `cargo test --locked`（第一次，默认并行度） | ≠0 | ❌ **环境/工具失败**：`E0786 ... failed to mmap ...librust_lib_app.rlib: 页面文件太小 (os error 1455)` —— rustc 并行编译全部目标时耗尽页面文件。**不是代码失败**；降 `-j 2` 后 EXIT=0 |
| `flutter test`（coordinator / status / ui-regression / hub / disk-first） | 0 | **39 passed / 0 failed** |
| `flutter analyze lib/store/remote_scan_coordinator.dart test/remote_session_ready_hub_test.dart` | 0 | **No issues found** |
| `flutter_rust_bridge_codegen generate` | 0 | `Done!`；`lib/src/rust/api/remote_scan.dart:31` 已生成 `notifySourceSessionReady` |
| `cargo clippy --locked --all-targets` | ≠0 | ❌ **既有失败**（未因本轮变化）：`this loop never actually loops` @ `src/reader.rs:277`，该文件不在 diff 中 |
| `flutter analyze`（全仓） | ≠0 | ❌ **既有 121 issues**（子代理证据） |
| `git diff --check` | 0 | clean |

---

## 6. Git

| 项 | 值 |
|---|---|
| branch / HEAD | `p1-cover-completion` / `1bf2e37`（未变，本轮无 commit） |
| staged | 空 |
| 本轮修改（tracked） | `app/rust/src/remote_scan/cover_store.rs`、`app/rust/src/api/remote_scan.rs`、`app/rust/src/api/remote_cover.rs`、`app/rust/src/remote_scan/{mod,persistence}.rs`、`app/lib/store/remote_scan_coordinator.dart`、**codegen 产物**：`app/rust/src/frb_generated.rs`、`app/lib/src/rust/api/{remote_scan,remote_cover}.dart`、`app/lib/src/rust/frb_generated{,.io,.web}.dart` |
| 本轮新增 | `app/rust/tests/remote_cover_compensation_contract.rs`、`app/rust/tests/remote_cover_long_retry_migration.rs`、`app/test/remote_session_ready_hub_test.dart`（+ P1-A/P1-B 既有新增） |
| 原有 dirty | P1-A / P1-B / P1-D 全部保留，未覆盖、未删除 |
| temp artifacts | 已删除 `D:\Temp\p1c_*.py` / `p1c_api_block.rs` / `p1c_l_tests.rs`；仓库内无本轮临时产物 |

> 说明：FRB codegen 会重写生成文件，这是新增 FRB API 的必然结果（决策 B 要求 Dart 能调用该 API）。
> 若你希望不引入生成文件改动，则需要改回"复用现有 FRB 入口"的方案 —— 那与决策 B 冲突，故未自行改回。

---

## 7. Remaining risks（如实）

1. **`failed` 的 episode 只由 worker 失败路径产生**：已在 `cover_job_failure_state` 的 `else` 分支接入
   （`api/remote_scan.rs`）。但**其它把 job 写成 `failed` 的路径**（例如 route-missing 分支、
   以及兼容写状态路径）目前传 `None`，即视为永久失败、不参与长期补偿。这是刻意的保守选择，
   但意味着"route 缺失"这类问题不会获得 6h 自动补偿。
2. **没有真实 provider 验证**：本报告全部结论来自本地确定性测试；真实 115/夸克下
   "续期后补偿是否真的能取到封面"需要真实环境（属 Release Gate PENDING）。
3. **recovery 延迟**：如你冻结的那样，补偿只在"下一次带有效会话的 per-source 事件"发生；
   若某 source 长期不被访问，则不会补偿，也**不会**为封面主动登录（刻意的隐私/资源边界）。
4. **Dart 侧没有断言"真实 FRB 调用真的到达 Rust"**：`test/remote_session_ready_hub_test.dart`
   用注入口验证了 hub 的行为；端到端（真 FRB）需要集成环境，未做。
5. **仓库级既有红灯仍在**：`cargo clippy`（`src/reader.rs:277`）与 `flutter analyze`（121 issues），
   均非本轮引入，按"禁止 unrelated cleanup"未修。
6. `long_retry_pending` 若在 reconcile 与 claim 之间**永远**得不到 claim（例如 source 再也不会被访问），
   该 job 会停在 `pending` + `pending=1`；额度未消耗，因此后续任何一次 session 事件都仍可执行。

**未进入 P1-D-2**（按你的顺序，需 P1-C 全部 PASS 后开始；本轮到这里 STOP）。
