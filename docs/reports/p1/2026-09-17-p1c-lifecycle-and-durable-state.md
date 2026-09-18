# P1-C 冻结报告：生命周期挂点 + durable-state matrix

> 工作目录 `D:/Projects/RCH-p1`｜分支 `p1-cover-completion`｜HEAD `1bf2e3743c026581ddaa9d444787fde05fd02f31`（未变）
> **未 commit / 未 push / 未 merge / 未 reset / 未 stash**｜P1-E / P1-F 未开始

## 状态摘要（先说清楚）

你要求"**先**冻结生命周期挂点、**先**画出 durable-state matrix，实现前明确区分两个重试维度"。
本轮完成了这两个**前置步骤**（均带 `file:line` 证据），并冻结了 schema 决策与 crash-safety 设计。

**但 P1-C 的实现与 16 项 RED 测试本轮未开始** —— 没有写入任何一行 reconciler 代码、没有做迁移、
没有写测试。原因是本轮预算在完成前置调研后耗尽。我**不把设计文档当作实现**，因此
**P1-C 状态 = NOT IMPLEMENTED**，P1-D-2 = **NOT STARTED**。

---

# 1. 生命周期挂点（冻结结果）

## 1.1 你指定的"主触发点"在本仓库**不存在**

| 你要求的语义 | 本仓库实际情况 | 证据 |
|---|---|---|
| Rust 侧 source attach | **不存在** | Dart `grep attachSource\|onSourceAttach` → 无 |
| Rust 侧 session restore | **不存在** | Dart `grep restoreSession\|ensureSession` → 无 |
| Rust 侧 login/auth 恢复事件 | **不存在**（无事件，只有**逐调用**的会话校验） | 见 1.2 |

Dart 侧同样没有 attach/restore/resume 概念：`lib/store/*_session.dart` **不发起任何 Rust 调用**
（`grep "rust\."` 无命中），只是**按书源 id 缓存运行时会话**，例如
`lib/store/cloud115_session.dart`：`Map<String, BigInt> _cloud115OpenSessions` /
`_cloud115CookieSessions` + `*SessionFlights` 单飞 + Cookie 过期自动续期重试
（`_is115ExpiredError` / `_refreshing115Cookie`）。Dart **完全不持有 `session_epoch`**
（`grep sessionEpoch` 无命中；`RemoteScanJobDto` 只有 `job_id/source_id/status/mode/generation`，
`api/remote_scan.rs:25-31`）。

## 1.2 但**存在**你明确许可的等价事件（因此不触发 `P1_C_ARCHITECTURE_EXPANSION`）

你在"额外事件可以复用同一 reconciler"里列了 **`scan/import complete`**，而它正是本仓库里
"有效 source session 落库"的**唯一**时点：

| 事件 | 现有入口 | 语义 |
|---|---|---|
| **scan 启动并绑定有效会话** | `persistence::bind_scan_epoch`（`persistence.rs:417`）← 生产唯一调用者 `api/remote_scan.rs:1408` | 把 `(source_fingerprint, root_path, session)` 派生的 `session_epoch` + `session_token` 落库 —— **"有效 source session 建立"的持久化时刻** |
| **新会话接管已完成 generation**（= 重新登录 / 会话恢复的现有机制） | `persistence::rebind_completed_generation_session` ← 生产调用者仅 `api/remote_cover.rs:255`（cover 请求）与 `:519`（显式 retry） | generation 已是 `complete` 时允许新 session 重绑；`running/failed` 代际仍拒绝（过期会话不能写进新代际） |
| **会话校验成功的 per-source 入口** | `remote_cover_request`（`api/remote_cover.rs:255`）、`remote_cover_retry`（`:519`） | 两者都先校验/重绑会话，失败即返回 `"登录状态已失效"` |
| 显式人工 retry | `api/remote_cover.rs:512-546` | 已有 |

**结论**：不需要新增常驻 scheduler、不需要全局 polling loop、不需要新 lifecycle registry、
不需要事件总线、更不需要把 retry/session lifecycle 下沉到 Flutter。**未触发任何 STOP 条件。**

## 1.3 冻结的挂点（建议；实现时按此接线）

主挂点选**会话校验成功的 per-source 入口**，并以 scan 绑定作为补充：

