# P1-D-2 报告：recovery integrity + offline disk-first 侦察与决策

> 工作目录 `D:/Projects/RCH-p1`｜分支 `p1-cover-completion`｜HEAD `1bf2e3743c026581ddaa9d444787fde05fd02f31`（未变）
> **未 commit / 未 push / 未 merge / 未 reset / 未 stash**｜未进入 P1-E / P1-F / P2

## 状态摘要

| 项 | 状态 |
|---|---|
| §0 accident recovery integrity | **`P1_RECOVERY_INTEGRITY_PASS`** |
| §1 interface evidence table | **已完成**（三个 path 全部沿真实调用链核对） |
| §2 A/B 决策 | **已定：混合 —— 本地路径用 A，legacy 路径必须用 B**（理由见 §3） |
| D2-1 ~ D2-6 实现与测试 | **NOT IMPLEMENTED**（本轮预算耗尽；未改一行代码） |

**本轮对代码零修改**（全部为只读审计与侦察）。按你"未完成该表之前禁止改代码"的要求，
我在表与决策完成时停止，**没有**开始改 `comic_cover.dart`。

---

# 0. `P1_RECOVERY_INTEGRITY_PASS`

对 `cover_store.rs` 做只读审计：`git diff HEAD -- app/rust/src/remote_scan/cover_store.rs`。

## 0.1 规模与结构性证据

| 指标 | 值 |
|---|---|
| HEAD 行数 → 当前行数 | `891` → `1347` |
| diff stat | `+465 / −9`（1 file changed） |
| hunk 总数（`-U0`） | **16 个**，全部为"锚定单行替换"或"纯插入" |
| **函数清单 diff** | **只有 5 个新增函数，零删除** |
| 被删除行数 | **9** |

新增的 5 个函数（全部为本轮预期产物，HEAD 中不存在）：
`resolve_cover_wake_session`（P1-B）、`mark_job_failure_owned_on`、
`has_claimable_work_on`、`reconcile_cover_compensation_for_source_on`、
`reconcile_missing_covers_for_source_on`（P1-C）。

**没有任何 HEAD 原有的函数消失** → 排除"事故导致整段丢失"。删除总量仅 9 行（见 0.3），
排除"异常大段删除/重排"。

## 0.2 hunk ownership table

| hunk（`@@` 头） | 所属阶段 | 对应功能 | 是否预期 |
|---|---|---|---|
| `-7,0 +8` | **P1-A** | 导入 `resolve_upsert_state` / `CoverJobUpsertCause` | yes |
| `-132,0 +134,12` | **P1-C** | 迁移：`remote_cover_job` 加 3 列 | yes |
| `-356 +369,4` | **P1-C** | `mark_job_state_on`：写 ready 时清空 episode 三列 | yes |
| `-381 +397,4` | **P1-C** | `mark_job_state_owned_on`：同上 | yes |
| `-414,0 +434,2` | **P1-C** | `mark_job_ready_owned_on`：清空注释 | yes |
| `-416 +437,2` | **P1-C** | `mark_job_ready_owned_on`：清空三列（结束 episode） | yes |
| `-568,0 +591` | **P1-A** | `upsert_job_on` 新增 `cause` 参数 | yes |
| `-571 +594,22` | **P1-A** | 非嵌套事务（`is_autocommit`/`owned_tx`/`scope`）+ `resolve_upsert_state` + revival | yes |
| `-581 +625,6` | **P1-A** | `state=excluded.state`（替换被禁止的隐式保留）+ 短重试预算重置 CASE | yes |
| `-590 +639` | **P1-A** | 落库用 `resolved.as_str()` 而非请求状态 | yes |
| `-595 +644,128` | **P1-A + P1-B** | `i64::from(revival)` 收尾 + 其后插入的 **P1-B `resolve_cover_wake_session`** 与 P1-C 常量/`ReconcileBudget`/`ReconcileReport`/`mark_job_failure_owned_on`/`has_claimable_work_on` | yes |
| `-598 +774,273` | **P1-A + P1-C** | `upsert_job_on` 尾（`load_job_on(scope)` + commit）+ 插入 **两个 reconciler** | yes |
| `-665,0 +1114,2` | **P1-C** | `claim_next_job_on`：claim 时原子消耗长期补偿 | yes |
| `-709,0 +1160,2` | **P1-C** | `claim_next_job_for_source_on`：同上 | yes |
| `-765,0 +1218,2` | **P1-C** | `claim_next_job_for_source_session_on`：同上（生产 claim 路径） | yes |
| `-808 +1262,3` | **P1-C** | `lease_job_on`：同上 | yes |

