# 封面状态文案闪烁 —— 只读静态审计（审计员产出，未改动任何产品代码）

- 仓库：`D:/Projects/RCH-p1` @ `56fffd9`（分支 `p1-cover-completion`），工作区脏文件仅接手前遗留的
  `app/{linux,macos,windows}/flutter/generated_*`。
- 数据：`D:/Documents/RCH/database.db`（只读打开）、`scan_diag.log`、`errors.log`。
- 本次审计**只读**：无 build、无启动、无 git 写、无 DB 写。

## 1. 症状与可复现条件

- 症状：卡片/墙上的封面状态文案在「未缓存 ↔ 等待扫描/等待获取」间来回跳，久不稳定。
- 触发条件（代码 + 日志可复现）：同一 source 上，**会话级 reconcile 每 3–5 分钟触发一次真实的
  durable transition + revision bump**；卡片每次收到 revision 就清空 `_coverState` 并重读。
- 关键事实（必须先说清）：**「等待扫描」不是卡片文案**。全仓三处来源：
  - `app/lib/ui/remote_scan_status.dart:12`（`RemoteScanStatusPanel.idleMessage` 默认值）
  - `app/lib/ui/source_browser.dart:1361`（该面板在浏览器页被传入 `'等待扫描'`）
  - `app/lib/ui/source_tree.dart:38`（书源树 idle 文案）
  卡片侧 `pending` 的文案是 **`'等待获取'`**（`comic_cover.dart:1042`），「未缓存」是
  `ComicCover.uncachedPlaceholder()`（`comic_cover.dart:299-321`，由 `:1049` 在 `label == null` 时兜底）。
  ⇒ 用户看到的两句话来自**同屏两个不同组件**，而两者都被同一条 cover-revision 链路驱动（见 §2/§4）。

## 2. 关键代码路径（file:line + 作用）

Rust 状态机 / 表：

- `app/rust/src/remote_scan/cover_state.rs:87-131` —— `resolve_upsert_state`，状态迁移唯一规则表
  （`Demand` 对 `failed/unsupported/blocked/retry_wait` 一律**保留**）。
- `app/rust/src/remote_scan/catalog.rs:320-400` —— `cover_state_for`：先查 `remote_cover_variant`
  （按 `updated_at DESC`），再查 `remote_cover_job`，按 `updated_at` 谁新谁说了算。
- `app/rust/src/remote_scan/cover_store.rs:1170-1302` —— `reconcile_missing_covers_for_source_on`
  （缺口补齐）：`:1221-1224` 以 `NOT EXISTS(remote_cover_job ... selection_revision=?2 AND profile=?3)`
  判定"缺口"，`:1265-1282` 用 **常量 `DEFAULT_SELECTION_REVISION`/`DEFAULT_COVER_PROFILE`** 建 `pending`
  job，`:1291` bump revision；`:1237-1240` 超过 `max_jobs=64` 置 `truncated`。
- `app/rust/src/remote_scan/cover_store.rs:789-790` —— `DEFAULT_SELECTION_REVISION="default"`、
  `DEFAULT_COVER_PROFILE="340x480@1"`（**与设置无关**）。
- `app/rust/src/remote_scan/cover_store.rs:935-1156` —— `reconcile_cover_compensation_for_source_on`：
  `:1115-1130` 把 `failed`（含 `cover_read_budget_exceeded`）**翻回 `pending`** 并 bump revision。
- `app/rust/src/remote_scan/cover_service.rs:110-209` —— `read_cached_cover`：**读路径会写 durable state**。
  `:159-182` 对"`state='ready'` 但字节读不到"的记录执行 `ready → pending`；`:183-203` bump
  `view_revision`；`:208` 无条件 `notify_cover_revision`。
- `app/rust/src/api/remote_cover.rs:596-638` —— `remote_cover_state`：只读；`:612-628`
  以 (source,asset,**selection_revision,profile**) 的 EXISTS 判定 "no job"，否则走 `cover_state_for`。
- `app/rust/src/api/remote_cover.rs:458-483` —— `remote_cover_read`：缓存 miss 时
  `:476` `wake_cover_worker_for_source`（读路径会**旁路唤醒 worker**）。
- `app/rust/src/api/remote_cover.rs:676-691` —— `remote_cover_reset_to_current_profile`：
  全仓**唯一**的 `DELETE FROM remote_cover_job`（`:684`，按 profile 清非当前档）。

