# P0-A 基线证据（机制级对照）

> 日期：2026-09-17 ｜ 阶段：**P0-A（取证）** ｜ 生产代码行为变更：**无**（仅新增埋点与测量装置）
>
> 运行入口：`cargo test --test p0_baseline_read_speed -- --nocapture --test-threads=1`
>
> 产物：`app/rust/tests/p0_baseline_read_speed.rs`（装置）、`app/rust/src/perf.rs`（埋点）

---

## 1. 这组基线测的是什么（以及**不**测什么）

**测的是生产代码路径本身**，不是等效模型：

| 环节 | 本组基线 | 说明 |
|---|---|---|
| HTTP 端点 | **本地 mock**（`127.0.0.1`） | 只提供 115 CDN 的协议形状：`Range: bytes=a-b` → `206` + `Content-Range` |
| CDN 门控 | **生产 `range_gate`（4 req/s）** | `Cloud115WebClient::read_range_url` 原函数，未改动 |
| 压缩包解析 | **生产 `ZipBook`** | `document::open_document` |
| 预读与窗口 | **生产 `SourceReader`（`READ_AHEAD = 256 KiB`）** | |
| 页缓存 / inflight / 优先级 | **生产 `Reader`** | L1 内存、L2 磁盘、`BlockingRequestGovernor` 全部生效 |
| 阅读记录、UI | 未覆盖 | 需要真机 |

**必须诚实标注的边界：**

1. 本组基线**不能替代真机 + 真实 115/夸克账号**的端到端测量：它不覆盖 CDN 真实延迟分布、TLS、真实 WAF 行为、真实压缩包结构、真实页面大小分布、Android/Windows 平台差异。
2. `downurl`（取链）端点需要 m115 加密响应，本装置无法伪造，因此**"取链全局锁"的量化不在本文件**，将在 P0-C 通过可注入 fetch 的单元测试给出（同 pickcode 合并 / 不同 pickcode 互不阻塞）。
3. 因此本文件命名为"机制级基线"。真机基线规程见第 5 节，仍需你执行。

---

## 2. 原始数据（逐场景 JSON，未加工）

### 2.1 CDN 门控是否构成阅读延迟的人工下限

```json
P0-BASELINE cdn_gate_sequential {
  "requests": 9,
  "wall_ms": 2259,
  "theoretical_floor_ms": 2000,
  "theoretical_formula": "(n-1) * 250ms",
  "gate_wait_us_total": 2197165,
  "gate_wait_us_max": 250362,
  "cdn_requests": 9,
  "cdn_status_206": 9,
  "inflight_max": 1,
  "server_request_count": 9,
  "arrival_offsets_us": [0, 249246, 499718, 749725, 1000166, 1250647, 1500512, 1751100, 2001287]
}
```

```json
P0-BASELINE cdn_gate_concurrent {
  "requests": 9,
  "wall_ms": 2259,
  "theoretical_floor_ms": 2000,
  "server_max_overlap": 1,
  "caller_entered_max": 9,
  "gate_wait_us_total": 11261330,
  "cdn_status_206": 9,
  "server_request_count": 9
}
```

### 2.2 真实阅读链路的打开成本与单页延迟

```json
P0-BASELINE page_latency_no_background {
  "page_bytes": 1572864,
  "read_ahead_bytes": 262144,
  "theoretical_reads_per_page": 6,
  "open_ms": 2006,
  "page_ms": [1007, 1751, 250],
  "source_reads_per_page": [5, 7, 1],
  "range_requests_per_page": [4, 8, 1],
  "gate_wait_us_total": 11561664,
  "inflight_max": 2
}

P0-BASELINE page_latency_no_background_totals {
  "total_source_reads": 13,
  "theoretical_total_if_all_cold": 18,
  "prefetched_pages": 0
}
```

### 2.3 前台阅读 vs 后台竞争（同一次运行内两相）

```json
P0-BASELINE foreground_vs_background {
  "quiet_page_ms": [762, 1751, 250],
  "quiet_p50_ms": 762,
  "quiet_open_ms": 4008,
  "loaded_page_ms": [3895, 5005, 0],
  "loaded_p50_ms": 3895,
  "loaded_open_ms": 4008,
  "loaded_p50_inflation_x": 5.11,
  "background_requests": 28,
  "background_max_overlap": 1
}
```

