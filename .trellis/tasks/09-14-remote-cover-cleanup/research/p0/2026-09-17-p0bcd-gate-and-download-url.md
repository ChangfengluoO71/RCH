# P0-B / P0-C / P0-D 实测对照与实施记录

> 阶段：**P0 · 阅读速度修复**（审阅通过，2026-09-17）｜ 未 commit / 未 push / 未进入 P1
>
> 同口径基线见 [`2026-09-17-p0a-baseline.md`](2026-09-17-p0a-baseline.md)。
> 所有 before/after 都由**同一套测量装置**（`app/rust/tests/p0_baseline_read_speed.rs`）产出，
> 驱动的是**未改动的生产代码路径**，只把 HTTP 端点换成 localhost。

---

## 0. 一句话结论

读速的人工下限**已经消失**：9 次 CDN Range 读从 **2259 ms → 757 ms**，流式打开一本 CBZ 从
**2006 ms → 505 ms**；后台负载下前台的**单次**门控等待峰值从 **1248 ms → 247 ms**（正好一个令牌
间隔），说明前台不再排在整条后台队列之后。**残留**：持续后台负载下前台页延迟总和仍是空载的
**2.4×**，机制是"共享的 4/s 持续速率被后台吃满"——这一条的最终档位按你的冻结决策 2 交给真机对照测量。

---

## 1. 本轮冻结边界（逐条对照）

| 冻结项 | 本轮做法 |
|---|---|
| P0 单独闭环；每步可归因、不一次改完 | 分 A→B→C→D 四次独立改动，每次都单独测量 |
| CDN Range 独立优先级令牌桶；**不冻结 20/s + burst 10 为默认** | 持续速率默认**仍是 4/s**；burst/并发/速率全部可用环境变量切换档位，无需重编译 |
| API 通道继续遵守现有预算 | `gate` 仍是 `RateGate::fixed_interval`，语义与改造前**逐字等价**（有测试锁定） |
| CDN 与 API 完全解耦 | 两个独立实例 + 独立在途上限；有测试断言 CDN 不排在 API 之后 |
| 本地磁盘缓存不经过网络 RateGate | 未被本轮触碰；有测试断言命中时 0 请求、0 门控等待 |
| 速率与并发分开治理 | `GateLimit{per_sec, burst, max_in_flight}` 三个正交参数，独立测试 |
| 优先级贯穿到单次网络请求 | 线程本地优先级 + 门控按优先级排队；前台只抢"下一个许可"，不打断在途；等待可取消；aging 防饿死 |
| 不机械实现 `Condvar`，先确认同步/异步模型 | **已确认：同步阻塞链路**，见第 2 节 |
| 先写能证明旧行为错误的测试 | 见第 3 节的"改造前红/绿"记录 |
| 不改默认 CDN 档位为 20/s | 默认 4/s；20/s 仅作为可用环境变量选择的实验档 |

---

## 2. 先确认同步/异步模型（冻结决策 5）

**结论：这条链路是同步阻塞的，因此 `Condvar` 是与运行时匹配的原语；`tokio::sync::Notify/Semaphore`
在这里用不了（非 async 上下文不能 `.await`）。这是先确认模型后得出的，不是习惯复制。**

| 证据 | 位置 |
|---|---|
| `read_at` 是同步 trait 方法，无法 `.await` | `app/rust/src/source/mod.rs:65`（`pub trait ByteSource: Send + Sync`） |
| 预取用 OS 线程 | `app/rust/src/reader.rs:383`（`std::thread::spawn`） |
| 既有 governor 就是 `Mutex` + `Condvar` | `app/rust/src/reader.rs:51-56` |
| provider 客户端是 `reqwest::blocking` | `app/rust/src/source/cloud115.rs`（`fn http_client()`） |
| async 只出现在 FRB 边界 | `app/rust/src/api/source.rs`（`tokio::task::spawn_blocking` 桥接） |

---

## 3. P0-B：API/CDN 门控解耦 + 等待模型修复

### 3.1 改了什么

