# P1-B / P1-C / P1-D-2 交付报告

> 工作目录 `D:/Projects/RCH-p1`｜分支 `p1-cover-completion`｜HEAD `1bf2e3743c026581ddaa9d444787fde05fd02f31`（P0 baseline，未动）
> **未 commit / 未 push / 未 merge**｜未进入 P1-E / P1-F

## 现状审计（开工前）

| 检查 | 结果 |
|---|---|
| `git rev-parse --abbrev-ref HEAD` | `p1-cover-completion` |
| `git rev-parse HEAD` | `1bf2e3743c026581ddaa9d444787fde05fd02f31` ✅ 与指定 baseline 一致 |
| `git worktree list` | `D:/Projects/RCH-p1  1bf2e37 [p1-cover-completion]` |
| `git status --short` | 9 modified + 6 untracked（均为 P1-A / P1-D / 本轮 P1-B），另有 7 个生成文件仅 stat 变化 |
| `git diff --cached --name-only` | 空（无 staged） |
| `git diff --check` | clean |

**没有任何 P1_BASELINE_MISMATCH。** 未执行 reset / checkout 覆盖 / stash；未触碰 primary workspace；未清理任何不属于本任务的 dirty 文件。

---

# P1-B

## 根因

`CoverJobState` 的 `ready` 记录若磁盘字节丢失，只读路径会把它对账回 `pending`
（`remote_scan/cover_service.rs:99-114`），但**该转换没有任何唤醒**。而 cover worker：

- `api/remote_scan.rs:135-151` `wake_remote_cover_worker(source_id, session)`：
  `session == 0` 直接返回（`:136`）；single-flight 用进程级 `HashSet` 键 `{source_id}:{session}`（`:142-145`）；退出时移除键（`:149`）→ 可安全再启动。
- `api/remote_scan.rs:283-346` worker：每个 claim 边界检查开关（`:301`）、回收过期 lease（`:306`）、
  按 **session token** claim（`:307-314`）、队空且无未来 `retry_wait` 期限则退出（`:336-345`）。

所以只读路径要唤醒 worker，却**手上没有 runtime session token**，而 worker 的 claim
（`remote_scan/cover_store.rs:786` `epoch.session_token=?3`）又必须用它 —— 结果就是一条
**被搁置的 pending**：封面永久停在 pending，直到下一次全量扫描才可能被重试。

### ⚠️ 更正你的假设（`P1_B_ASSUMPTION_INVALID`，**部分**）

你的指令说"真正怀疑缺失的是**两个**写入 pending 但没有 wake 的路径"。经代码核实：

| 写入点 | 是否缺 wake | 证据 |
|---|---|---|
| `cover_service::read_cached_cover` 对账（ready→pending） | **确实缺** | `cover_service.rs:104-113` 只写库；其唯一生产调用者 `api/remote_cover.rs:466` 当时没有唤醒 |
| `persistence::publish_staged_generation`（staged pending 插入） | **不缺** | 同一分支 `api/remote_scan.rs:1571` 调用 `consume_staged_covers`，后者在函数尾部 `:1726` 调用 `wake_remote_cover_worker` —— **调用链已覆盖** |

因此实际只有**一个**缺失点。按你的要求我**没有硬改** `publish_staged_generation`（未加冗余 wake、未改其调用链）。
我没有整体 STOP，因为该原语本身正是你指定的交付物、且真实缺口确实存在一个；如你认为应据此整体停下，请驳回。

## RED

分两层，都是**先写测试再改实现**。

### 1) 原语层（`src/api/remote_scan.rs` 新增 `mod wake_tests`，4 个用例）

先把 `resolve_cover_wake_session` 与 `wake_cover_worker_for_source` 加为**空实现桩**（`Ok(None)` / no-op），跑出：

```
running 4 tests
test ...source_wake_drains_pending_without_duplicating_the_worker ... FAILED
  panicked: the source wake must start a consumer that drains the pending job
test ...exited_worker_is_restarted_by_a_source_wake ... FAILED
test ...pending_without_a_session_survives_until_a_session_is_attached ... FAILED
test ...repeated_source_wakes_do_not_duplicate_consumption ... FAILED
test result: FAILED. 0 passed; 4 failed
```

### 2) 端到端层（`tests/remote_cover_reconcile_wake_contract.rs`）

真实生产路径：FRB `remote_cover_read` → 对账 → 应有消费者。**未接 wake 时 RED**：

```
panicked at tests/remote_cover_reconcile_wake_contract.rs:138:
the reconcile must leave a live consumer behind, not a stranded pending job
test result: FAILED. 0 passed; 1 failed
```