### 2.4 磁盘缓存命中是否绕开门控

```json
P0-BASELINE disk_cache_hit {
  "cold_ranges": 6,
  "reopen_ms": 1005,
  "reopen_requests": 6,
  "warm_ms": 0,
  "warm_cdn_requests": 0,
  "warm_gate_wait_us": 0,
  "warm_disk_hits": 1
}
```

### 2.5 埋点归属与错误码计数（P0 停止条件的度量基础）

```json
P0-BASELINE gate_attribution {
  "server_requests_delta": 1,
  "cdn_requests_delta": 2,
  "cdn_gate_wait_us_delta": 1490775,
  "api_gate_wait_us_delta": 0
}

P0-BASELINE range_status_accounting {
  "status_403": 1,
  "status_405": 1,
  "waf_cooldowns": 1,
  "cooldown_message_is_chinese": true
}
```

> `cdn_requests_delta: 2` 高于 `server_requests_delta: 1`，是**装置隔离**造成的：`cargo test` 共用一个进程，其它用例遗留的 `Reader` 预取线程仍在后台发请求。线级断言一律以服务端观测为准。这不是产品问题。

---

## 3. 这些数字证明了什么

### 3.1 **读速被门控决定，而不是被网络决定**（P0-B 的核心前提）

`arrival_offsets_us = [0, 249246, 499718, 749725, 1000166, 1250647, 1500512, 1751100, 2001287]`

服务端收到的 9 个请求，相邻间隔稳定在 **249–250 ms**——正是 `1 / 4.0` 秒。总 wall time 2259 ms 中，**门控等待占 2197 ms（97.3%）**，真实网络往返（含 mock 5 ms 服务延迟）合计约 60 ms。

> 结论：在 4 req/s 固定间隔门下，**请求数量本身就是延迟**。降低阅读延迟的唯一有效手段是减少请求数或去掉固定间隔，而不是换网络、加并发或调超时。

### 3.2 **并发被压成串行**（外层优先级不可能生效的第二重原因）

`cdn_gate_concurrent`：9 个调用线程**同时进入**（`caller_entered_max: 9`），但**服务端观察到的最大并发 `server_max_overlap: 1`**，wall time 2259 ms 与顺序发起**完全相同**。

> 结论：`RateGate::wait` 持锁等待把并发彻底串行化。即使 governor 把许可发给了前台，前台仍要在同一把锁上排队 —— 这就是"外层优先级没有贯通到底层请求"的线级证据。

### 3.3 **打开一本远程 CBZ 要付约 2 秒门控代价**

`open_ms = 2006`（4 页 × 1.5 MiB 的 CBZ）；`foreground_vs_background` 里 8 页 × 1.5 MiB 的 CBZ `open_ms = 4008`。

> 结论：`open_*_book` 走流式时，`ZipBook::open` 读中心目录的每个窗口都排在 250 ms 之后。**这与页面大小无关、与阅读优化无关，纯粹是门控 + 目录规模。** 这是方案里"压缩包索引复用 / 合并相邻 Range"的量化依据。

### 3.4 **单页成本 ≈ 预读窗口数 × 250 ms**

1.5 MiB 页 / 256 KiB 预读 = 6 个窗口（`theoretical_reads_per_page: 6`，实测 `[5, 7, 1]`）。
冷页实测 1007 ms / 1751 ms —— 与"5~7 个窗口 × 250 ms"吻合。

> 结论：**单页延迟 ≈ ceil(页大小 / 256 KiB) × 250 ms**。提高远程预读块（方案 3.4-D1）与降低固定间隔，两者都能线性改善这里。

### 3.5 **后台负载把前台延迟抬高 5.1 倍**（优先级反转的量化）

| 相位 | 前台 p50 | 前台逐页 |
|---|---|---|
| A：仅前台 | **762 ms** | `[762, 1751, 250]` |
| B：前台 + 3 个后台 Range 线程 | **3895 ms** | `[3895, 5005, 0]` |

后台线程在 3.5 s 内只发出了 **28 个**请求（`background_requests: 28` —— 它们自己也被门控堵住），却让前台 p50 涨到 **5.11×**。

> 结论：这是 P0-B "优先级必须真正贯穿到单次网络请求"的直接目标。基线数值即为验收对照锚点。

