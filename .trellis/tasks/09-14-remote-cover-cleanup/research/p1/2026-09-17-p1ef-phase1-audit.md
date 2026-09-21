# P1-E / P1-F 第一阶段：只读调用链审计报告

> `D:/Projects/RCH-p1`｜分支 `p1-cover-completion`｜HEAD `1bf2e3743c026581ddaa9d444787fde05fd02f31`（未变）
> **本轮对生产代码零修改**（全部只读）｜未 commit / push / merge / reset / stash

## 状态

> 你要求的"第一阶段：先做只读调用链审计"**已完成**：7 个问题全部有 `file:line` 答案，
> 并产出 Consumer table（见 §2）。**P1-E / P1-F 实现尚未开始**（未改一行生产代码）。

按你"在改生产代码前必须回答"的前置要求，我在审计闭环处停下，以便你先确认两个会**显著改变方案规模**
的发现（§3）：F 的聚合**已经存在**且已是通知驱动；E 的 per-asset 状态**没有**订阅机制（那才是真缺口）。

---

# 1. 七个问题（逐条）

| # | 问题 | 答案（`file:line`） |
|---|---|---|
| 1 | `30 × 350ms` polling 精确位置 | **`comic_cover.dart:852-868`**（`_loadUnifiedRemoteCover` 内）：`for (var attempt = 0; attempt < 30; attempt++)`，`attempt > 0` 时 `await Future<void>.delayed(const Duration(milliseconds: 350))`（`:857`），每轮 `repository.readCover(...)`（`:859-864`）。注释自述 "Request is intentionally non-blocking on the Rust side. Polling only the local cache…"（`:849-851`） |
| 2 | `8 × 900ms` repeated request 精确位置 | **`comic_cover.dart:583-585`**（`_scheduleUnifiedRetry`）：`if (_remoteRetryAttempts >= 8 \|\| _remoteRetryTimer != null) return;` → `_remoteRetryAttempts++` → `_remoteRetryTimer = Timer(const Duration(milliseconds: 900), …)`；由 `_attachLoadResult` 的 `catchError`（`_onRemoteScanStatusChanged`/`_scheduleUnifiedRetry`）驱动 |
| 3 | 卡片当前如何知道 7 种状态 | **统一 remote 卡片并不知道**。它只拿到两个来源：① `requestCover` 的**一次性返回** `RemoteCoverStateDto`（`remote_cover_repository.dart:115`）——拿到即丢，未订阅；② scan 级 `RemoteScanCoordinator.statusFor(sourceId)`（`comic_cover.dart:377`）→ `ValueNotifier<RemoteScanStatus?>`，只在 `_onRemoteScanStatusChanged`（`:389-408`）里用作"扫描成功且我曾失败 → 重试一次"。**`pending/running/retry_wait/failed/unsupported/blocked` 从未被卡片识别为展示状态**：非 ready 一律表现为 spinner（`_loading()`），超时后 `throw StateError('封面仍在队列中')`（`:869`）→ 占位图 |
| 4 | folder representative 状态来源 | 同一条 unified remote 路径：`source_browser.dart:1568-1570` 把 `remoteEntry?.representativeAssetId` 传给封面组件；folder 与 comic 各自建**自己的** `ComicCover` 实例与自己的 load/poll |
| 5 | folder 与 comic 是否已共享 assetId | ✅ **已共享 canonical assetId**。`representativeAssetId` 来自 Rust `RemoteDirectoryViewDto`（`lib/src/rust/api/remote_cover.dart:175`），不是显示名或 path ⇒ E4 的共同订阅键已具备 |
| 6 | 是否已有可复用的 stream / notifier / callback / subscription / refresh | ✅ **有，而且已在用**：`RemoteScanCoordinator` 持有 `Map<String, ValueNotifier<RemoteScanStatus?>> _statuses`（`remote_scan_coordinator.dart:154`）与 `Map<String, ValueNotifier<RemoteScanViewState?>> _viewStates`（`:155`），暴露 `statusFor(sourceId)`（`:166`）/`viewStateFor(sourceId)`（`:167`）。消费者：`comic_cover.dart:377`、`source_browser.dart:1353`、`source_tree.dart:216`。另有 `RemoteSessionSuccessHub`（broadcast `Stream`，`:45-50`）。**⇒ 不需要新 event bus** |
| 7 | 卸载/滚出视口时订阅如何释放 | `comic_cover.dart` `_detachRemoteScanStatus()`（`:384-387`）`removeListener` + 置空；由 `dispose`/`didUpdateWidget` 调用（`:446` 附近重建）。`RemoteScanCoordinator` 的 `_statuses`/`_viewStates` 是**永久 per-source map**（`:611` 仅 dispose 时遍历清理）—— 没有 ref-count；但因为它是 **per-source（不是 per-widget）**，listener 泄漏风险由各 widget 自己的 `removeListener` 控制 |

# 2. Consumer table