| 文件 | 改动 |
|---|---|
| `app/rust/src/source/gate.rs`（新） | `RequestPriority` 唯一定义、线程本地优先级作用域、`CancelSignal`（代际取消）、`GateLimit`、`RateGate`（令牌桶 + 四优先级队列 + 独立在途上限 + aging）、`GateGuard`（drop 归还名额） |
| `app/rust/src/source/mod.rs` | 删除旧 `RateGate`（持锁 sleep）；登记 `gate` / `singleflight` 模块；门控等待按 API/CDN + 前台/后台分桶 |
| `app/rust/src/reader.rs` | 改为复用 `source::gate::RequestPriority`；`load_claimed` 用 `with_priority` 标注整段读页工作 |
| `app/rust/src/source/cloud115.rs` | CDN 门改为令牌桶；`read_range_url` 走可取消的优先级许可；许可持有到响应体读完；WAF 标记同时取消等待者 |
| `app/rust/src/source/{quark,baidu}.rs` | 门改为 `fixed_interval`（语义等价）；许可按次持有 |
| `app/rust/src/api/remote_scan.rs` | 封面任务标注 `Cover`；**网络许可只覆盖网络阶段，解码移出许可** |
| `app/rust/src/remote_scan/engine.rs` | 目录发现标注 `Scan` |

**关键语义变化（三条）**

1. **突发与持续速率分离**：旧实现是"每 `1/rate` 秒才准发一个"，请求数本身就是延迟下限；
   新实现是令牌桶——突发额度内连发，超出后回到持续速率。
2. **等待不持锁**：`Condvar::wait_timeout` 分片等待，等待期间释放互斥量；分片边界重新评估
   优先级、aging 与取消信号。
3. **前台只抢下一个许可**：已在途的请求不被打断；aging = 等待越久有效优先级越高（最多升到
   Foreground），同级按 ticket 序号 FIFO，因此后台**有界前进**。

### 3.2 测试（先红后绿）

改造前，旧 `RateGate` 持有 `Mutex` 并 sleep，**没有优先级、没有取消、没有在途上限、没有可查询状态**。
下表是"旧行为 vs 新契约"的对照，新契约为 `source/gate.rs` 中的 9 个用例：

| 用例 | 改造前 | 现在 |
|---|---|---|
| `foreground_waiter_is_admitted_before_an_older_background_waiter` | 按到达顺序，先排队的 Cover 先拿到 | Foreground 先拿到 |
| `background_is_not_permanently_starved_by_a_foreground_flood` | 无 aging，洪泛可永久饿死后台 | aging 下有界（< 1 s） |
| `waiting_request_can_be_cancelled_without_waiting_for_the_token` | 不可取消，必睡满间隔 | 取消后 < 600 ms 返回 |
| `snapshot_stays_responsive_while_another_thread_is_waiting` | 查询会被睡着的线程挡住 | 微秒级返回 |
| `in_flight_cap_is_independent_of_the_rate` | 无独立并发上限 | 不限速也能限并发 |
| `burst_lets_one_page_of_windows_through_without_per_request_spacing` | 6 个窗口 = 1250 ms | 突发放行；第 7 个仍守 250 ms |
| `gates_are_independent_so_api_waits_never_block_cdn_reads` | 已解耦（回归锁定） | 保持 |
| `fixed_interval_limit_reproduces_the_old_spacing_exactly` | — | **锁定 API 通道零语义变化** |
| `priority_scope_is_thread_local_and_restored` | — | 线程本地作用域正确恢复 |

**开发中我自己踩的两个坑（如实记录）**：两个用例最初挂死，原因都是**测试设计错误**——
`max_in_flight=1` 时先持住许可又去要第二个、以及在归还许可前 join 被卡住的线程。反过来这恰好证明
在途上限真的在生效。已修正测试，不是放宽产品行为。

### 3.3 实测对照（同一装置，9 次 Range 读）

| 指标 | 基线 | P0-B 后 | 变化 |
|---|---|---|---|
| 总 wall time | **2259 ms** | **757 ms** | **−66.5%** |
| 理论下限 `(n-1)×250ms` | 2000 ms | — | 旧下限已消失 |
| 服务端到达时刻（前 6 个） | 249 / 250 / 250 / 250 / 250 ms 递进 | **6 / 6 / 6 / 6 / 6 ms**（突发） | 逐请求摊平消失 |
| 服务端到达时刻（第 7~9 个） | 1250 / 1501 / 1751 / 2001 ms | 250 / 500 / 750 ms | **持续速率仍被守住** |
| 门控等待总量 | 2 197 165 µs | 700 368 µs | −68% |
| 并发 9 次：服务端最大并发 | **1**（彻底串行） | **2**（在途上限内真实并发） | 并发被解锁且有界 |
| 并发 9 次 wall time | 2259 ms | 759 ms | −66.4% |