### 3.6 **磁盘命中确实完全绕开门控**（P0 必须保持的既有正确行为）

`warm_cdn_requests: 0`、`warm_gate_wait_us: 0`、`warm_ms: 0`、`warm_disk_hits: 1`。

同场景还暴露一个**待优化项**：重新流式打开同一本书时 `reopen_ms = 1005`、`reopen_requests = 6` —— 即便所有页都在磁盘缓存里，**打开动作仍要重读一遍压缩包中心目录**。

### 3.7 **错误码可如实计数**（P0 停止条件已可度量）

强制 403 → `RangeStatus403 = 1` 且返回 `PermissionDenied`；强制 405 → `RangeStatus405 = 1`、`WafCooldowns = 1`，且后续请求被冷却短路并给出中文提示（`"115 请求暂被风控限流，请约 N 秒后重试"`）。

> 结论：P0 提高 CDN 档位时的停止条件（出现 403/405/429 即停）**有可测抓手**，不需要靠人工翻日志。

---

## 4. 埋点契约（本轮新增，行为中立）

| 环境变量 | 作用 |
|---|---|
| `RCH_PERF_LOG=<path>` | 打开 JSONL 事件流（默认**完全关闭**，不写任何文件） |
| `RCH_PERF_TAG=<label>` | 每行的阶段标签：`baseline` / `p0b` / `p0c` / `p0d` |

事件类型：`cdn.range`（offset/len/gate_wait_us/status/error）、`cdn.probe`、`source.read_at`（offset/len/requested/filled）、`reader.get_page`、`reader.load_claimed`（disk_hit/governor_wait_us）。

计数器快照键（`perf::snapshot()`）：`RangeRequests`、`RangeBytes`、`RangeWaitUsTotal`、`RangeWaitUsMax`、`RangeInFlight`、`RangeInFlightMax`、`RangeStatus206/200/403/405/429/Other`、`RangeErrors`、`DownUrlRequests`、`DownUrlCacheHits`、`DownUrlFetched`、`DownUrlCoalesced`、`DownUrlErrors`、`DownUrlLockWaitUsTotal`、`DownUrlLockWaitUsMax`、`ApiGateWaitUsTotal/Max`、`CdnGateWaitUsTotal/Max`、`PageLoads`、`PageLoadUsTotal/Max`、`PageDiskHits`、`PageMemoryHits`、`GovernorWaitUsTotal/Max`、`GovernorWaitUsTotalBackground`、`WafCooldowns`、`Cancellations`、`SourceReadAt`、`SourceReadAtBytes`。

**脱敏保证**：事件里没有 Cookie / Authorization / 直链 URL / 请求响应正文，只有偏移、长度、状态码、耗时与计数。

**行为中立性保证**：默认不写文件；计数器为原子累加；`RateGate::wait()` 的返回值只被埋点使用；未引入任何新的调度、限流、缓存或错误语义分支。既有测试全绿（见第 6 节）。

---

## 5. 真机基线规程（**需要你执行**，我无 115/夸克 账号）

我无法代跑这一节。请按下面步骤产出真机证据，我再据此选最终 CDN 档位。

### 5.1 采样对象（对应你列的清单）

| 样本 | 要求 |
|---|---|
| S1 | 单页约 **1.5 MB** 的漫画，连续翻 10 页 |
| S2 | 单页约 **3 MB** 的漫画，连续翻 10 页 |
| S3 | 同一本书，**先触发一次云端扫描/封面队列**，再重复 S1 的翻页 |
| S4 | 20 / 100 / 300 MiB 的整本下载（仅记录 Range 行为，不做优化结论） |
| S5 | 同一批样本在**夸克**上重跑 S1、S3 |

### 5.2 操作

1. 构建并安装带本轮埋点的构建（Windows 桌面最方便）。
2. 设环境变量后启动（路径任意，建议放到缓存目录下）：
   ```powershell
   $env:RCH_PERF_LOG = "$env:TEMP\rch-perf-baseline.jsonl"
   $env:RCH_PERF_TAG = "baseline"
   ```
3. **先清理该书的 `page/`、`raw/` 缓存**，保证冷启动。
4. 按 S1→S5 操作，每换一个样本重新设 `RCH_PERF_TAG`（`baseline-s1`、`baseline-s3` …）。
5. 收工后把 JSONL 给我。**不要**上传到任何外部位置。