（两个大 hunk `+128` / `+273` 之所以呈现为"1 行 → N 行"，是因为 git 把"锚点行改写"与
"紧接着插入的新函数"合并成一个最小 diff 块；hunk 头里的函数名 `upsert_job_on` 只是
**最近的上下文函数**，不是改动发生的位置。）

## 0.3 全部 9 行删除的逐行归属

| # | 删除内容 | 位置 | 所属 | 替换为 |
|---|---|---|---|---|
| 1 | `lease_owner=NULL,lease_until=NULL,updated_at=?3` | `mark_job_state_on` | P1-C | 追加 ready 清空 CASE |
| 2 | 同上 | `mark_job_state_owned_on` | P1-C | 同上 |
| 3 | `next_attempt_at=NULL,lease_owner=NULL,lease_until=NULL,updated_at=?1` | `mark_job_ready_owned_on` | P1-C | 追加清空三列 |
| 4 | `conn.execute(` | `upsert_job_on` | P1-A | `scope.execute(` |
| 5 | **`state=remote_cover_job.state,`** | `upsert_job_on` | **P1-A** | **`state=excluded.state,`（正是被禁止的隐式语义）** |
| 6 | `state.as_str(),` | `upsert_job_on` | P1-A | `resolved.as_str(),` |
| 7 | `now` | `upsert_job_on` | P1-A | `now, i64::from(revival)` |
| 8 | `load_job_on(conn, &job_key)?.ok_or_else(...)` | `upsert_job_on` | P1-A | `load_job_on(scope, ...)` + `owned_tx.commit()` |
| 9 | `lease_until=?2,attempt=attempt+1,next_attempt_at=NULL,updated_at=?3` | `lease_job_on` | P1-C | 追加 claim 消耗 |

**结论：16 个 hunk 与 9 行删除 100% 可归属 P1-A / P1-B / P1-C，无遗漏、无额外重写。**

## 0.4 独立元素在位核对（不依赖"406 tests passed"）

在 `cover_store.rs` 中直接计数确认 P1-A/B/C 的关键元素**各在位且无重复残骸**：

```
is_autocommit 1   owned_tx 3   resolve_upsert_state 3   state=excluded.state 2
i64::from(revival) 1   resolve_cover_wake_session 1   mark_job_failure_owned_on 2
reconcile_cover_compensation_for_source_on 1   reconcile_missing_covers_for_source_on 1
LONG_RETRY_DELAY_MS 1   long_retry_pending 18
```

未重新格式化整个文件（`rustfmt` 只作用于我自己的 hunk，已在 P1-B 报告中说明）。

---

# 1. Interface evidence table

沿真实调用链（Flutter widget → repository → FRB → Rust → disk cache / network boundary）核对。