### 3.4 实测对照（真实阅读链路：`Reader` → `ZipBook` → `SourceReader` → `read_range_url`）

| 指标 | 基线 | P0-B 后 | 变化 |
|---|---|---|---|
| 流式打开 4 页 × 1.5 MiB CBZ（`open_ms`） | **2006 ms** | **505 ms** | **−74.8%** |
| 磁盘命中后重新流式打开（`reopen_ms`） | 1005 ms | **13 ms** | **−98.7%** |
| 重新打开的 provider 请求数 | 6 | 4 | −33% |
| 单页延迟（1.5 MiB） | [1007, 1751, 250] ms | [1018, 1492, 747] ms | 同量级（见下） |
| 磁盘命中：CDN 请求 / 门控等待 | 0 / 0 | **0 / 0** | 既有正确行为**未退化** |

### 3.5 前台 vs 后台（关键机制指标）

| 指标 | 基线 | Final | 说明 |
|---|---|---|---|
| 前台**单次**门控等待峰值 | **1248 ms** | **247 ms** | 正好一个令牌间隔 ⇒ 前台只等"下一个许可" |
| 空载下前台单次峰值 | （无优先级口径） | 253 ms | 与有负载时同量级 ⇒ 负载不再是主因 |
| 前后台页延迟总和比 | 3.22×（3 页同口径） | **2.41×**（8 页，首次 3 页为 1.31×） | 见第 7 节残留 |

> **口径诚实说明**：
> - 基线的 1248 ms 是"当次运行中所有线程的门控等待峰值"——改造前 CDN 门根本没有优先级口径，
>   后台请求与前台请求在门控眼里完全一样，所以这个峰值就是前台可能遭遇的最坏情况。
>   Final 的 247 ms 是新增的**前台专桶**峰值。两者测的是同一件事（前台最坏等待），但计数器不同源。
> - 总和比在基线是 3 个样本、Final 是 8 个样本，绝对值不可直接比；因此我另给了 Final 首次 3 页
>   的同口径值 1.31×。**我不用 5.11×→2.41× 这种跨口径对比来充数。**

---

## 4. P0-C：115 取链按 pickcode 合并

### 4.1 改了什么

| 文件 | 改动 |
|---|---|
| `app/rust/src/source/singleflight.rs`（新） | 通用按 key 合并：同 key 只跑一次、异 key 不阻塞、leader 失败一致传播、不缓存结果 |
| `app/rust/src/source/cloud115.rs` | 删除 `downurl_lock: Mutex<()>`；`downurl` 改走 `SingleFlight<String, Result<DownloadInfo, String>>`；失败不写缓存；错误文案原样上抛（`scan_error`/`scan_io_error` 是文本分类，语义不变） |

**为什么这是必要的**：旧锁是**跨 pickcode 的全局锁**，且覆盖 `gate.wait()`（最多 667 ms）+ 一次
HTTP POST。后台给漫画 A 取链时，阅读打开漫画 B 会在同一个锁上等满这段时间。

### 4.2 测试

`singleflight` 5 个用例（同键合并 / 异键并发 / leader 失败一致传播且不重试 / slot 不泄漏 /
follower 上报等待时长）+ `cloud115` 新增 2 个用例：

| 用例 | 断言 |
|---|---|
| `web_downurl_cache_expires_after_ttl` | 超 TTL 的记录不再被返回；未过期仍可用 |
| `web_downurl_coalesces_same_pickcode_without_serialising_other_pickcodes` | 同 pickcode 6 并发**只执行 1 次**；4 个不同 pickcode 各 150 ms 并发完成 < 500 ms（若仍串行则 ≥ 600 ms） |

既有 17 个 115 用例全部保持通过（含缓存失效、WAF 冷却短路）。

### 4.3 实测

P0-C 不改变 CDN Range 通道，因此对第 3.3/3.4 节的读数无影响（Final 与 P0-B 后一致，差异在噪声内）。
它的收益在**取链侧**：不再有跨文件串行化。真机侧的取链次数/等待需按第 6 节流程用
`DownUrlRequests` / `DownUrlCacheHits` / `DownUrlCoalesced` / `DownUrlLockWaitUs*` 采集。

