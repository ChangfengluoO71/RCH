# 真实库扫描审查（2026-09-19）

**审查对象**：`D:\Documents\RCH`（真实数据根，346→389MB，**被正在运行的应用持续写入**）
**方法**：只读 —— `.timeout 60000` + SQLite `.backup` 一致快照（`quick_check=ok`）+ 静态核对解析链；
真实库本身只跑 `PRAGMA quick_check` 与只读查询，**未写一个字节**。
**快照**：`/d/Temp/dsh-step31/audit.db`（389,201,920 字节）

---

## 0. 运行中的实例

| 项 | 值 |
|---|---|
| 进程 | PID 5212，**12:45:05** 启动（**未重启**） |
| 路径 | `D:\Projects\RCH-p1\app\build\windows\x64\runner\Debug\RCH.exe` |
| exe 构建时间 | 2026-09-18 22:33（**不含** ①/③-1/② 等今日全部改动） |
| pdfium | 已放到同目录（本轮）⇒ **需重启才生效**（`get_pdfium()` 的失败被进程内 `OnceLock` 缓存） |

## 1. 扫描状态：全部终态成功

| 源 | status | mode | gen | 最后成功 |
|---|---|---|---|---|
| `115_1789360028897` | Succeeded | Snapshot | 17 | 12:56:14 |
| `quark_1786277879032` | Succeeded | Snapshot | 24 | 12:51:30 |
| `b1-real-delivery` | **interrupted** | Snapshot | 2 | —（config 仍 `running`） |
| `b1-real-delivery-1789…` ×2 | Succeeded | Snapshot | 1 | 00:21 / 00:22 |

## 2. 封面问题总账（真实库）

| 源 | state | error_code | 数量 |
|---|---|---|---|
| quark | failed | `provider`（旧码） | 275 |
| 115 | failed | **`notFound`** | **225** |
| 115 | failed | `route_missing` | 214 |
| 115 / quark | ready | — | 50 / 30 |

---

## 3. 真问题 ①（新）：115 的 225 个 `notFound` = 扫描结束后**拒绝使用 staging 元数据**

**现象**：138 本漫画的封面永久失败（`notFound`），而它们**确实存在**于当前代际列表。

**证据链（逐条可复算）**：

1. job 引用的路径（`remote_asset_route.logical_path`，内层形 `/…/x.zip/x.zip`）
   在 `library_index` 里 **一行都没有**（`0/225`，INNER JOIN 口径）。
2. 同一路径在 `remote_scan_preview` 里 **229 行全命中**，且 `size>0` **229/229**、
   `asset_kind ∈ {ArchiveFile,ImageFolder,ImageFile}` **229/229** ⇒ 元数据齐备。
3. 但 `cover_source_info` / `cover_route_for_job` 的 preview 分支要求
   `JOIN remote_scan_state s … AND s.status='Running'`；115 的扫描**早已 Succeeded**（12:56）
   ⇒ 该分支被完全跳过 ⇒ 返回 `None` ⇒ `RemoteScanError::NotFound`。
4. 这批漫画在 gen 17 的权威列表 `remote_listing_state` 里 **138/138 本命中**（`IS NOT NULL` 口径）
   ⇒ **不是孤儿**、不该失败。
5. 对照：50 个 `ready` 封面里也有相当一部分是在**扫描运行期间**解析成功的（同样的 preview 路径）
   ⇒ 与"扫描结束后这条路失效"的解释一致。

**根因（经两轮假设被推翻后，由证据确定）**：

1. `notFound` **全部发生在 12:56:11–12:56:29** —— 与扫描发布 gen 17 索引（12:56:11）**同一时刻**。
2. 那一刻解析器的两条跳同时不可用：
   - preview 跳要求 `remote_scan_state.status='Running'`，而扫描正在收尾 ⇒ 该跳关闭；
   - `library_index` 的行此刻尚未落齐 ⇒ 另一跳也空。
   ⇒ `cover_source_info` 返回 `None` ⇒ `RemoteScanError::NotFound` ⇒ 按 `_ => Failed` 记成**终态**。
3. 这些终态的 `long_retry_not_before=NULL` ⇒ 无长期补偿资格 ⇒ **此后永不重试**（卡了 2 小时以上）。
4. 而**今天**这些行的数据完全健康：`library_index` 中
   name / `asset_kind=ArchiveFile` / `size` / `content_fingerprint` / `scan_generation=17` /
   `listing_complete=1` / `deleted=0` **225/225 齐备**，且
   `library_index.path == remote_asset_route.logical_path` **225/225**、
   `library_index.id == job.asset_id` **225/225**。
   ⇒ 失败不是"取不到"，而是"**没人再试**"。

**曾被提出、又被证据推翻的假设**（记录在案，避免重复走弯路）：

- ❌ "`library_index.asset_kind` 老行为空 ⇒ 解析器强匹配失败"：
  失败行（按 id 联接）的 kind **非空**；那 292 行空 kind 是另一批历史行（264 dir + 28 file）。