（即：对账确实把记录变成 `pending`，但 3 秒内**无人领走** = 搁置。）

## 实现（最小）

| 位置 | 改动 |
|---|---|
| `remote_scan/cover_store.rs` | 新增 `resolve_cover_wake_session(conn, source_id, now) -> Result<Option<u64>>`：只解析**真正能 claim 该 source 待办**的 token（与 job 的 `(generation, session_epoch)` 三元组 join、`session_token<>0`、且 job 为 pending 或到期 retry_wait）。取不到 → `None` |
| `api/remote_scan.rs` | 新增 `wake_cover_worker_for_source(source_id)`：开关关闭→返回；解析 token；`None`→**什么都不做**（不伪造 session、不做远程访问）；`Some`→调用**既有** `wake_remote_cover_worker`（同 single-flight、同"队空可退出/可再启动"） |
| `api/remote_cover.rs` | 唯一缺失写入点接线：`remote_cover_read` 在缓存未命中时调用 `wake_cover_worker_for_source(&source_id)` |

**没有**新建 worker registry / scheduler / state machine；**没有**动短退避 `attempt<3` 语义；
**没有**让 Flutter 承担 session 生命周期（调用方只需稳定 source identity）。

## GREEN

```
test api::remote_scan::wake_tests::source_wake_drains_pending_without_duplicating_the_worker ... ok
test api::remote_scan::wake_tests::exited_worker_is_restarted_by_a_source_wake ... ok
test api::remote_scan::wake_tests::pending_without_a_session_survives_until_a_session_is_attached ... ok
test api::remote_scan::wake_tests::repeated_source_wakes_do_not_duplicate_consumption ... ok
test result: ok. 4 passed; 0 failed
```

```
test a_byte_less_ready_cover_is_reconciled_to_pending_and_then_actually_consumed ... ok
test result: ok. 1 passed; 0 failed
```

`tests/remote_cover_wake_contract.rs`（session 解析矩阵，8 行 + 跨 source 不串 + 多代际选对 epoch）：
`test result: ok. 3 passed; 0 failed`

覆盖你要求的 A–D：A 在运行中的 worker + 新 pending（用**未来 `retry_wait` 期限**造出确定的
"worker 存活"窗口，无长 sleep）→ 单飞挡住第二次 spawn、每个 job `attempt==1`；
B worker 退出后 → 可重新启动并消费；C 无 session → **0 worker 启动**（因此零网络）、
job 保持 pending、`attempt==0`，session attach 后同一 source-level 唤醒即生效；
D 连续 5 次唤醒 → 只被 claim 一次。

## regression（P1-B）

| 命令 | 结果 |
|---|---|
| 9 个 cover 契约测试文件（transition / wake / reconcile-wake / missing-file / queue / store / cache / error / directory-view） | 全部 `ok`，0 failed |
| `cargo test --locked --lib -- wake_tests remote_scan::cover_state` | `ok. 6 passed; 0 failed` |
| **全量 `cargo test --locked -- --test-threads=1`** | **EXIT=0；16 个目标全 ok；388 passed / 0 failed / 2 ignored** |

### 我在过程中引入并修掉的两个测试夹具问题（如实记录，均为测试问题而非生产缺陷）

1. `tests/remote_cover_wake_contract.rs`：`persistence::migrate` 需要先有 `library_index` /
   `book_sources` 表 → 夹具补基础表（与既有 `remote_cover_store_contract` 一致）。
2. `tests/remote_cover_reconcile_wake_contract.rs`：接上 wake 后，worker 可能在**同一次调用内**
   就把对账出的 pending 领走（状态变 `running`），我原先硬断言"恰好是 pending"会竞态
   → 改为断言"已离开 `ready`"，再断言最终被消费。**这是修复生效太快的表现。**

## touched files（P1-B 部分）

```
M app/rust/src/remote_scan/cover_store.rs     （+resolve_cover_wake_session；含 P1-A 改动）
M app/rust/src/api/remote_scan.rs             （+wake_cover_worker_for_source、+mod wake_tests）
M app/rust/src/api/remote_cover.rs            （+调用点接线）
?? app/rust/tests/remote_cover_wake_contract.rs
?? app/rust/tests/remote_cover_reconcile_wake_contract.rs
```

---

# P1-C

**NOT STARTED（本轮预算耗尽）。** 没有写入任何一行对应代码或测试。未触发
`P1_C_ARCHITECTURE_EXPANSION`——我尚未开始"是否需要通用 scheduler"的判断。