1. `remote_cover_request`（会话校验通过之后）→ `reconcile_cover_work_for_source(source_id, now, budget)`
   —— 这是"用户/应用带着有效会话触达该 source"的最常见事件，覆盖 **登录恢复 / 会话恢复 /
   network 重新开启 / cooldown 之后的下一次交互**。
2. `remote_cover_retry`（显式 retry 之后）—— 已在上面这条路径上。
3. `bind_scan_epoch` 成功之后（scan 完成）—— 你许可的 `scan/import complete`。

### startup 规则（严格按你的语义）

`startup` **不**做任何 source 级扫描。本仓库没有 startup session hook，因此 startup 时：
不启动 worker、不访问 provider、durable work 保留；等到上述**任一会话成功事件**发生时
自然补齐。**不实现** `startup -> 全库扫描所有 source / failed jobs`。

### ⚠️ 必须报告的限制（请你确认是否可接受）

由于不存在 attach/restore hook，**overdue 6h 补偿只有在"下一次带有效会话的 per-source 事件"
（打开该源的卡片 / 显式 retry / 触发一次扫描）时才会执行**。不存在"应用一启动就自动补偿"。
这是本仓库现有事件模型的直接结果，而不是我的取舍。若你要求更早/更主动的补偿，
那就需要新增一个 source-session 生命周期事件 —— 那属于你说的架构决策，**我没有自建**。

---

# 2. durable-state matrix（冻结结果）

## 2.1 两个重试维度必须严格分离

| 维度 | 现有字段 | 语义 | 冻结 |
|---|---|---|---|
| **A. 单次任务执行中的短退避** | `attempt` + `next_attempt_at` | `cover_job_failure_state`（`api/remote_scan.rs:153-186`）：`TransientNetwork\|RateLimited` **仅当 `attempt < 3`** → `retry_wait`（delay ≤ 15 min）；`RangeUnavailable\|Unsupported` → `unsupported`；`Unauthorized\|Forbidden` → `blocked`；其余 → `failed` | **原样保留，绝不与 B 共用计数器** |
| **B. 长期补偿** | **当前不存在** | `failed -> >=6h -> 最多一次自动补偿` | 新增，见 2.3 |

## 2.2 schema 审计：**不足**，需要最小附加列迁移

`remote_cover_job` 现有列（`cover_store.rs:35-53`）：
`job_key, source_id, asset_id, content_revision, selection_revision, profile, state, demand_kind,
priority, attempt, next_attempt_at, lease_owner, lease_until, generation, session_epoch,
error_code, updated_at`。

| 需要表达 | 现有字段能否表达 | 结论 |
|---|---|---|
| failure 是否 retryable | 只有 `error_code` 字符串 | ❌ **不足**，且你明确禁止"从错误字符串临时反推 retryability" |
| failure episode 身份 | 无 | ❌ 不足 |
| long compensation 是否已使用 | 无 | ❌ 不足（`attempt` 是 A 维度，不可共用） |
| earliest eligible time | `next_attempt_at` 属 A 维度 | ❌ 不可共用 |

## 2.3 冻结的最小迁移（3 个附加列，幂等）

复用项目既有幂等迁移范式（`cover_store.rs:250-259`：`pragma_table_info` 检查 + `ALTER TABLE ADD COLUMN`）：

| 列 | 类型/默认 | 语义 |
|---|---|---|
| `long_retry_not_before` | `INTEGER`（可空） | **NULL = 永久失败，不参与长期补偿**；非空 = retryable，最早可补偿时间 = `failed_at + 6h`。用 NULL/非 NULL **表达 retryability**，无需额外 flag、不碰 `error_code` |
| `long_retry_consumed` | `INTEGER NOT NULL DEFAULT 0` | 本 failure episode 的长期补偿额度是否已消耗 |
| `long_retry_pending` | `INTEGER NOT NULL DEFAULT 0` | reconciler 已把该 job 重新入队、但**尚未被 claim**；由 claim 原子消耗 |

## 2.4 crash-safety 语义（这是你要的"不要只发现 overdue 就烧掉额度"）