| Path | Flutter entry | Rust/FRB API | Disk lookup owner | Disk lookup before network? | Miss may network? | Offline current behavior |
|---|---|---|---|---|---|---|
| **remote（unified）** | `comic_cover.dart:696`（`_loadUnifiedRemoteCover`）← `repository.readCover`（`remote_cover_repository.dart:86`） | `api::remote_cover::remote_cover_read` → `cover_service::read_cached_cover` | **Rust**：`cache::remote_cover_cache_read` | ✅ 是（`:696` 在开关判定 `:705` **之前**） | ❌ **从不触网**（网络只由 `:707 remote_cover_request` + worker 承担） | ✅ **正常显示**（P1-D 已修） |
| **legacy（webdav / sftp / baidu / 115 / quark）** | `:587` / `:602` / `:617` / `:632` / `:648` | `webdav_cover`(source.rs:1656) / `sftp_cover`(:1067) / `baidu_cover` / `cloud115_cover_for` / `quark_cover` | **Rust**：`*::raw_cache_path` + `cache::cover_cache_read` | ⚠️ 是——**但在 `get_session(session)?` 之后**（例：`sftp_cover` 先取 session，再 `raw_cache_path`） | ✅ **是**（命中磁盘则 return，否则继续走 provider 读取） | ❌ **被隐藏**：`:498` 提前 return；且即便到达分支，每次调用都被 `_guardRemoteCoverIo` 包裹，开关关闭时返回 null → 抛 `_RemoteCoverFetchDisabled` → 占位图 |
| **custom / local** | `:663`（`bookCover`，唯一不需要 session 的分支） | `api::book::book_cover`（book.rs:107） | **Rust**：`cache::cover_cache_read` + 本地文件解码 | ✅ 是（函数第一段就是"先查磁盘缓存"） | ❌ **从不触网**（只解码本地文件） | ❌ **被错误隐藏**：它根本不需要网络，却被同一个 `:498` 网关拦住 |

## 1.1 你要求明确标出的 5 点

| 问题 | 答案 | 证据 |
|---|---|---|
| `comic_cover.dart:498` 的提前 return 位于**哪一层** | **Flutter widget 状态层**（`_maybeLoad`），在 `_load()` 被排入队列**之前**短路。它不是 Rust/FRB 层、也不是 cache 层的判定 | `:486-512`（`_maybeLoad`）+ `:509` 才 `acquire(key, _load)` |
| 哪个 API 最终拥有 cache path | **Rust**：unified 走 `cache::remote_cover_cache_read`；legacy/local 走 `cache::cover_cache_read` + `*::raw_cache_path` | 上表 |
| Dart 是否知道或拼接磁盘路径 | **不知道、不拼**。`comic_cover.dart` 与 `remote_cover_repository.dart` 中没有任何 `Directory` / `path.join` / cache-path 构造（grep 命中的全是 `RemoteDirectoryViewLoader` 这类 typedef 名） | grep 结果 |
| Rust API 在 disk miss 时是否自动触发网络 | **unified：不会**（`remote_cover_read` 是纯读）；**legacy：会**（命中即 return，未命中继续走 provider）；**local：不会** | 上表 |
| network-disabled flag 在哪一层生效 | **三层**：① UI 层 `:498` 的 blanket gate；② 逐调用网关 `_guardRemoteCoverIo` → `runRemoteCoverOperationWithGate(isEnabled: () => !_remoteCoverNetworkPaused)`（`:315-324`，disabled 时 operation 返回 null → 抛 `_RemoteCoverFetchDisabled`）；③ Rust 层 `current_cover_fetch_enabled()`（worker 每个 claim 边界 `api/remote_scan.rs:301`）与 P1-B 的 `wake_cover_worker_for_source` | 行号 |

## 1.2 一处必须更正的既往判断

我在 P1-D 报告里曾建议"legacy 路径用现有 `CoverFetchPolicy.cacheOnly` 在 Dart 侧解决"。
**这是错的。** 全仓搜索 `app/rust/src` + `app/lib`：

```
grep -rn "CoverFetchPolicy\|cacheOnly\|cache_only" → 零命中
```

`CoverFetchPolicy` 只存在于规范文档（`remote-cover-update-contracts.md`），**代码里没有实现**。
我当时把文档当成了既有实现。特此更正，并说明这也直接改变了 P1-D-2 的方案选择（见 §3）。