Dart 读取 / 卡片 / 文案：

- `app/lib/store/remote_cover_repository.dart:136-158` —— `readState`（STATE-READ，带 selection+profile）。
- `app/lib/store/remote_scan_coordinator.dart:225-260` —— `startCoverRevisionWatch` /
  `_applyCoverRevision`（`durable <= lastSeen` 去重，否则推进 notifier）；
  `:639-648` `_setStatus` 同时写 `_statuses` 与 `_viewStates`（面板文案由此驱动）。
- `app/lib/ui/comic_cover.dart:407-418` —— `_onCoverRevisionChanged`：`_future=null`、
  **`_coverState=null`**、`_maybeLoad()`、`setState`。
- `app/lib/ui/comic_cover.dart:933-966` —— wake 后 `readState`，`_coverState = current?.state`（`:950`）；
  非 ready 直接抛异常（`:961-964`）⇒ 由 `:617-619` 落回 `_coverState`。
- `app/lib/ui/comic_cover.dart:879-897` —— `_readAnyCachedCover`：跨档回退，最多 **4 次**
  `readCover`（`:885-891`），每次 miss 都可能触发上面的 `ready → pending` 写入。
- `app/lib/ui/comic_cover.dart:856-870` —— `_profileCandidates`：本档 → 340 → 510 → 170。
- `app/lib/ui/comic_cover.dart:1040-1049` —— `_placeholder()`：`pending→等待获取`、
  `retry_wait→等待重试`、`failed→获取失败`、`unsupported→暂不支持`、`blocked→暂不可用`；
  **`ready` 也落到 `_ => null` ⇒ 显示「未缓存」**。
- `app/lib/ui/comic_cover.dart:1012-1013` —— `running` 或 `_future==null` 时是 spinner（不是文案）。
- `app/lib/ui/comic_cover.dart:299-321` + `source_browser.dart:1851-1857` —— 「未缓存」两处用法。

## 3. 原始证据（命令 + 要点）

```bash
sqlite3 -readonly "D:/Documents/RCH/database.db" ".timeout 8000" \
  "SELECT profile,selection_revision,state,COUNT(*) FROM remote_cover_job GROUP BY 1,2,3;"
# 340x480@1|default|pending|64      170x240@1|default|pending|12      170x240@1|default|running|1
sqlite3 -readonly ... "SELECT generation,state,COUNT(*),datetime(MIN(updated_at)/1000,'unixepoch'),datetime(MAX(updated_at)/1000,'unixepoch') FROM remote_cover_job GROUP BY 1,2;"
# 104|pending|12|07:33:24|07:33:25   105|running|1|07:43:11   108|pending|64|07:43:11|07:43:11
sqlite3 -readonly ... "SELECT key,value,datetime(updated_at/1000,'unixepoch') FROM app_settings WHERE key IN ('coverQuality',...);"
# coverQuality=low（映射 170x240@1），updated 07:43:19
sqlite3 -readonly ... "SELECT demand_kind,COUNT(*) FROM remote_cover_job GROUP BY 1;"
# visible|13   background|64
```

- **DB 与常量直接冲突（硬证据）**：`coverQuality=low` ⇒ 期望档位 `170x240@1`
  （`api/remote_scan.rs:478-493`），但 `reconcile` 建的 **64 行落在 `340x480@1`**（`background`、
  gen 108、`updated_at` 全为 `07:43:11`），与 `scan_diag.log` 的
  `2026-09-21T07:43:11Z cover_reconcile ... promoted=0 cleared=0 created=64 truncated=true` 同秒。
  换言之：**扫描侧写 340 档，卡片按 170 档读**，缺口判定（`cover_store.rs:1221-1224` 只认
  340+default）永远看不到卡片实际使用的那一组键。
- 12 行 `visible`+170 档（gen 104）证明卡片确实在 170 档下工作。
- `scan_diag.log`：`cover_reconcile` 共 26 次，**26 次都真的改了 durable truth**
  （`grep -v "promoted=0 cleared=0 created=0"` 仍得 26）⇒ 每次会话事件都 bump revision + wake。
- `scan_diag.log`：`cover_fail` 281 条，全部 `state=failed`、`code=cover_read_budget_exceeded`；
  attempt 分布 `1:61 2:112 3:91 4:13 5:4`，单 asset 最高 **14 次**（`2f53bda10e8f`）
  ⇒ 同一本书的状态在 failed↔pending↔running 之间长期来回。