| 阶段 | durable 状态 | 崩溃后行为 |
|---|---|---|
| 形成 failure episode | `failed` + `not_before = failed_at+6h` + `consumed=0` + `pending=0` | — |
| lifecycle 事件 <6h | 不变 | 不重入队、不触网、不消耗额度 |
| lifecycle 事件 >=6h 且全部前置条件满足（retryable / 未消耗 / 允许联网 / 会话有效 / 无 cooldown / budget 允许） | `state='pending'` + `pending=1`，**`consumed` 仍为 0** | **reconcile 后、claim 前崩溃 → 下次仍可执行**（job 已 pending 且 `pending=1`） |
| worker **claim** | 在同一条 claim UPDATE 中原子置 `consumed=1`、`pending=0` | claim 后失败 → `consumed=1` ⇒ **永不再次自动补偿**，杜绝"每 6h 无限循环" |
| 成功 → `ready` | 清空三个列 | 之后形成**新的独立 failure episode** 时可获得**全新的一次**补偿额度（不污染后续 episode） |

`unsupported` / `blocked` **不**写 `long_retry_not_before`（保持 NULL）⇒ 天然不参与 6h 补偿。

---

# 3. 不同终态的严格分离（冻结）

| 终态 | 是否走 6h 补偿 | 允许的重新候选条件 | 现有证据 |
|---|---|---|---|
| `failed`（retryable） | ✅ 最多一次 | 上面的 durable 状态机 | `cover_job_failure_state` 的 `else` 分支需按 retryable/permanent 分流 |
| `failed`（permanent） | ❌ | 仅 `ManualRetry` / `FullRescan`（P1-A 矩阵） | P1-A `cover_state::resolve_upsert_state` |
| `unsupported` | ❌ **禁止按时间重入队** | 仅 `CapabilityChanged` / `ManualRetry` | P1-A 矩阵已钉住 |
| `blocked` | ❌ **不走 failed 的 6h timer** | 仅 `CauseCleared`（登录/会话/网络/cooldown/权限恢复）/ `ManualRetry` | P1-A 矩阵已钉住 |

**本轮没有可靠的 capability-change signal**（未发现 provider/archive capability 版本变更事件），
因此按你的要求 `unsupported` 保持 durable、**不猜**。

---

# 4. 补齐 missing cover（设计，未实现）

缺口来源只用 durable 信息，**不重新遍历远端目录树**（`library_index` ⟕ `remote_cover_job`）：

```sql
SELECT li.source_id, li.id FROM library_index li
LEFT JOIN remote_cover_job j
  ON j.source_id=li.source_id AND j.asset_id=li.id
 AND j.selection_revision='default' AND j.profile='340x480@1'
WHERE li.source_id=?1 AND li.deleted=0
  AND li.entry_type IN ('file','dir')          -- 与 refresh_status_counts 的口径一致
  AND (j.job_key IS NULL OR j.state<>'ready')
```

去重/不动规则（按你的要求）：asset/source 去重；已有 `pending`/`running`/`retry_wait` **不重复创建**；
`ready` 且文件存在**不动**；`ready` 但文件缺失走 **P1-B 已验证的 reconcile 路径**；
`unsupported`/`blocked`/`failed` 遵守上表。

## Budget 冻结

- `max_jobs`（每次 reconciliation 最多推进的 job 数）
- `max_wall_time`
- `max_requests` / `max_bytes` 若低成本可得再加

**reconciler 自身只操作 durable state / enqueue，不直接执行网络**；真正的 provider 请求预算继续由
现有 worker + `provider_budget`（`remote_scan/provider_budget.rs:36-82`）+ P0 的 CDN gate 控制，
**不在 reconciler 里造第二套网络限流器**。一次存在大量缺口时只推进 bounded batch，
由既有 wake/drain 机制继续处理，绝不一次制造上千个网络任务。

---

# 5. P1-C RED 测试清单（**全部未写**）