已完成、可直接复用的侦察结论（供下一轮）：

- 既有有界短退避必须保留、且与 6h 补偿是**两个维度**：`api/remote_scan.rs:153-186`
  `cover_job_failure_state`：`TransientNetwork|RateLimited` 仅 `attempt<3` → `retry_wait`
  （delay 上限 15 min）；`RangeUnavailable|Unsupported` → `unsupported`；
  `Unauthorized|Forbidden` → `blocked`；其余 → `failed`。
- 全仓库**不存在**任何 6 小时 / 周期性"扫所有 failed"路径（已搜索 `6*60*60`、`21600`、
  `hours(6)`、`Timer.periodic`、`backfill`、`replenish`）。
- worker 已具备"为未来 `retry_wait` 期限保持存活（睡眠切片 ≤30 s）"的能力
  （`api/remote_scan.rs:336-345`）—— 这正是"deadline-aware wake"可复用的现成机制。
- `CoverJobUpsertCause` 是唯一状态转换 authority（P1-A）。新增 `TimedCompensation` 必须
  加进 `cover_state::resolve_upsert_state` 的矩阵与该矩阵测试，而不是在别处特判。
- 迁移范式现成：`cover_store.rs` 有"附加列 + 默认值"的幂等迁移循环可加
  `compensation_eligible_at` / `compensation_consumed`。
- **尚未解决的关键设计问题（下一轮第一步）**：overdue reconciliation 应该挂在哪个**既有**
  生命周期事件上（startup / source attach / session restore / capability restore）？
  本仓库没有全局 scheduler，不能新建周期扫描；如果找不到足够的事件钩子，
  就命中你的 STOP 条件 `P1_C_ARCHITECTURE_EXPANSION`。必须在写代码前先回答它。

---

# P1-D-2

**NOT STARTED（本轮预算耗尽）。** 你要求的"先做接口侦察、禁止先写补丁"尚未执行，
因此**没有** evidence table，也**没有**选择 A/B/C 任一路径。

已确认的现状事实（P1-D 遗留，静态分析）：

- `app/lib/ui/comic_cover.dart:498` 仍是
  `if (widget.remoteAssetId == null && _remoteCoverNetworkPaused) return;`
  → legacy 源（`bookCover`/`webdavCover`/`sftpCover` 缓存命名空间）在联网关闭时直接返回，
  磁盘上已存在的封面依旧被隐藏。
- 规范依据已在 `.trellis/spec/backend/remote-cover-update-contracts.md:214-217`：
  "Turning off `remoteCoverFetchEnabled` … stops new network work but preserves existing
  covers and index metadata."
- 待查清你列出的三问：legacy cache key/path 的 authority；能否仅凭稳定
  source/book/asset identity 定位已有磁盘封面；runtime session token 是
  cache namespace 的**身份**一部分，还是仅仅是当前 API 恰好要求的访问上下文。

---

# Gates

| 命令 | exit | 结果 |
|---|---|---|
| `git rev-parse --abbrev-ref HEAD` | 0 | `p1-cover-completion` |
| `git rev-parse HEAD` | 0 | `1bf2e3743c026581ddaa9d444787fde05fd02f31` |
| `git diff --cached --name-only` | 0 | 空（无 staged） |
| `git diff --check` | 0 | clean |
| `cargo test --locked --lib -- wake_tests remote_scan::cover_state --test-threads=1` | 0 | `ok. 6 passed; 0 failed` |
| `cargo test --locked --test remote_cover_wake_contract --test remote_cover_reconcile_wake_contract -- --test-threads=1` | 0 | 3 + 1 passed，0 failed |
| 9 个 cover 契约测试文件（见上） | 0 | 全部 ok |
| **`cargo test --locked -- --test-threads=1`（全量）** | **0** | **16 targets ok；388 passed / 0 failed / 2 ignored** |
| `cargo clippy --locked --all-targets` | **≠0** | ❌ **既有失败（非本轮引入）**：`error: this loop never actually loops` @ `src/reader.rs:277` |
| `rustfmt --check`（我新增/编写的文件） | 0 | `cover_state.rs`、3 个新测试、`api/remote_cover.rs` 全部 OK |
| `rustfmt --check`（共享的既有文件） | ≠0 | `persistence.rs` / `api/remote_scan.rs` / 2 个既有测试文件仍有差异，**绝大多数是既有 in-flight 风格**（如单行 `CoverJobKey{...}`）。按"禁止大范围格式化"我**未**做全仓格式化；只修了 `cover_store.rs` 中**属于我**的那 1 处 hunk |
| `flutter test test/comic_cover_disk_first_test.dart`（P1-D 复核） | 0 | 2 passed |
| `flutter test`（P1-D 回归集 4 个文件） | 0 | 12 passed |
| `flutter analyze` | ≠0 | **既有失败（非本轮引入）**：121 issues，与改动前基线逐字节一致（子代理已 diff 比对，我未复跑该比对 —— 标注为**其证据**） |