---

# 2. 冻结语义的落点

你的统一规则是 `local evidence first, network permission second`。按上表，当前分歧只在两点：

- **local 路径**：完全没有网络能力，却被网络网关挡住 → 纯粹是**排序错误**。
- **legacy 路径**：磁盘查询存在于 API 内部，但**入口需要活跃 session**（`get_session(session)?` 在
  磁盘查询之前），而获取 session 本身就是要被网关禁止的网络行为 → **单靠移动 Dart 的 guard
  无法修好**。

---

# 3. 最终方案选择：混合（本地 = A，legacy = B）

## 3.1 本地 / custom（`:663 bookCover`）→ **方案 A**

它**没有网络路径**、不需要 session。因此只需让它不再被网络网关短路：

> 把 `:498` 的 blanket gate 收窄为"仅对有网络能力的路径生效"，让 `bookCover` 这类
> **已证明无网络**的本地读取先走 `local evidence first`。

这是最小修复，符合你 §2 的 A 定义（现有 API 已满足 `disk → hit return → miss 才可能 network`，
且它连 miss 都不会触网）。

## 3.2 legacy（`:587`–`:662`）→ **方案 B**

**为什么 A 不成立**：legacy 的 `disk lookup` 在 Rust API **内部**，而该 API 的**第一个动作**
就是 `get_session(session)?`；Dart 侧获取 session 的 `*SessionFor(source)` 又被
`_guardRemoteCoverIo` 逐调用拦截。结果是：**offline 时既拿不到 session、也就永远走不到磁盘查询**——
即使磁盘上有封面、即使 Dart 会话缓存里还有 id。A 做不到 D2-1。

**B 的最小形态**（严格符合你的 §5 边界：local-only / side-effect-free）：

- 新增一个 Rust-owned、**sessionless** 的 local-only cover lookup，输入稳定身份
  （`source_id` + 书源类型/origin + logical path + page/size/crop），
  内部用**既有**的 `*::raw_cache_path` + `cache::cover_cache_read` 计算并读取磁盘缓存；
- 它**不创建 session、不唤醒 worker、不创建 job、不访问 provider、不修改 durable state**；
- Dart 流程改为：

  ```
  local lookup → hit: 显示
              → miss + network off: 停止（占位图）
              → miss + network on: 走既有 remote fetch（legacy 分支原样保留）
  ```

- 该调用必须放在 `_guardRemoteCoverIo` **之外**（它不是网络操作，不应受网络网关管辖）。

## 3.3 为什么不是 C

C 明确禁止的六项我都没有采用：不拼 cache path、不建第二套 Dart cache、不复制 Rust 索引、
不在 offline 时篡改 durable job state、不伪造 failed/unsupported、不改 P1-C 的 retry/lifecycle。

---

# 4. D2-1 ~ D2-6 的状态：**全部 NOT IMPLEMENTED**

| # | 要求 | 状态 |
|---|---|---|
| D2-1 | network off + disk cover exists → visible，0 provider request | ❌ 未实现（需 §3.2 的 B） |
| D2-2 | network off + disk missing → placeholder，0 provider request | ❌ 未实现 |
| D2-3 | network on + disk exists → disk hit，0 provider request | ❌ 未实现（unified 已满足；legacy 待 B） |
| D2-4 | network on + disk missing → 进入既有 remote fetch，不另造路径 | ❌ 未实现 |
| D2-5 | custom/local：network flag 不影响显示 | ❌ 未实现（需 §3.1 的 A） |
| D2-6 | legacy 与 remote 不得一个修好另一个又被提前 return | ❌ 未实现 |

将要观测的 **network boundary / provider-call counter**（不靠 widget 文本断言）：