| # | 行为 | 状态 |
|---|---|---|
| 1 | startup 无 session：0 worker / 0 network / durable 不丢 | ❌ 未写 |
| 2 | source attach 获得有效 session：overdue 被发现 → wake → 可消费 | ❌ 未写 |
| 3 | 同一 lifecycle 连续触发：不重复 job、不多 consumer | ❌ 未写 |
| 4 | retryable failed `<6h`：不重入队 | ❌ 未写 |
| 5 | retryable failed `>=6h`：补偿一次 | ❌ 未写 |
| 6 | 补偿再失败：再过 6h 不自动循环 | ❌ 未写 |
| 7 | reconcile 后 claim 前 crash/reopen：机会仍在 | ❌ 未写 |
| 8 | claim 后失败：durable budget 已消耗 | ❌ 未写 |
| 9 | ready 后再形成独立新 episode：新 episode 有自己的一次额度 | ❌ 未写 |
| 10 | `unsupported`：单纯时间经过不重试 | ❌ 未写 |
| 11 | `blocked`：单纯 6h 不重试；blocker cleared 才推进 | ❌ 未写 |
| 12 | library 有漫画、无 job：创建 pending | ❌ 未写 |
| 13 | 已有 pending/running：不重复 | ❌ 未写 |
| 14 | ready + 磁盘存在：不动 | ❌ 未写 |
| 15 | ready + 磁盘缺失：P1-B 路径恢复并可消费 | ❌ 未写 |
| 16 | 大量缺口：单次只处理 budget，不外溢 | ❌ 未写 |

时钟：将复用**可注入的 `now`**（现有代码所有 store 函数都已接受 `now: i64`，见
`cover_store::claim_next_job_on(conn, owner, now, lease_ms)`），**不会引入 6h sleep**。

---

# 6. STOP 条件自检

| STOP 条件 | 是否命中 | 说明 |
|---|---|---|
| 找不到现有 source/session lifecycle hook | **未命中**（有保留） | 你指定的 attach/restore hook 确实**不存在**；但你许可的 `scan/import complete` 与两个会话校验入口存在且可用。限制见 §1.3，**请你确认是否接受"下次 per-source 会话事件才补偿"** |
| 必须新增全局 scheduler | 未命中 | 不需要 |
| 必须新增通用 event bus | 未命中 | 不需要 |
| 必须让 Flutter 负责 retry/session lifecycle | 未命中 | 不需要（session 仍由现有 Dart 缓存持有，Rust 侧用 P1-B 的 source-level 解析） |
| schema 无法安全表达且需超最小迁移的重构 | 未命中 | 3 个附加列即可，复用现有幂等迁移范式 |
| 需要修改 P0 网络 gate 语义 | 未命中 | 不需要 |

**因此本轮不 STOP**，但**实现尚未开始**，我没有把设计当成完成。

---

# 7. Gates（本轮）

本轮**没有新增可运行的 P1-C 产物**，因此没有新的 P1-C 测试结果可报。既有状态：

| 命令 | exit | 结果 |
|---|---|---|
| `cargo test --locked -- --test-threads=1`（P1-B 结束时） | 0 | 16 targets ok；388 passed / 0 failed / 2 ignored |
| `cargo clippy --locked --all-targets` | ≠0 | ❌ **既有失败** `this loop never actually loops` @ `src/reader.rs:277`；该文件不在 diff 中（P0 baseline 已存在），未做 unrelated cleanup |
| `flutter analyze` | ≠0 | ❌ **既有** 121 issues（子代理已与改动前基线逐字节比对一致；我未复跑该比对） |
| `git diff --check` | 0 | clean |

本轮**只做了只读侦察**（`grep` / `sed` / 读取），**未修改任何文件**。

# 8. Git

branch `p1-cover-completion`｜HEAD `1bf2e37`（未变）｜本轮**无任何文件改动**（P1-A / P1-B / P1-D 的 dirty 原样保留）｜未 commit / push / merge / reset / stash。

# 9. Scope

| 阶段 | 状态 |
|---|---|
| **P1-C** | **NOT IMPLEMENTED**（前置两步已完成并冻结：生命周期挂点 + durable-state matrix/schema 决策；实现与 16 项测试未开始） |
| **P1-D-2** | **NOT STARTED**（按你的顺序，需 P1-C PASS 后再做） |
| P1-E / P1-F | NOT STARTED |
| P0 / P1-A / P1-B / P1-D | 未回改 |

## 需要你决定的一件事

§1.3 的限制：**overdue 6h 补偿只会在"下一次带有效会话的 per-source 事件"时执行**（打开该源 / 显式
retry / 触发扫描），不存在"启动即补偿"。这是本仓库没有 source-session 生命周期事件的事实结果。
请确认：
- **(A)** 接受该语义 → 我按 §1.3 接线并实现；或
- **(B)** 你要求更主动的补偿 → 那需要新增一个 source-session 生命周期事件（架构决策，我不自建）。

**未进入 P1-E / P1-F，未 push / 未 merge，等待审阅。**