- `scan_terminal`：`incremental` 间隔 2–15 分钟（gen 104→108），与用户"每 3–5 分钟一次"一致。
- `errors.log`：`grep -n "comic_cover" errors.log` → 136 处
  `setState() or markNeedsBuild() called when widget tree was locked`，栈为
  `_ComicCoverState.initState → _maybeLoad → VisibleCoverScheduler.acquire → _load →
  cloud115SessionFor → RemoteSessionSuccessHub.emit → RemoteScanCoordinator._startShared →
  _setStatus → ValueNotifier → ValueListenableBuilder<RemoteScanViewState?>`。
  即：**卡片挂载/回收期的封面加载会同步推 `_viewStates`，在 `finalizeTree/_unmountAll` 期间
  标记面板需要重建**（`RemoteScanStatusPanel` 正监听该 notifier）—— 帧级重建抖动，属实证噪音。
  （注：该栈行号来自更早构建，与 HEAD 行号不一一对应。）

## 4. 根因判断（置信度 / 反证）

按用户列出的假设逐条裁定：

- **(a) 两个真相来源交替 —— 部分成立，但不是"无行=未缓存"**。置信度 **中**。反证：卡片一旦
  `requestCover` 过（`comic_cover.dart:381/967`），`remote_cover_state` 只要该键有行就返回 `Some`，
  不会返回 `null`；`_coverState` 变 `null` 的**唯一真实路径**是 `:415`（revision 到达时主动清空）。
  该清空窗口极短（同一回调内 `_maybeLoad` 同步入队），单帧级。
- **(b) 不同 profile / selection 读状态 —— 成立，且是本症状最硬的实证**。置信度 **高**。
  证据即 §3 第一条（`low` 设置 + `340` 档 64 行 + `created=64`）。
  后果有二：① 缺口补齐永远补不满（每个会话事件都重造 64 行、bump revision）；② 卡片"图来自 A 档、
  文案来自 B 档"。反证：`selection` 两侧都是 `default`（`bookCoverPage=0`、无 crop 时），
  所以**差异只在 profile 一侧**，不是 selection。
- **(c) 每次扫描删建任务 —— 不成立**。置信度 **高**。全仓 `DELETE FROM remote_cover_job` 仅 4 处，
  3 处是测试（`api/remote_scan.rs:4014/4280/4471`），生产仅
  `remote_cover_reset_to_current_profile`（`api/remote_cover.rs:684`，用户手动换档才跑）。
  reconcile 只做 UPDATE/UPSERT（`cover_store.rs:984-1135`、`:1272-1282`）。
  ⇒ `promoted/cleared/created/truncated` **不含删行语义**。
- **(d) 跨档回退在缓存被清空后失效 —— 成立，且是文案跳变的直接机制之一**。置信度 **高**。
  `read_cached_cover`（`cover_service.rs:159-203`）在 `ready` 但字节缺失时把 job 改回 `pending` 并
  bump revision；`_readAnyCachedCover`（`comic_cover.dart:885-891`）**每次卡片加载最多发 4 次**
  这种读 ⇒ 卡片自己就能把状态从"已就绪"打成 `pending`（文案变「等待获取」），随后
  `:951` 判 `ready` 走读图分支、读不到、`:988` 抛 `ready` 异常，而 `_placeholder()` 对 `ready`
  无分支 ⇒ 显示**「未缓存」**（`:1047-1049`）。这就是"同一时刻图与文案互相矛盾、且来回跳"的成因。
- **(e) 前端轮询 / 重复 setState —— 轮询已删，但"重复重建"仍在，且跨组件**。置信度 **中**。
  反证：`comic_cover.dart:927-937/637` 明确删掉了 30×350ms 与 8×900ms 轮询，
  `_scheduleUnifiedRetry()` 已是空函数；`requestCover` 由 `_coverRequestIssued` 保证一次（该口径成立）。
  但 `errors.log` 的 136 条 locked-tree 栈证明：卡片加载会经 `RemoteScanCoordinator._setStatus`
  在**锁定帧内**推 `_viewStates`（`:639-648`），使
  `RemoteScanStatusPanel`（`source_browser.dart:1349-1363`，idle 文案正是**「等待扫描」**）被反复标脏。
  ⇒ 「未缓存」（卡片）与「等待扫描」（面板）在同一条链路上各自闪烁。

