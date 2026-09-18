# P1-E / P1-F 实现轮次报告（2026-09-18）

本文件补齐本会话**实现轮次**的可追溯性：此前 `docs/reports/p1/` 只有审计期报告，
U-B / U-α 等冻结语义仅存在于代码注释中。

## 1. 范围

- **P1-E**：事件驱动卡片状态（9 个 cover durable transition 的 post-commit wake）、
  仓库首个 Rust→Dart 窄 stream、只读 state API、Dart coordinator bridge、删除两个轮询循环。
- **P1-F**：可用性语义（`available` / `waiting` / `other`）、精确分母、不变量、进度文案。

## 2. 冻结语义（本会话裁决）

1. **`remote_view_revision`** = *monotonic source-generation token for an **atomic durable view
   change***。数值增量**不**编码改动行数或类别：一个事务内创建/修改 N 个 job 仍然 `+1`，
   一次 atomic batch 只 wake 一次。
2. **cover stream** = **best-effort wake-up transport**，事件只表示
   "该 source 的 cover durable truth 可能已变"，**不携带** state / error_code / retry 信息 /
   authoritative revision；consumer 必须自己重读 durable state。
3. **U-B（已裁决，优先于 U-A）**：等价 upsert **会**刷新 `updated_at`，而聚合按
   `updated_at` 取 latest-per-asset ⇒ 它**属于 observable change** ⇒ `+1 revision / +1 wake`
   是正确行为。因此**修改测试**而非生产代码；不引入 SQL `WHERE` 抑制时间戳刷新。
4. **U-α（revision/wake 粒度所有权）**：revision 的粒度所有权属于**拥有该事务的那一层**。
   `upsert_job_on` 仅在 `owned_tx.is_some()`（autocommit）时 bump；**借用外层事务时只 mutate，
   不 bump 不 emit**，由 outer transaction owner 在 commit 后决定 bump/wake 一次。
5. 锁纪律：**No file I/O under the database lock** —— `available_books` 的缓存/文件系统校验
   必须在 DB 锁释放后进行。

## 3. 落地内容

### Rust
- 9 个 cover transition 接入 `bump_view_revision_on`：`upsert_job_on`、三处 claim、
  `mark_job_ready_owned_on`、`mark_job_state_on/_owned_on`、`mark_job_failure_owned_on`、
  `reconcile_cover_compensation_for_source_on`、`reconcile_missing_covers_for_source_on`、
  `cover_service::read_cached_cover`、`publish_staged_generation`。
  失败路径（`0 affected` / 竞争失败 / wrong owner）一律 **不 bump**。
- 新增 `remote_scan/cover_revision_stream.rs`：单 subscriber holder（重订阅替换、send 失败清失效 sink）、
  `notify_cover_revision`（**只在 commit 之后**调用）。
- 新增 `api::remote_cover::subscribe_cover_revisions`（FRB StreamSink）与
  `remote_cover_state`（只读，复用 `cover_state_for` + `cover_dto`）。
- 新增 `remote_scan/cover_progress.rs`：锁内收集（latest-per-asset 身份、`tracked_distinct`、
  未知 state）→ **锁外** `cover_material_available` 校验 → `available/waiting/other` + 不变量
  （违例 `debug_assert!` + 计入 `other`，**不静默 clamp**）。
- `RemoteScanStatusDto` 新增 `available_books` / `waiting_books` / `other_books`（raw 字段全部保留）。

### Dart
- `RemoteScanCoordinator`：唯一订阅 `subscribeCoverRevisions()`，维护 `lastSeenRevision` +
  `ValueNotifier<int>`，durable-token 去重、missed-event catch-up、聚合刷新 best-effort、**无 timer**；
  `lib/main.dart` 启动唯一订阅。
- `ComicCover`：删除 **30×350ms** 轮询与 **8×900ms** 重复 request；首次 `requestCover` 仅一次
  （`_coverRequestIssued` + 只读 `readState` seam）；state→UI **只有 running 转圈**。
- 进度主文案 `可用 X / 共 Y 本`（source scope，不新增 folder progress）。

## 4. 实现过程中发现并修复的真实缺陷

1. `running` 丢失 spinner：`requestCover` 返回的 durable state 未落回错误路径。
2. wake 后重复 `requestCover`（违反 REQUEST-ONCE）：加 `_coverRequestIssued` 与只读 `readState`。
3. wake 链路把本地读取异常抛给消费者：聚合刷新改 best-effort。
4. 同一事务 double bump（listing bump + cover bump = +2）：按 §6 用进入事务前的 revision 比对。
5. staged-only 漫画被重复计数（`pending` 与 `no_job` 各算一次，sum=6 vs discovered=3）：
   新增 `staged_pending_represented` 扣除。该缺陷由 F 的**不变量断言**捕获。
6. reconciler 批次 N 次 bump：改为批次事务 + 批次 bump 一次。
7. 归因纠正：`read_cached_cover` 的 `InvalidQuery` 实为**生产守卫**（generation 非 Running），
   不是 SQL 错误。

## 5. 验证（fresh）

| Gate | 结果 |
|---|---|
| `cargo test --locked -j 2 -- --test-threads=1` | **EXIT=0 · 24 targets · 453 passed / 0 failed / 0 ignored** |
| 契约测试 | **60 条全绿**（Rust 38 + Dart 22） |
| Flutter focused（D2/E/F/bridge） | 22 passed / 0 failed |
| Changed-file `flutter analyze` | No issues found |
| Full `flutter analyze` | 121 issues（与基线一致，无新增） |
| `cargo clippy --locked --all-targets` | 仅既有 `src/reader.rs:277`；改动文件零发现 |
| **P1-D2 fresh 复验** | unified disk-first、ready-but-missing 对账、offline/online legacy 顺序、local/custom offline、cache authority、SFTP raw-key identity 全部保持 |

## 6. 证据边界

- Rust `wake_decided_count()` = **post-commit notify 决策**证据，**不是** delivery 计数。
- **真实 FRB `StreamSink` 端到端投递**：生产接线存在（`main.dart` 启动订阅，codegen 已生成），
  但自动化测试**未构造真实 sink**；Dart 侧的消费逻辑（去重 / catch-up / 聚合刷新）由
  coordinator 真实 wake 路径覆盖。
- `session`/`provider` 事件在 P1-D2 中记录于注入的 legacy 传输边界，而非真实 FRB session 层。

## 7. Family 2（known unrecoverable）

> all deterministically recoverable historical cover identities are supported. Legacy raw-key
> covers for 115-web and Quark become unrecoverable if their raw file is removed, because the
> provider-returned filename was never durably persisted.

## 8. 遗留

- **Release Gate PENDING**：真实 115/Quark 校验、P0 S1–S5、无新增 403/405/429、
  WAF/download-thread 风险、真实 CDN rate/burst/in-flight 确认、20/100/300 MiB Range 与取消证据。
- **基线例外（未修）**：6 个既有 Flutter 测试失败（`CustomCover*` / `remoteLivePaths` /
  `UpdatePlatform*` 等符号在 `lib/` 中不存在）、121 条 analyze、clippy `reader.rs:277`、
  `cover_service.rs` 在 HEAD 即已 dirty 的 rustfmt 偏差。
- 审查遗留（未处置，非阻塞）：`wake_decided_count`/`reset_wake_decided_count` 仍为永久 pub API
  （建议 `#[doc(hidden)]`）；Dart 三个 `debug*` 注入点建议加 `@visibleForTesting`；
  三处 claim 的 bump 行与两个 reconciler 的 epoch 查询可去重。
- 无 commit / push / merge / reset / stash。