### clippy 红灯的归属（不把红灯描述成 PASS）

`error: this loop never actually loops` 位于 **`src/reader.rs:277`**。`git diff --name-only`
**不包含** `src/reader.rs`（`git diff --stat -- src/reader.rs` 为 0 行）→ 该文件与本轮
（以及 P1-A/P1-D）完全一致，即该 clippy 错误在 **P0 baseline 提交 `1bf2e37` 中已存在**。
按"禁止 unrelated cleanup"我**没有**修它。**因此 clippy 这一条本轮不 PASS，属既有失败。**

---

# Git

| 项 | 值 |
|---|---|
| branch | `p1-cover-completion` |
| HEAD | `1bf2e3743c026581ddaa9d444787fde05fd02f31`（未变；本轮无 commit） |
| `git status --short` | 9 modified + 6 untracked（+ 7 个生成文件仅 stat 变化） |
| staged | 空 |
| untracked | `app/rust/src/remote_scan/cover_state.rs`、`app/test/comic_cover_disk_first_test.dart`、`app/rust/tests/{remote_cover_transition_contract,remote_cover_wake_contract,remote_cover_reconcile_wake_contract}.rs`、`docs/reports/p1/` |
| 本轮修改文件 | `app/rust/src/remote_scan/cover_store.rs`、`app/rust/src/api/remote_scan.rs`、`app/rust/src/api/remote_cover.rs` + 上述 2 个新测试文件 |
| 原有 dirty 与本轮追加的交集 | P1-A/P1-D 留下的 9 modified 文件全部保留；本轮只在其中 3 个（`cover_store.rs`、`api/remote_scan.rs`、`api/remote_cover.rs`）追加。未 reset / 未 checkout 覆盖 / 未 stash / 未删他人改动 |
| temp artifacts cleanup | 已删除 `D:\Temp\wake_tests_append.rs`、`D:\Temp\wake_tests_frag.rs`；仓库内无 `.tmp/.log/.part/.jsonl` 或 scratch 文件；未提交任何 graph/build 产物 |

`git diff --stat`（相对 baseline）：

```
app/lib/store/remote_cover_repository.dart   |  32 +   (P1-D)
app/lib/ui/comic_cover.dart                  |  66 +   (P1-D)
app/rust/src/api/remote_cover.rs             |  12 +   (P1-B)
app/rust/src/api/remote_scan.rs              | 288 +   (P1-B)
app/rust/src/remote_scan/cover_store.rs      |  85 +-  (P1-A + P1-B)
app/rust/src/remote_scan/mod.rs              |   1 +   (P1-A)
app/rust/src/remote_scan/persistence.rs      |   7 +-  (P1-A)
app/rust/tests/remote_cover_missing_file_contract.rs |   3 +- (P1-A)
app/rust/tests/remote_cover_queue_contract.rs        |  23 +- (P1-A)
9 files changed, 494 insertions(+), 23 deletions(-)
```

---

# Scope

| 阶段 | 状态 |
|---|---|
| **P1-B** | **PASS**（原语 + 唯一缺失写入点闭环；RED→GREEN；全量门禁 EXIT=0 / 388 passed / 0 failed）——但**附一条更正**：你假设的"两个 no-wake 写入点"实际只有**一个**（`publish_staged_generation` 已由调用链覆盖），我已按要求未硬改 |
| **P1-C** | **NOT STARTED**（本轮预算耗尽；未触发 `P1_C_ARCHITECTURE_EXPANSION`；下一轮第一步必须先回答"overdue reconciliation 挂在哪个既有生命周期事件上"） |
| **P1-D-2** | **NOT STARTED**（未执行接口侦察，因而无 evidence table，未选 A/B/C） |
| **P1-E** | NOT STARTED |
| **P1-F** | NOT STARTED |

**P1 未完成**（B/C/D-2 未全部 PASS）。另有两条门禁红灯：
`cargo clippy`（既有 `reader.rs:277`）与 `flutter analyze`（既有 121 issues）——均非本轮引入，
已在上面给出归属证据。**未 push / 未 merge，等待审阅。**
