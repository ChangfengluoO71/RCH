# P1-E 决策记录：`remote_view_revision` 复用安全性（§1 / §29）

> `D:/Projects/RCH-p1`｜分支 `p1-cover-completion`｜HEAD `1bf2e3743c026581ddaa9d444787fde05fd02f31`（未变）
> **本轮对生产代码零修改**｜未 commit / push / merge / reset / stash

## 判定

> ## **SAFE —— 复用既有 `remote_view_revision`，不新增第二套 revision**
>
> 即 §29 的独立 `remote_cover_revision` 分支**不启用**；§1 的 Unsafe case 未成立。

---

# 1. 消费者全量追踪

| 消费者 | 位置 | 读到 revision 后做什么 | 是否产生 remote 副作用 |
|---|---|---|---|
| `remote_directory_view` | `rust/src/remote_scan/catalog.rs:465` | `let revision = cover_store::view_revision(conn, source_id)?;` → 作为字段附在目录视图 DTO 上返回 | ❌ **纯读**。listing 内容来自 `remote_listing_state`/`remote_scan_listing_stage` 的本地表；**不因 revision 变化而重新 listing、不触 provider** |
| `RemoteScanStatusDto.view_revision` | `rust/src/api/remote_scan.rs:57`（由 `:1395-1396` 填充） | 随 scan status 返回给 Dart | ❌ 本地 DB 读 |
| FRB getter | `rust/src/api/remote_cover.rs:560-562` | 返回 `i64` | ❌ 本地 DB 读 |
| Dart `RemoteScanStatus.viewRevision` | `lib/store/remote_scan_coordinator.dart:640` | `viewRevision: dto.viewRevision.toInt()` | ❌ 仅存字段 |
| Dart 模型 | `lib/store/remote_scan_models.dart:22`（字段）/`:45`（默认 0）/`:70`（`copyWith`） | 无逻辑 | ❌ 仅搬运 |

**全 `lib/` 搜索 `viewRevision|view_revision`（排除 `lib/src/rust` 生成物）仅上述 4 处命中，
没有任何"revision 变化 → 使目录快照失效 / 重取 / 重新 listing"的逻辑。**

⇒ 每个 cover transition 增加一次 bump，最多导致：本地 view DTO 的 version token 递增、
scan status 的 token 递增 —— **属于你 §1 定义的 Safe case（本地 durable view refresh / local cache invalidation / UI refresh）**。

# 2. 由本判定确定的后续实现（已冻结，未实施）

| 项 | 决定 |
|---|---|
| revision 载体 | **复用** `remote_view_revision`（`cover_store.rs:87` 表 / `:324 bump_view_revision_on` / `view_revision(conn, source_id)`） |
| bump 位置 | 9 个 cover-job durable transition 的**同一事务内**（§1 的 checklist，见 `2026-09-17-p1e-revision-coverage-checklist.md`） |
| bump 粒度（§2） | **每个成功的逻辑 durable transition 恰好一次**；先画实际 call graph，把 bump 放在**最靠近事务拥有者**的位置，避免 helper 栈内重复 bump（不得稳定 +2/+3） |
| bump 条件（§3） | 只有 **affected/inserted rows 真正改变 durable state** 才 bump；`0 affected`（lease owner 不匹配、claim 竞争失败）**不 bump**（禁止制造假 revision） |
| 事务模式（§4） | 沿用既有 `is_autocommit` / owned transaction / `_on` helper；**不得覆盖** P1-A upsert、P1-B ready-missing、P1-C long compensation 的既有 dirty 事务代码 |
| 事件（§6） | `CoverRevisionEvent { source_id, asset_id? }` —— **不带 revision**；Dart 收到后自行读一次 durable version token；duplicate event 无害 |
| transport（§7/§8） | 仓库首个 `StreamSink`，保持极小：**单 subscriber**（install/replace 当前 sink）；send 失败只清理失效 sink；不做 multi-subscriber broadcast registry；**widget 永不直接订 FRB stream** |
| emit 时机（§9/§10） | **COMMIT 之后** emit；禁止事务内 `StreamSink.add()`（UI 卡住/消费慢/ sink 异常都不得延长 DB 事务）；emit 由 transaction-owning public wrapper 或 worker 的统一出口调用，**不塞进最低层 `_on(conn)` helper**；可抽极窄的 `notify_cover_revision(source_id, asset_id?)`（只 send，不查 DB） |
| 只读 API（§12） | 给 `catalog.rs:313 cover_state_for` 加最薄 FRB wrapper：`sourceId + assetId → Option<RemoteCoverStateDto>`（None = no job；no enqueue/session/worker/provider/mutation） |
| missed-event recovery（§11） | coordinator 维护 `sourceId → lastSeenRevision`；仅在**首次观察 source / 收到 wake-up / stream reconnect / source context 切换或重新 attach** 时读 durable revision；`durable > lastSeen` → 更新 notifier + refresh local aggregate + cards reread；**无 timer** |
| F 刷新（§27） | 与 E 共用同一事件；aggregate refresh 必须 **DB/filesystem local-only**；允许**事件触发的 micro-debounce 合并 burst**，**不是**循环 timer |
| 500ms scan monitor（§15/§23） | **保留**（tree scan / generation / pause-resume-cancel 用）；但必须有测试证明「scan terminal 后 cover completion correctness 不依赖它」 |