---

## 5. P0-D：夸克直链缓存 + 合并

### 5.1 先核对契约（你的要求：不要照搬 115）

前置审计（只读）确认了四件事，直接决定了实现：

| 审计结论 | 对实现的影响 |
|---|---|
| 夸克直链**有效期在代码里没有任何依据**（全仓库唯一的直链 TTL 是 115 的 300 s） | TTL 取保守值 120 s，并明确标注"由真机测量校正" |
| `__puus` 在**任意一次** request 中静默轮换，而直链请求带的是**当前** cookie；"签名绑定 cookie 哪一部分"**无代码依据** | 保守策略：记录取链时的 `__puus` 代际，代际不同一律不复用；**且只在 `__puus` 真的变化时才推进代际**（否则无变化的 Set-Cookie 会让缓存永不命中） |
| 403 是唯一的"直链失效"信号；`read_range_url` 403 → `PermissionDenied` | 403 时失效**共享**缓存（只清 `QuarkFile` 自己的 dlink 不够：扫描/封面走 adapter，读的是共享缓存） |
| 瞬时错误（超时/429）不该被缓存 | 只缓存成功结果 |

### 5.2 改了什么

`app/rust/src/source/quark.rs`：新增 `DlinkCache`（fid → 直链，带 TTL + cookie 代际 + **LRU** 容量上限
256）、`dlink_flight: SingleFlight`、`cookie_epoch: AtomicU64`；`downlink` 拆成"缓存 → 合并 → 真实取链"，
只有成功才写缓存；`QuarkFile::read_at` 与 `download_to_raw_cache` 的 403 路径改为先失效共享缓存再重取。

> 注：115 的缓存淘汰是 `HashMap` 任意键（不是 LRU），夸克这里刻意**不照搬**，用 LRU。

### 5.3 测试（5 个新用例）

| 用例 | 断言 |
|---|---|
| `dlink_cache_hits_expires_and_invalidates` | 命中 / 超 TTL 丢弃 / 主动失效 |
| `dlink_cache_is_discarded_when_the_cookie_epoch_changes` | 代际变化不复用；**无变化的 Set-Cookie 不推进代际** |
| `dlink_cache_is_lru_bounded` | 容量有界；淘汰最旧；最新存活 |
| `dlink_coalesces_same_fid_without_serialising_other_fids` | 同 fid 6 并发只执行 1 次；4 个不同 fid 150 ms 并发完成 < 500 ms |
| `failed_dlink_leader_propagates_without_stampede_or_poisoning` | 5 个调用者都拿到同一错误、只执行 1 次、**失败不写缓存** |

夸克既有 6 个用例保持通过。

---

## 6. 最终参数与选择依据

### 6.1 本轮采用的默认值

| 参数 | 默认值 | 依据 |
|---|---|---|
| CDN 持续速率 `per_sec` | **4.0（未变）** | 你冻结的 baseline 档位；不在本轮擅自提高 |
| CDN 突发 `burst` | **6.0（新增）** | = 一页 1.5 MiB / 256 KiB 预读的窗口数，使单页不再被逐请求摊平。**这是本轮唯一需要你签字的默认值变化** |
| CDN 在途上限 `max_in_flight` | **2（新增）** | 独立于速率；实测把服务端最大并发从 1 提到 2 且保持有界 |
| aging | **1000 ms** | 后台有界前进（< 1 s 保证，有测试） |
| API 门 | **不动** | 115 web 1.5/s、115 app 1.5/s、夸克 2/s、百度 5/s，全部 `fixed_interval`，有测试锁定等价 |

### 6.2 档位切换（无需重编译，供你的真机对照测量用）

```powershell
$env:RCH_CDN_RATE_PER_SEC = "8"    # 4 / 8 / 12 / 20
$env:RCH_CDN_BURST        = "6"    # 1 可退回旧的固定间隔语义
$env:RCH_CDN_MAX_INFLIGHT = "2"
$env:RCH_PERF_LOG         = "D:\Temp\rch-perf.jsonl"   # 必须用 Windows 路径
$env:RCH_PERF_TAG         = "rate-8"
```