1. **Dart 侧**：`comic_cover.dart` 的 `_guardRemoteCoverIo` 是唯一的网络许可层 → 可用
   `runRemoteCoverOperationWithGate` 的可注入性（或对 `*SessionFor` / `*Cover` 的注入）
   统计"是否被调用"；现有 `test/comic_cover_disk_first_test.dart` 已用
   `RemoteCoverRepository` 注入 + "loaders that would reach the network record+throw" 的模式，
   可直接复用并扩展到 legacy/local 分支。
2. **Rust 侧**：新 local-only API 的测试需证明它**不**触碰 session/provider ——
   可用"不存在 `book_sources` / 无 epoch 时仍能读到磁盘缓存"来证明 sessionless，
   并用计数断言（不注册任何 adapter、无 `remote_scan_epoch` 行也不报错）。

**本轮没有写这些测试**，也没有动 `comic_cover.dart`。

---

# 5. P1-C residual risk（按你 §6 要求记录，不扩展）

> **session-ready notification delivery is non-durable; failure delays reconciliation but does not
> corrupt cover state.**

具体到实现：`RemoteSessionSuccessHub.emit` 以 `unawaited(_notifyReady(...).catchError(...))`
发出通知，`_nativeNotifySourceSessionReady` 内部亦 try/catch。因此：

- 通知失败**不影响**正常 session acquisition（会话已提交、事件已投递）；
- 长期补偿**可能延迟**到下一次有效 lifecycle event；
- **不会**损坏 cover state（不写 durable state、不改 job 状态）。

当前错误是被**完全静默吞掉**的。按你 §6 的允许范围，可以补一条与项目现有日志体系一致的
诊断日志（**不新增任何持久化机制 / retry queue / event persistence**）。
**本轮未加**（避免在没有日志约定确认的情况下自造日志通道）—— 记为待办。

---

# 6. Gates / Git

| 命令 | exit | 结果 |
|---|---|---|
| `git diff HEAD --stat -- cover_store.rs` | 0 | `+465 / −9` |
| `git diff HEAD -U0 -- cover_store.rs \| grep -E "^@@"` | 0 | 16 个 hunk，全部可归属 |
| 函数清单 `diff`（HEAD vs working） | 1（差异存在，符合预期） | **只有 5 个新增函数，零删除** |
| 被删除行清点 | 0 | **9 行，逐行可归属** |
| `git diff --check` | 0 | clean |
| 本轮代码修改 | — | **零**（仅只读 grep/sed/read） |

| 项 | 值 |
|---|---|
| branch / HEAD | `p1-cover-completion` / `1bf2e37`（未变） |
| staged / commit / push / merge / reset / stash | 全部无 |
| P1-A / P1-B / P1-C / P1-D 的 dirty | 全部原样保留 |
| temp artifacts | 无（本轮未生成任何临时文件） |

---

# 7. Remaining risks

1. **§3.2 的 B 需要新增 FRB API → 必须再跑一次 codegen**，会再次改动生成文件
   （`app/rust/src/frb_generated.rs`、`app/lib/src/rust/**`）。这与你此前对生成文件改动的关注相关，
   但它是决策 B 路线的必然结果（与 P1-C 的 `notify_source_session_ready` 同理）。
2. **legacy 的 cache path 需要 origin**（webdav 用 `origin`、sftp 用 endpoint），
   而 `raw_cache_path` 目前只接受 `&origin`/client 派生值。若 origin 不能仅由
   `book_sources`（url/username/path）稳定推导，则 B 会退化为"必须持有 session" —— 那时
   就只能命中 `P1_D2_CACHE_AUTHORITY_AMBIGUOUS`。**这一点必须在实现 B 之前用代码确认**，
   也是我建议作为下一步第一件事的原因。
3. unified 路径已由 P1-D 满足 D2-3 的语义（`:696` 先读本地、`:705` 才看开关）；
   但**尚无 D2-3 的显式测试**。
4. 仓库级既有红灯仍在：`cargo clippy`（`src/reader.rs:277`）、`flutter analyze`（121 issues），
   均非本轮引入。