| Consumer | 当前数据源 | Poll / Retry | Durable state? | 可复用机制 | 缺口 |
|---|---|---|---|---|---|
| **comic card（unified remote）** | `repository.readCover` → Rust `remote_cover_read`（读 `remote_cover_variant`/`remote_cover_job`）；`requestCover` 的一次性 `RemoteCoverStateDto` | ❌ **30 × 350ms** 轮询 `readCover`（`:852-868`）+ **8 × 900ms** 重复 `requestCover`（`:583-585`） | ✅ durable（job/variant 表） | `RemoteScanCoordinator` 的 `ValueNotifier` 模式 | **无 per-asset 状态订阅**；7 种状态未接入 UI；非 ready 一律 spinner |
| **comic card（scan 级）** | `RemoteScanCoordinator.statusFor(sourceId)` → `ValueNotifier<RemoteScanStatus?>` | ✅ 事件驱动（`addListener`） | ✅ 由 durable 派生 | ✅ **已在用** | 只覆盖 scan 级，不覆盖 per-asset cover state |
| **comic card（legacy）** | P1-D2：local-only → （online）session → provider | 无轮询 | ✅ | ✅ P1-D2 路径 | 保持不动（E8） |
| **folder representative card** | 与 comic **同一条** unified 路径，`representativeAssetId` 来自 Rust DTO | ❌ 它**自己**那一份 30×350 / 8×900 | ✅ | 同上 | 与同一 assetId 的 comic **各建一套** load/poll ⇒ 状态可分裂 |
| **进度/统计（现状）** | `RemoteScanCoordinator.viewStateFor(sourceId)` → `ValueNotifier<RemoteScanViewState?>`（`remote_scan_models.dart:75-94`：`readyBooks/activeBooks/pendingBooks/retryBooks/blockedBooks/unsupportedBooks/failedBooks/discoveredBooks`） | ✅ 通知驱动（`source_browser.dart:1353`、`source_tree.dart:216` 消费） | ✅ 由 Rust `refresh_status_counts` 聚合 | ✅ **已在用** | "可用 = ready **且** 磁盘字节有效"的 validity；分母语义与"X / 共 Y 本"表述 |

# 3. 两个会改变方案规模的发现

## 3.1 P1-F 的聚合**已经存在，且已是通知驱动**

`RemoteScanViewState` **已包含** F2 要求的全部桶：`readyBooks`（available）、`pendingBooks`+`retryBooks`（waiting）、
`activeBooks`（running）、`failedBooks`、`unsupportedBooks`、`blockedBooks`，以及 `discoveredBooks`（total）。
它由 Rust `refresh_status_counts` 聚合、经 `ValueNotifier` 下发、已被 source browser / tree 消费。

⇒ **不需要为 F 新建第二张状态表，也不需要新的轮询**；F 的工作集中在语义：
① "可用"必须排除 `ready 但磁盘字节缺失`（F1 的 validity）；
② 主进度改为"可用 X / 共 Y 本"且 **Y = durable 库总量**（`discoveredBooks`），terminal error **留在分母**（F3）；
③ 明确 bucket 覆盖不变量（F5）；
④ 明确 progress scope（F6）；
⑤ 若已有 query 足够就复用，不"为接口漂亮"新建 Rust API（F4）。

## 3.2 P1-E 的真缺口是 **per-asset cover state 订阅**，不是机制缺失

机制已有（`ValueNotifier` + per-source map，仓库风格），缺的是：卡片需要"**某 asset 的 durable cover state 变化**"
这一事实的通知，才能做到 E1 的 7 态展示 + E2 去掉两个轮询。按 E3 的模型：

```
durable state changed（Rust 侧 job 状态迁移点）
   → notification（窄原语：asset/source 的 cover state 变了，请重读）
   → consumer re-read durable state（既有 remote_cover_state 读取）
```

notification **不带状态**（E3：payload 不得成为 lifecycle state），因此事件丢失也不会破坏正确性——
下一次 refresh 仍从 durable state 恢复。

---

# 4. 冻结的 E / F 设计（未实施）

## E