> 踩坑记录：`RCH_PERF_LOG` 传给原生 Windows 进程时**必须用 Windows 路径**；Git Bash 的 `/tmp/...`
> 会让原生进程建目录失败、静默不写事件流。

**停止条件（任一命中即停止升档并回退上一档）**：出现 403 / 405 / 429、`WafCooldowns > 0`、
下载线程错误、异常断链、错误率上升。若 8/s 或 12/s 已达性能平台，**不为了对齐计划里的 20/s 继续提高**。

---

## 7. 残留问题（带证据，本轮**未**修）

### 7.1 【高】压缩包打开路径存在约 4000× 读放大

**这是本轮最重要的新发现，也解释了基线里 `open_ms = 2006 ms` 的另一半。**

- **现象**：项目自带测试 `document::zip::tests::opening_many_pages_does_not_fetch_every_local_header`
  断言"打开 40 页 ≤ 8 次 Range"，实测 **81 次**，**当前为失败**。
- **归属证据**：我用 A/B 证明它与本轮埋点无关——把 `SourceReader` 的埋点整段摘掉后，实测**同样 81 次**。
  该测试在本轮开始前就是红的（属 in-flight 变更集的回归）。
- **根因**（读 `zip` crate 源码 + 事件流定位）：`ZipArchive::new` 的
  `central_header_to_zip_file` 对**每个条目**都调用 `find_data_start` 去读该条目的**局部文件头**，
  然后 seek 回中心目录继续。而 `SourceReader` 只有**一个**预读窗口，这种"中心目录 ↔ 页数据"
  来回交替让窗口每次失效 ⇒ **每条目 2 次 `read_at`**。
- **放大倍数（实测事件流）**：40 次 `read_at` 的 `len = 262144`（`requested` 只有 30 字节！）⇒
  打开一本 12 MiB / 40 页的 CBZ，**实际拉取约 10 MiB**，而真实需要的数据约 2.5 KB。
- **代码注释是错的**：`document/zip.rs:48-50` 写着"Names are already in the parsed central directory"，
  但 `ZipArchive::new` 本身就逐条目读了局部头，所以 `name_for_index` 省不掉这部分 I/O。
- **建议的下一步（P0-B2，需要你批准后才做）**：
  1. 打开阶段改用**单次连续读**自行解析中心目录（不再让 crate 逐条目读局部头），
     或给这一段单独的、小预读的读取器；
  2. 验收就用现成的那个红测试（≤ 8 次）+ 新增"打开不拉取超过 X 字节"的断言；
  3. 预期收益：`open_ms` 与首屏请求数大幅下降，且显著降低 115/夸克 的流量与风控面。

### 7.2 【中】前台仍会等一次后台预取

- **现象**：`page_latency_no_background` 里单页延迟出现 `[1018, 1492, 747]`，个别页升到 ~2 s。
- **机制**：`Reader::load_or_wait` 发现该页已被**后台预取**认领时会等待它完成；此时前台不自己发请求，
  优先级机制无从发挥。
- **与冻结决策的关系**：你的规则是"前台只能抢下一个许可，**不能取消已经在途的正常请求**"，
  因此本轮**刻意不改**。可行的后续（不违反该规则）：前台发现同页在途时**提升该在途请求**后续
  请求的有效优先级，而不是取消它。

### 7.3 持续后台负载下前台总和仍是空载的 2.4×

- **机制**：令牌桶的突发额度是**共享**的。后台持续占满时，前台每次读页仍要为每个窗口等一个令牌
  （4/s ⇒ 250 ms），于是 7 个窗口 ≈ 1.75 s。
- **两个候选杠杆（都超出 P0-B 的"只解耦门控"范围，未实现）**：
  1. 提高持续速率档位——由你在真机上按第 6.2 节流程选定；
  2. 方案里的"阅读会话活跃时先暂停新的后台封面提取"——它只拦新请求、不打断在途，与你的冻结决策 4 兼容。

### 7.4 【低】夸克 403 的文案分类不一致

`read_range_url` 的 403 文案是 `"夸克直链失效，请重试"`，**不含 `403`**；而
`scan_io_error` 是文本分类，因此它落到兜底 `Provider("range_read_failed")`，而 `downlink` 的
`"夸克 API HTTP 403: …"` 会正确映射为 `Forbidden`。同一个事实两条路径两个分类。
本轮**未改**（它会改变错误分类语义，属于独立小改动）。