**综合判断（根因，置信度 高）**：卡片状态文案没有唯一真相，而是被三个不同所有者按各自口径重写：
① `reconcile` 用**常量档位 340** 写 durable job（与设置的 170 档错位，且每会话事件重复造 64 行 + bump revision）；
② 读路径 `read_cached_cover` 会**写** `ready→pending` 并 bump revision（读即改状态）；
③ Dart 侧在 revision 到达时**清空 `_coverState`**，而 `_placeholder()` 对 `ready`/`null` 一律兜底成「未缓存」，
且与「等待扫描」面板共享同一 revision/status 推送链路。三者叠加 ⇒ 文案在两个"等待"语义间来回跳、且与图不同步。

## 5. 最小修法与验收口径

按"改动最小、风险最低、可量化"排序（**均未实施，待授权**）：

1. **统一档位来源（根因修复，最小改动）**：`cover_store.rs:1269-1270` 的缺口补齐不再用常量，
   改为读 `app_settings.coverQuality`（复用 `api/remote_scan.rs:478` 的 `cover_quality_profile_on`），
   或把 `reconcile` 的 profile 作为参数由调用方传入。风险：低；影响面仅封面排队。
2. **`_placeholder()` 补 `ready` 分支**（`comic_cover.dart:1041-1049`）：`'ready'` 应显示
   "处理中/重取中"之类的**显式**文案，而不是兜底「未缓存」。风险：极低。
3. **读路径别再改状态**：`read_cached_cover` 的 `ready→pending` 对账改为**不在卡片读路径**触发
   （或加"仅当字节过期超过 N 秒"的节流），避免"读一次就把状态打回 pending"。风险：中（牵涉 P1-B 对账语义）。
4. **卡片清空策略**：`_onCoverRevisionChanged`（`:407-418`）不清 `_coverState`，改为"读到新状态再替换"
   （保留旧文案直到新证据到达），消除单帧 null 窗口。风险：低。
5. **面板与卡片解耦**：`RemoteScanCoordinator._setStatus`（`:639-648`）在 locked frame 内不直推
   `ValueNotifier`，改为 `scheduleMicrotask`/下一帧；同时给面板的 idle 文案与卡片文案做术语区分
   （「等待扫描」只用于扫描，封面用「等待获取」）。风险：低。

**可量化验收口径（建议）**：

- 同一 (source, asset, profile=当前设置档) 的卡片文案，在 **60 秒内**不得发生 **>1 次**非"-`ready`→有图"
  的文案切换（用测试注入 revision + fake repository 断言，`app/test/` 已有同类 consumer 测试可扩展）。
- `cover_reconcile ... created=` 在**同一 source 连续 3 次会话事件**中必须恒为 **0**（前提：缺口已被前一轮补满）。
- `remote_cover_job` 中 `profile <> 当前 coverQuality 档` 的行数在稳态下为 **0**。
- `errors.log` 中 `widget tree was locked` 的 `_ComicCoverState` 栈**消失**（当前 136 条）。
- `grep -c cover_fail scan_diag.log` 的**单 asset 最高重试次数**不随会话次数单调增长。

## 6. 未验证项

- **未做屏幕取证**：本次为只读静态审计，无运行、无录屏，"用户实际看到的是哪一对字符串"
  仍是推断（「等待扫描」只能来自 `RemoteScanStatusPanel`/`source_tree`，卡片侧不存在该文案）。
  建议先让用户确认：闪烁的是**封面格子内的文字**，还是**页面上方扫描状态条**的文字。
- **`errors.log` 栈行号属于更早构建**，与 HEAD 行号不一一对应（仅用于定位组件，不用于定位行）。
- **`07:43:11` 那 64 行的写入者**只由"同秒日志 + 常量档位"推定，未逐条比对
  `remote_cover_job` 的 `job_key` 编码（`cover_model.rs` 的 `encode()`）以排除第二写入方。
- **DB 是运行中快照**（`journal` 未合并时可能撕裂），本次仅取聚合与只读单行查询，未做一致性校验。
- **`_readAnyCachedCover` 是否真的在本机触发了 `ready→pending`** 需运行时埋点确认
  （`cover_service.rs:183` 的 `job_changed > 0` 未在 `scan_diag.log` 打印）。