| 项 | 设计 |
|---|---|
| **E1 展示状态** | `ready`→封面；`running`→**唯一** spinner；`pending`→"等待获取"（不转圈）；`retry_wait`→"等待重试"（等待态，不 spinner）；`failed`→"获取失败"（不无限转圈）；`unsupported`→"暂不支持"；`blocked`→现有 API 只能给统一 blocked 时显示"暂不可用"（不在 Dart 猜 auth/network/cooldown）；无 job→普通 placeholder（与 pending 区分，不假装 running） |
| **E2 去轮询** | 删除 `:852-868` 的 30×350ms 与 `:583-585` 的 8×900ms；改为订阅 durable state 变化通知 + 重读 |
| **E3 通知≠真相源** | 通知只含 `{source_id, asset_id}`（无状态负载）→ consumer 重读 durable state |
| **E4 assetId 去重** | comic 与 folder representative 同 assetId 时共用同一 subscription key（`representativeAssetId` 已是 canonical assetId，无需解析名字/path） |
| **E5 生命周期** | mount 订阅 / assetId 或 source 变化时 unsubscribe old + subscribe new / dispose 退订 / 快速滚动不泄漏 / 同 asset 多 widget 共享底层 subscription 且各自安全 dispose（ref-count 或既有等价机制；不建永不释放的全局 map） |
| **E6 边界** | 订阅本身不建 session、不 provider request、不 scan、不建 job；取封面仍由 scan/replenisher/explicit request/worker 负责；consumer 只 observe→render |
| **E7 测试** | E-1…E-11（spinner only running / pending / retry_wait / failed / unsupported / blocked / ready 迁移 / failed 迁移 / **no-polling 用 fake clock + 调用计数** / shared asset consumer / dispose） |
| **E8** | 不得破坏 P1-D2：local evidence first、offline legacy disk-first、`local-cache → session → provider`、Family 2 clean miss、unified disk-first |

## F

| 项 | 设计 |
|---|---|
| **F1** | 主进度"可用 X / 共 Y 本"；X = `ready` **且**本地 cover 有效（复用既有 validity helper / P1-B reconcile 语义，不在统计层修 DB）；Y = durable 库总量（`discoveredBooks`），不从卡片数量反推 |
| **F2** | available / waiting(`pending`+`retry_wait`) / running(`active`) / failed / unsupported / blocked；无 job 者由 **durable query 明确定义**归入 waiting/unresolved，禁止 Dart 猜 |
| **F3** | terminal error **留在分母**：273 本（210 ready / 20 waiting+running / 18 failed / 15 unsupported / 10 blocked）→ "可用 210 / 共 273 本"，并单列等待/失败/暂不支持/阻塞；禁止 `210/230`、禁止偷删 unsupported 分母、禁止把 failed 算 available、禁止用 spinner 数当 remaining |
| **F4** | 复用既有 `RemoteScanViewState` 聚合（Rust `refresh_status_counts`）；不让 Flutter 拉全部 job 自行 join、扫卡片、统计 viewport |
| **F5** | 同一快照下 `available + waiting + running + failed + unsupported + blocked + other_defined_bucket` 必须能解释 total；no-job / permanently skipped / local-only source 必须**明确归桶**，不允许 silently disappear；报告给出 invariant |
| **F6** | 先按现有产品语义实现并**明确写出** `progress scope = ...`；folder representative ≠ folder 内全部 comics，不得用 representative state 当 folder 统计 |
| **F7** | 进度刷新复用 E 的状态变化通知 → refresh aggregate；禁止 1s timer / 周期轮询 / refresh loop；允许极短 debounce（不改 durable state、不延迟 worker、不成为定时轮询） |
| **F8** | F-1…F-10（buckets / failed not available / unsupported not available / retry_wait is waiting / ready missing file / denominator / live refresh / folder-comic 同 asset 不双计数 / no polling / large fixture 非 O(cards×polling)） |

---

# 5. 本轮门禁与完整性

| 项 | 结果 |
|---|---|
| 生产代码修改 | **零**（本轮仅只读 grep/read） |
| `git diff --check` / staged | clean / 0 |
| `P1_RECOVERY_INTEGRITY_PASS` | 未受影响（零文件改动） |
| branch / HEAD | `p1-cover-completion` / `1bf2e37`（未变） |
| 临时产物 | 无 |
| 既有 baseline exception | 保留：`cargo clippy` `src/reader.rs:277`；全仓 `flutter analyze` 121 issues；完整 `flutter test` 的 6 个既有失败 |

## Remaining risks（针对 E/F）

1. **E 的通知原语尚未选定落地形态**：要么新增 Rust 侧 job 状态迁移点的窄通知（需 codegen），
   要么复用既有 per-source `ValueNotifier` 粒度。**per-asset** 粒度在当前机制下不存在，
   这是 E 的主要工作量与唯一需要你确认的设计点。
2. F 的"可用"validity（ready 但字节缺失）需要与 **P1-B reconcile** 对齐语义：
   统计层**不得**改 DB，只能等待既有 reconcile；因此计数在极短窗口内可能与磁盘状态短暂不一致。
3. `_viewStates`/`_statuses` 是永久 per-source map（无 ref-count）；若 E 复用该模式做 per-asset 订阅，
   **必须**为 per-asset 引入 ref-count 或等价释放机制（E5 已冻结，但尚未实现）。
4. 115-web / Quark 的 raw-key `present→absent` 仍为 known unrecoverable（P1-D2 冻结）。
5. Release Gate 仍 PENDING（真实 115/Quark 校验、P0 S1–S5、无新增 403/405/429、CDN rate/burst/in-flight
   实测、20/100/300 MiB Range 证据）—— 与 E/F 无关，不得因 P1 完成而误报为可发布。