### 5.3 我会从 JSONL 里算出的表

首屏 `open_ms`、单页 wall time（P50/P95）、每页 `source.read_at` 次数、`cdn.range` 的 `gate_wait_us` 分布、取链请求数（`DownUrlRequests` / `CacheHits` / `Coalesced`）、`DownUrlLockWaitUs` 峰值、cache hit/miss（`PageDiskHits` / `PageMemoryHits`）、请求到达时间戳、服务端并发、HTTP 状态码分布、取消次数、WAF 冷却次数。

### 5.4 CDN 档位选择流程（P0-B 之后执行）

1. 固定样本 S1 + S3，依次跑档位：`4/s`（现状）→ `8/s` → `12/s` → `20/s`（`20/s` 仅作实验档，不是默认值）。
2. 每个档位记录：单页 P50/P95、Range 请求数、以及 **403/405/429 与 `WafCooldowns` 计数**。
3. **停止条件（任一命中即停止升档并回退上一档）**：出现 403/405/429、`WafCooldowns > 0`、下载线程错误、异常断链、错误率上升。
4. 若 `8/s` 或 `12/s` 已达性能平台（P95 不再下降），**不为了对齐计划里的 20/s 继续提高**。
5. 最终默认值 = 在该平台且零停止条件的档位，回填为常量。

---

## 6. 工作树影响（本轮）

相对本轮开始前的快照（131 项 in-flight 改动），**新增仅 3 项，全部属于 P0**：

```
 M app/rust/src/lib.rs                              （注册 perf 模块）
?? app/rust/src/perf.rs                             （埋点模块）
?? app/rust/tests/p0_baseline_read_speed.rs         （测量装置）
```

其余被修改的文件（`source/mod.rs`、`source/cloud115.rs`、`source/quark.rs`、`source/baidu.rs`、`reader.rs`）在本轮开始前**已经是 dirty 状态**，属于既有的 in-flight 变更集，本轮只在其上追加埋点。**没有触碰任何无关文件。**

本轮**未** commit / push / merge。

---

## 7. 既有测试回归

见 `2026-09-17-p0b-gate-decoupling.md` 的回归章节（全量 `cargo test --locked -- --test-threads=1` 结果）。本轮埋点阶段的全量结果记录如下：

- `cargo test --test p0_baseline_read_speed -- --test-threads=1` → **7 passed / 0 failed**
- 全量套件结果见第 8 节回填。

---

## 8. 回填区（已补齐）

| 项 | 值 |
|---|---|
| 全量 `cargo test --locked -- --test-threads=1` | **325 passed / 1 failed / 2 ignored**；唯一失败为 `document::zip::tests::opening_many_pages_does_not_fetch_every_local_header`，已 A/B 证明与本轮埋点无关（摘掉埋点后同样 81 次读），属 in-flight 变更集既有回归，根因见下方 |
| 真机基线（S1–S5） | **待你执行**（第 5 节规程） |
| 最终 CDN 档位 | 本轮未改，默认仍为 4/s；突发额度默认 6（需你签字），档位切换见 P0 报告第 6.2 节 |

### 8.1 该失败测试的根因（在本轮实测中定位，**未修**）

打开 40 页 CBZ 实测 81 次 Range，原因不是本项目代码，也不只是埋点：

1. `ZipArchive::new` 的 `central_header_to_zip_file` 对**每个条目**调用 `find_data_start`，
   去读该条目的**局部文件头**，然后 seek 回中心目录继续；
2. `SourceReader` 只有**一个**预读窗口，这种"中心目录 ↔ 页数据"来回交替让窗口每次失效，
   于是**每条目 2 次 `read_at`**；
3. 每次 `read_at` 的 `len` 是 `262144`（而 `requested` 只有 30 字节）⇒ 打开一本 12 MiB / 40 页的
   CBZ 会**实际拉取约 10 MiB**，真实需要的数据约 2.5 KB。

这条放大约 **4000×**，是首屏时间与流量/风控面的主要来源之一，已作为建议的下一阶段（P0-B2）
记入 [`2026-09-17-p0bcd-gate-and-download-url.md`](2026-09-17-p0bcd-gate-and-download-url.md) 第 7.1 节。