### 7.5 【低】百度 `dlink` 仍无缓存

`api/source.rs` 的百度分支同样每次 `read_range` 都调 `c.link(path)`（`baidu.rs:379`，注释称直链约 8h 有效）。
你的 P0-D 只点名夸克，因此本轮**未动**；但同一套 `SingleFlight` + 缓存可直接复用。

---

## 8. 测试与回归

| 项 | 结果 |
|---|---|
| `cargo test --locked -- --test-threads=1`（全量） | **325 passed / 1 failed / 2 ignored** |
| 唯一失败 | `document::zip::tests::opening_many_pages_does_not_fetch_every_local_header` —— **A/B 已证明与本轮改动无关**，根因见 7.1，属 in-flight 变更集既有回归 |
| `perf` 模块 | 6/6 |
| `source::gate` | 9/9 |
| `source::singleflight` | 5/5 |
| `source::cloud115` | 19/19（含 2 个新用例） |
| `source::quark` | 11/11（含 5 个新用例） |
| P0 测量装置 | 7/7 |
| 编译告警 | 0（`cargo build` 无 warning） |

### 异常响应统计（本轮可断言的范围）

| 项 | 结果 |
|---|---|
| mock CDN 观察到的 206 / 403 / 405 / 429 | 正常场景全部 206；专门的用例强制 403 与 405 并被如实计数（`RangeStatus403=1`、`RangeStatus405=1`、`WafCooldowns=1`，冷却文案为中文） |
| 本轮读数中出现的 429 / 断链 / 下载线程错误 | **0** |
| **真实性限制** | 以上都是**本地 mock** 的结果。**"真机上没有新增 403/405/429"这一条我无法代证** —— 需要你按第 6.2 节跑真机档位流程。这一点我不做任何推断。 |

---

## 9. 工作树影响与清理

相对本轮开始前的快照（131 项 in-flight 改动），**新增恰好 6 项，全部属于 P0**：

```
 M app/rust/src/lib.rs                              （登记 perf 模块）
?? app/rust/src/perf.rs                             （埋点，行为中立）
?? app/rust/src/source/gate.rs                      （P0-B 优先级令牌桶）
?? app/rust/src/source/singleflight.rs              （P0-C/D 合并）
?? app/rust/tests/p0_baseline_read_speed.rs         （测量装置）
?? docs/reports/p0/                                 （本文件 + 基线文件）
```

其余被修改的文件（`source/mod.rs`、`source/cloud115.rs`、`source/quark.rs`、`source/baidu.rs`、
`reader.rs`、`api/remote_scan.rs`、`remote_scan/engine.rs`）在本轮开始前**已经是 dirty 状态**，
本轮只在其上追加改动。**没有触碰任何无关文件。**

- 临时产物已清理（测量用的 `D:\Temp\zipdiag.jsonl`、`/tmp/*` 均已删除；仓库内无 `.tmp`/`.part`/`.log` 残留）。
- **未** commit / **未** push / **未** merge。
- **未**进入 P1 / P2。

---

## 10. STOP 边界确认

本轮在 **P0 完成并通过自身门禁**后停止：

- ✅ 阅读延迟不再由旧的固定 4 QPS CDN 门控形成理论下限（到达时刻证明）
- ✅ 前台不再排在整条后台队列之后（前台单次等待 1248 → 247 ms）
- ✅ 相同条件下真实页面 latency 有明确改善（打开 −74.8%；9 次 Range −66.5%）
- ⚠️ "没有新的 403/405/429" **需要你真机确认**（我只能证明度量口径可用）
- ✅ Range 合约仍正确（206/`Content-Range` 校验、错范围拒绝逻辑未改）
- ✅ 本地缓存命中仍 0 请求 0 门控等待；磁盘缓存与取消语义未退化
- ✅ 相关测试通过（唯一失败为既有回归，已 A/B 归因）
- ✅ unrelated dirty worktree 未变化；临时文件已清理

**等待你审阅**。第 6.1 节的 `burst = 6` 是唯一需要你签字的默认值；第 7.1 节的压缩包打开放大是我建议的
下一阶段（P0-B2），需你批准后再动。