# 3. 本轮状态（如实）

| # | 项 | 状态 |
|---|---|---|
| 1 | §1 `remote_view_revision` 复用安全性判定 | ✅ **本轮完成（SAFE）** |
| 2 | 9 个 transition 同事务补 bump（含 REV-1~REV-9 RED） | ❌ 未开始 |
| 3 | 窄 cover revision stream（单 subscriber `StreamSink`） | ❌ 未开始 |
| 4 | 只读 asset-state FRB wrapper（含 STATE-READ-1~3） | ❌ 未开始 |
| 5 | Dart coordinator bridge + missed-event recovery | ❌ 未开始 |
| 6 | 删除 `30×350ms`（`comic_cover.dart:852-868`）/ `8×900ms`（`:583-585`） | ❌ 未开始 |
| 7 | E RED（含 E-SCAN-TERMINAL-READY / E-SCAN-TERMINAL-FAILED）+ state→UI matrix | ❌ 未开始 |
| 8 | F：`is_cover_material_available` 纯函数 + `availableBooks/waitingBooks/otherBooks` + no-job/stale-ready 归桶 + invariant + F-1~F-11 | ❌ 未开始 |
| 9 | codegen + generated diff audit + P1 总门禁 | ❌ 未开始 |

**本轮零生产代码修改** ⇒ `P1_D2_PASS` 证据不受影响；**`P1_PASS` 未声明**；未进入 P2。

# 4. 门禁与完整性

| 项 | 结果 |
|---|---|
| 生产代码修改 | **零**（仅只读 grep/sed） |
| `git diff --check` / staged | clean / **0** |
| `P1_RECOVERY_INTEGRITY_PASS` | 未受影响（零文件改动） |
| branch / HEAD | `p1-cover-completion` / `1bf2e37`（未变） |
| 临时产物 | 无 |
| baseline exception（保留，未修） | `cargo clippy` `src/reader.rs:277`；全仓 `flutter analyze` 121 issues；完整 `flutter test` 的 6 个既有编译/装配失败 |

## Remaining risks

1. **SAFE 判定的时间点性**：当前 Dart 无 revision 失效逻辑；若将来有消费者把 `viewRevision`
   当作目录快照 cache key，cover bump 会放大目录重读 —— 届时需回看本决策。
   （`folder_snapshot_store` 相关测试在既有失败集合中，需在实施 bump 时顺带确认它不 key on revision。）
2. 9 个 transition 的 bump 必须同事务且沿用 `is_autocommit`/`owned_tx`；`0 affected` 不得 bump。
3. 首个 `StreamSink` 需 codegen 后审核 generated diff（含 `frb_generated.web.dart`）。
4. F 的 `availableBooks` 必须复用**纯** validity 判断；若 P1-B helper 会做 `ready → pending`，须先抽纯函数。
5. `discoveredBooks = max(current-generation indexed eligible, staged cover tasks)`；差额归 `noJobBooks → waitingBooks`；
   `trackedDistinct > discovered` 时必须暴露为 invariant violation，不得 silent clamp。
6. 115-web / Quark 的 raw-key `present→absent` 仍 known unrecoverable（P1-D2 冻结）。
7. **Release Gate PENDING**（真实 115/Quark 校验、P0 S1–S5、无新增 403/405/429、CDN rate/burst/in-flight
   实测、20/100/300 MiB Range 证据）—— P1 完成 ≠ 可发布。