- ❌ "preview 的 `Running` 门槛是这 225 的直接原因"：
  这些资产**不需要** preview（`library_index` 行齐备）；该门槛是**同一收尾窗口的成因之一**，
  但不是当前失败的直接原因——直接原因是**终态后不再重试**。

**受影响规模**：225 个 job / **138 本漫画**（最小样本 5.6MB ArchiveFile，用于验证）。

## 4. 真问题 ②：`route_missing` 214 行（**不是** bug）

`src/api/remote_scan.rs` 的 **F1/F2**（commit `191da6b`）已把
`failed + route_missing` 判为 **stale orphan** 并从失败桶里摘出。核对该判定与数据的口径一致
（这些 job 的资产在当前列表里确实已无路由）。⇒ 无需处理，仅记录。

## 5. 真问题 ③：真实库里残留在测试源

`b1-real-delivery`（RG-B B-1 harness 产物）现在状态 `interrupted` + config `running`，
和两个 `b1-real-delivery-1789…` 一起留在**用户的真实库**里。
与第 53 轮修掉的 `%APPDATA%` 污染同源（harness 用了真实数据根）。

**残留足迹（只读清点，`source_id LIKE 'b1-real-delivery%'`）**：

| 表 | 行数 | 表 | 行数 |
|---|---|---|---|
| `book_sources` | 3 | `library_index` | 9 |
| `remote_scan_state` / `_config` / `_epoch` | 3 / 3 / 3 | `remote_listing_state` | 6 |
| `remote_cover_job` | 6 | `remote_cover_variant` / `_ref` | 6 / 6 |
| `remote_asset_route` | 9 | `remote_directory_cover` | 3 |

**处置建议**：在应用内「书源管理」删除这三个源（走应用自己的清理路径，能保证跨表一致）；
**不要在应用运行时用 SQL 直写真实库**（应用持有连接与内存状态，会不一致）。

## 6. 真问题 ④：**日志几乎无声**（直接影响"紧盯日志"）

| 文件 | 行数 | 最后写入 | 内容 |
|---|---|---|---|
| `errors.log` | 22,482（158 条错误；10 条 `PanicException`） | **12:30** | 12:30 启动期的 panic/FRB 异常 |
| `scan_diag.log` | 1 行 | **12:30** | `startup_recovered_residual_running=1` |

应用 12:45 启动后**持续运行 2.5 小时**（DB 写到 15:18、产生 225 个封面失败），
这两个日志**没有新增一行** ⇒ 扫描/封面的实际问题在日志里**完全不可见**，
目前只能靠 DB 或 `RCH_PERF_LOG`（本轮新增的 `cover.fetch` / `cover.probe` 事件）。

---

## 7. 已实施与验证（第 55 轮）

| 项 | 内容 | 状态 |
|---|---|---|
| **Fix 1** | `cover_route_for_job` / `cover_source_info` 的 preview 跳去掉 `status='Running'` 要求，改"**最新代际 + 身份守卫**"（`session_epoch<>''` + `source_fingerprint` 一致） | ✅ 已改（消除同类收尾窗口；非这 225 的直接解药） |
| **Fix 2** | reconcile 新增 **(1b) 桶**：重挂**现在确实能解析**（`library_index` 存活 + kind/指纹齐备）的 `notFound`/`route_missing` 终态失败。复用 episode 语义：`long_retry_pending=1`，claim 时消耗 ⇒ **一次**重试，不会无限 | ✅ 已改 + 契约测试（`stale_resolution_failures_are_rearmed_once_after_they_become_resolvable`） |
| **诊断通道** | Rust 侧新增 `remote_scan::diag` → `<数据根>/scan_diag.log`（扫描终态 / 封面失败 / reconcile 结果；UTC+`Z`，只写安全枚举码与 12 位短哈希） | ✅ 已改 + 单测 |
| **清理** | 真实库里的测试源 `b1-real-delivery*` | ⏳ 待你在应用内删除（**不能在应用运行时写真实库**；残留清单见 §5） |

**Fix 2 的端到端验证（快照副本 + 真实 115 账号，只写副本）**：

- 夹具：把 225 个 `notFound` 中除目标外的 `long_retry_consumed` 置 1
  （只留 1 个额度，避免一次重挂 64 本＝数 GB 流量）。
- 结果：`compensation_promoted=8`（= 目标 1 + 真的可解析的 orphan 7，**与谓词口径完全一致**）、
  `ready 50 → 60`（**10 个原先失败的封面全部产出**）、`notFound 225 → 224`、
  `route_missing 214 → 205`、`cover.fetch=10`、**`CoverFailures=0`**、
  `CoverBytesFetched=11.9MB`（10 枚，走有界窗口而非整包）。
- 诊断通道同时落行：
  `2026-09-19T07:29:05Z cover_reconcile source=115_1789360028897 promoted=8 cleared=0 created=0 truncated=false claimable=true`

## 8. 顺带建议的处置

1. **`scan_diag.log` 补关键失败** —— 已实施（见上表"诊断通道"）✓
2. **测试 harness 改用临时数据根** —— 待办（第 53 轮已修 `cache_root` 测试隔离；
   RG-B harness 用的是 Dart 侧真实根，需另改）。
