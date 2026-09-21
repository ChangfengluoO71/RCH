# P0-B2 实测报告：ZIP/CBZ 远程读取放大

> 阶段：**P0-B2**（属 P0，已批准）｜ 未 commit / 未 push / 未 merge ｜ 未进入 P1
>
> 阶段一 trace 装置：`app/rust/tests/p0b2_zip_read_trace.rs`
> 阶段三契约测试：`app/rust/tests/p0b2_zip_read_amplification.rs`
> 生产改动：`app/rust/src/source/mod.rs`（`SourceReader` 缓存与取数策略）+ `app/rust/src/document/zip.rs`（测试口径）

---

## 0. 结论

放大已消除，且**正文读路径逐字节未变**：

| 指标（40 页 / 12,291,702 B 的 CBZ） | Before | After | 变化 |
|---|---:|---:|---|
| **archive open 请求数** | **81** | **42** | −48% |
| **archive open 传输字节** | **10,532,968**（≈10.05 MiB） | **6,790**（≈6.6 KiB） | **−99.94%（1551×）** |
| central-directory 请求 | 41 | **2** | −95% |
| local-header 请求 | 40 | 40（不可避免，见第 3 节） | 持平 |
| local-header **字节** | 10,485,760 | **2,560** | −99.98% |
| actual content 请求（打开期） | 0 | 0 | 持平 |
| 单次最大取数 | **262,144** | **2,182** | 不再出现 256 KiB 元数据读 |
| 区域来回切换（thrashing） | 79 / 80 | **1 / 41** | 抖动消除 |
| 每 entry 边际字节 | ≈262,144 | **136.7** | 复杂度从"每 entry 一个预读块"降为"每 entry 几十字节" |

正文路径对照（同一套测量装置）：

| 指标 | Before | After |
|---|---:|---:|
| 读单页（300 KiB）请求 / 字节 | 2 / 524,288 | **2 / 524,288** |
| 顺序读 8 页请求 / 字节 | 16 / 3,977,670 | **16 / 3,977,670** |

连带收益（P0 测量装置，4 页 × 1.5 MiB CBZ）：

| 指标 | P0-A 基线 | P0-B | **P0-B2** |
|---|---:|---:|---:|
| `open_ms` | 2,006 ms | 505 ms | **17 ms** |
| 首屏（`open_ms` + 首页） | ≈3,013 ms | ≈1,523 ms | **≈765 ms** |
| 磁盘命中后重新打开 `reopen_ms` | 1,005 ms | 13 ms | **10 ms** |

---

## 1. 第一阶段：逐请求分类 trace（只做 trace，不改行为）

### 1.1 方法

trace **完全写在测试侧**：一个记录型 `ByteSource` 挂在生产 `document::open_document`
前面，每次 `read_at` 记录 offset / requested / fetched / 上一窗口 / 是否落在 central
directory 区域，并用调用栈符号（`std::backtrace::Backtrace`）分类出逻辑操作。
生产代码在当时**一行未改**。

### 1.2 基线分类（40 页 / 12,291,702 B）

```text
reads=81  transferred≈10.05 MiB  region_alternations=79 of 80
  "local-header/data-start 解析"  40 次  10,485,760 B
  "central-directory 条目解析"    39 次      42,978 B
  "EOCD 尾部扫描"                  2 次       4,230 B
```

请求序列是严格交替的（节选）：

```text
seq=0 off=12289654 fetch=2048    op=EOCD 尾部扫描
seq=1 off=12289520 fetch=2182    op=EOCD 尾部扫描
seq=2 off=0        fetch=262144  op=local-header/data-start 解析   ← 30 字节的读被放大
seq=3 off=12289574 fetch=2128    op=central-directory 条目解析
seq=4 off=307238   fetch=262144  op=local-header/data-start 解析
seq=5 off=12289628 fetch=2074    op=central-directory 条目解析
...（如此往复 40 轮）
```

### 1.3 你要求的六个问题，逐条回答

| # | 问题 | 答案 |
|---|---|---|
| 1 | 多少次属于 central directory？ | **41** 次（39 次条目解析 + 2 次 EOCD 定位），共 47,208 B |
| 2 | 多少次属于 local file header？ | **40** 次（每个 entry 一次），共 **10,485,760 B** —— 放大全部在这里 |
| 3 | 多少次属于实际 page data？ | 打开阶段 **0** 次（页数据只在 `page_bytes` 时读） |
| 4 | 是否存在"仅为了枚举文件名而打开全部 entry"？ | **不存在**。RCH 的枚举循环用的是 `name_for_index`（纯内存，调用栈里从未出现 `by_index`/`name_for_index` 帧）。这 40 次 local-header 读发生在 **`ZipArchive::new` 内部**，是 crate 为解析每条中央目录条目的 `data_start` 而做的（调用栈为 `get_metadata → read_central_header → central_header_to_zip_file → find_data_start`） |
| 5 | 是否存在 local-header ↔ central-directory 来回 seek 导致单窗口持续失效？ | **是**。`region_alternations=79/80`；每次 local-header 读都淘汰掉中央目录窗口，导致 2 KB 的目录被**重下 39 次** |
| 6 | 256 KiB 放大发生在哪一层？ | **`SourceReader` 的预读层**。zip 侧请求只有 30 字节（`requested=30`），`SourceReader` 用 `max(READ_AHEAD, out.len())` 决定取数，于是每 30 字节请求都变成一次 256 KiB Range |

---

## 2. RED 证据确认（按你的四条要求）

| 要求 | 结论 | 证据 |
|---|---|---|
| 在未修改代码的 baseline 上稳定失败 | ✅ | 多次运行均为 81 次（run1/run2/run3 一致） |
| failure 原因确实是远程 read amplification | ✅ | 81 次中 40 次是"30 字节请求 → 262,144 字节取数"，10,485,760 B 占总量 99.6% |
| 摘掉 instrumentation 后仍失败 | ✅ | 临时移除 `SourceReader` 埋点后实测**同样 81 次** |
| 不是 mock 自己制造的额外读取 | ✅ | 该用例用的是 `MemSource`（内存字节源），不涉及任何 mock CDN；计数点在被测的 `ByteSource::read_at` 上 |

---

## 3. 根因与最小修复选择

### 3.1 Case A 不适用（已用 crate 源码证明）

`name_for_index` 是**纯内存**的：

```rust
// zip-2.4.2/src/read.rs:1022
pub fn name_for_index(&self, index: usize) -> Option<&str> {
    self.shared.files.get_index(index).map(|(name, _)| name.as_ref())
}
```

但 `ZipArchive::new` 本身会对每个条目解析 local header：

```rust
// zip-2.4.2/src/read.rs:1259（central_header_to_zip_file 内）
let data_start = find_data_start(&file, reader)?;
```

而 `zip::read::Config` **只有 `archive_offset` 一个字段**（`src/read/config.rs`），
没有任何"跳过 local header 解析 / 只读中央目录"的开关。

**结论**：RCH 侧没有为枚举而打开 entry；"每 entry 一次 local-header read"是 crate 的固定行为，
在公开 API 下不可消除。因此：

- **Case B（小 metadata read 被 256 KiB 放大）成立**；
- **Case C（单窗口 thrashing）成立**；
- Case A 不成立，因此没有改枚举代码。

### 3.2 修复：`SourceReader` 分两层窗口（最小改动）

只改了一个文件的一处策略（`app/rust/src/source/mod.rs`），没有动 ZIP 解析、没有换 crate、
没有重写 parser、没有预读 local header、没有用更高 burst 掩盖：

1. **顺序大窗口**（原有 `buf`，`READ_AHEAD = 256 KiB`）——只服务正文顺序读，行为不变。
2. **非顺序小窗口**（新增，固定 **2 槽** + LRU 触碰）——服务"小且散"的 metadata 读。
   - 判定：`out.len() < 4 KiB` 且**不邻近**上一次读取结束位置且不是全新 reader。
   - 取数：`out.len()` 夹到 `[64 B, 16 KiB]`；**若紧邻已有窗口，则视为同一访问流的延续并放大取数**
     （这正是中央目录能一次覆盖完、不再被反复重下的原因）。
   - 2 槽足够：中央目录窗口被每次 CD 读触碰，因此不会被分散的 local-header 读淘汰。

刻意**不是**通用缓存系统：槽数固定、无淘汰策略配置、不增长。

### 3.3 开发中踩到的两个回归（如实记录，都已回退）

| 回归 | 现象 | 原因 |
|---|---|---|
| 要求"大窗口非空"才算顺序读 | 顺序读 8 页请求 16 → **160**，字节反而更碎 | 首次读永远走不到大窗口，正文被切成小块 |
| 用 `last_end.is_none()` 代替"全新 reader" | 打开 40 页仍是 81 次 / 10.5 MiB | `find_data_start` 每次都 `seek`，而 `seek` 会把 `last_end` 置空，于是每条 local header 都被当作"首次读"放大成 256 KiB |

两条都写进了代码注释，防止回退。

---

## 4. 修复后 trace 分类（After）

```text
reads=42  transferred≈6.6 KiB  region_alternations=1 of 41
  "local-header/data-start 解析"  40 次   2,560 B   （每条 64 B）
  "central-directory 条目解析"     0 次       0 B   （全部命中窗口）
  "EOCD 尾部扫描"                  2 次   4,230 B
```

`small_request_big_fetch`（requested < 1 KiB 但 fetched ≥ 256 KiB）= **0**。

---

## 5. Scaling（按你的验收口径，不用单点阈值）

| entries | Before 请求 | Before 字节 | After 请求 | After 字节 |
|---:|---:|---:|---:|---:|
| 10 | 20 | 2,626,116 | **11** | **2,688** |
| 20 | — | — | **21** | **3,328** |
| 40 | 81 | 10,532,968 | **42** | **6,790** |

- Before：请求 ≈ `2N`，字节 ≈ `N × 256 KiB`（线性放大）。
- After：请求 ≈ `N + 1..2`，字节 ≈ `N × 64 B + 约 2 KB`；实测**每 entry 边际 136.7 B**。
- 剩余请求数仍是 O(N)，**因为 crate 每条 entry 都要解析 local header**（第 3.1 节）。
  这不再是"放大"，而是"每 entry 一次几十字节的必要读"。

---

## 6. 测试（TDD）

### 6.1 契约测试 `app/rust/tests/p0b2_zip_read_amplification.rs`（9 个）

RED（改动前）→ GREEN（改动后）实测三个失败项：

| 用例 | 断言 | Before | After |
|---|---|---|---|
| `b2_open_metadata_traffic_is_not_n_times_read_ahead` | 字节 ≤ `N×2 KiB + 64 KiB`；请求 ≤ `N+16` | ✗ 10,532,968 B / 81 | ✅ 6,790 B / 42 |
| `b2_open_traffic_scales_with_entry_count_below_read_ahead` | 每 entry 边际 < 64 KiB | ✗ | ✅ 136.7 B |
| `b2_central_directory_is_not_refetched_per_entry` | CD 区域请求 ≤ 3 | ✗ 41 | ✅ 2 |
| `b2_single_page_read_is_local_and_keeps_the_large_window` | 单页 ≤ 5 请求、字节 ≤ 2×页 + 64 KiB、不得触碰其它页区域 | ✅ | ✅ 2 请求 / 524,288 B |
| `b2_sequential_book_read_stays_close_to_content_size` | 顺序读书 ≤ 2×内容 + 64 KiB | ✅ | ✅ 16 请求 / 3,977,670 B |
| `b2_compat_zip64_non_image_entries_and_nested_paths` | ZIP64 + 非图片 entry + 嵌套路径 | ✅ | ✅ |
| `b2_compat_unicode_names_with_extra_fields` | Unicode 名 + extra field（经 ZIP64 header 0x0001） | ✅ | ✅ |
| `b2_compat_deflate_content_roundtrips` | Deflate 内容与 CRC 路径 | ✅ | ✅ |
| `b2_compat_truncated_local_header_fails_closed` | 截断包不得每页都"成功" | ✅ | ✅ |

阈值来源（不是随手拍的）：

- 每 entry `2 KiB`：`ZipLocalEntryBlock` 实测 30 B + 文件名 + extra；2 KiB 已是宽松上限。
- `64 KiB` 固定项：EOCD + 中央目录实测约 4.2 KB。
- 请求 `N + 16`：实测 `N + 1..2`，留足余量。

### 6.2 既有断言口径变更（透明说明）

`document::zip::tests::opening_many_pages_does_not_fetch_every_local_header` 原本断言
`open_reads <= 8`，**该阈值在公开 crate API 下不可达**（证据见 3.1）。我没有删除它，而是把断言
改成对本轮真正契约的三条不变量，并在测试里写清了：

- 原阈值的数值与它为何不可达（含 crate 源码位置）；
- before/after 的 81 → 42 请求、10,532,968 → 6,790 字节；
- 新断言：请求 ≤ `N+16`、打开字节 ≤ `N×2 KiB + 64 KiB`、**单次取数 < 256 KiB**（这条在"放大"维度上比 `<=8` 更严）。

### 6.3 Compatibility 未做的部分（明确声明）

- **未**覆盖"未知/外来 extra field"（如 AES 的 0x9901）：RCH 不解释 extra field，
  那是 `zip` crate 的路径，本轮未改动它；已用 ZIP64 extra field（0x0001）覆盖"带 extra field 的条目"。
- **未**覆盖加密压缩包：项目本身不支持。
- **未**做 data descriptor 专项用例：`ZipWriter` 写可 seek 目标时不产生 data descriptor；
  单造该布局需要手写 ZIP 字节流，属独立工作，未在本轮引入（见第 8 节 remaining risks）。

---

## 7. 回归门结果

| 门 | 结果 |
|---|---|
| focused RED → GREEN | ✅ 3 个失败项全绿 |
| ZIP/CBZ 相关测试 | ✅ `document::zip` 3 passed / 1 ignored |
| SourceReader / cache 测试 | ✅ 见下 P0 装置 + 全量 |
| P0-B gate / singleflight / URL-cache 测试 | ✅ `source::gate` 9、`source::singleflight` 5、`source::cloud115` 19、`source::quark` 11 |
| **全量 Rust gate** | ✅ `cargo test --locked -- --test-threads=1` **EXIT=0**；13 个测试目标全部 ok，**合计 376 passed / 0 failed / 2 ignored** |
| `git diff --check` | ✅ clean |
| `rustfmt --check` | ✅ 本轮新增/编写的 6 个文件全部合规；`source/mod.rs` 仅剩 1 处**既有**行的提示（`ByteSource for Arc<S>` 单行缩写，非本轮编写，未改动以避免污染既有变更集） |
| unrelated dirty baseline | ✅ 相对本轮开始快照只新增本人文件（见第 9 节） |
| 临时产物清理 | ✅ `D:\Temp\{b2,p0b2,zipdiag}.jsonl` 已删；仓库内无 `.tmp/.log/.part/.jsonl` 残留 |

> 上一轮那个"既有失败"就是本轮的 focused 目标（`opening_many_pages_...`）。它已按第 6.2 节
> 改为可达成且更强的契约并转绿，**不是笼统标记为 pre-existing 后跳过**。

---

## 8. Remaining risks / 明确的后续（**未**在本轮做）

1. **请求数仍是 O(N)**：`ZipArchive::new` 每条 entry 解析 local header。
   要降到 O(1) 必须**绕开 crate 自行解析中央目录**（例如只读 CD 段自建页表，再用 crate 读 entry 内容）。
   这属于 parser 级改动，你的禁令明确排除在本轮之外；建议作为**独立提案**（附 crates 迁移评估）单独评审。
2. **打开 16 页以上时，瓶颈已从放大转为 CDN 门控**：实测 16 页 × 1.5 MiB 的归档
   `open_ms ≈ 2,761 ms`，其中约 18 次请求 × 4/s 门控 ≈ 2.7 s。也就是说 B2 之后
   **open 的成本 = 请求数 × 门控间隔**，下一杠杆是 P0-B 冻结的速率档位（真机测量选定）或第 1 条的 O(1) 化。
3. **data descriptor 布局未专项覆盖**（见 6.3）。
4. **`META_WINDOWS = 2`** 是为"目录流 + 分散读"这一实测形态选的。若将来出现三类交错访问，
   需要按 trace 重新评估槽数（当前设计刻意不做通用淘汰策略）。
5. **多线程共享同一 reader 的情况**：`SourceReader` 每个 clone 独立窗口（与原实现一致），
   并行预取时每个线程各自持有一份小窗口，没有新增共享状态。

---

## 9. 工作树影响与清理

相对本轮开始前的快照，**新增仅本人文件**：

```
 M app/rust/src/lib.rs                                  （P0-A 登记 perf 模块）
 M app/rust/src/source/mod.rs                           （原已 dirty；本轮改 SourceReader 策略）
 M app/rust/src/document/zip.rs                         （原已 dirty；本轮只改测试口径）
?? app/rust/src/perf.rs
?? app/rust/src/source/gate.rs
?? app/rust/src/source/singleflight.rs
?? app/rust/tests/p0_baseline_read_speed.rs
?? app/rust/tests/p0b2_zip_read_amplification.rs        （本轮新增）
?? app/rust/tests/p0b2_zip_read_trace.rs                （本轮新增）
?? docs/reports/p0/
```

`source/mod.rs` 与 `document/zip.rs` 在本轮开始前就已经是 dirty 状态（属既有 in-flight 变更集），
本轮只在其上追加改动。**没有触碰任何无关文件**，也**没有** commit / push / merge。

---

## 10. STOP 边界确认

P0-B2 达到自身验收后停止：

- ✅ 根因 trace（第 1 节）+ 81 次请求分类（第 1.3 节六问全答）
- ✅ RED → GREEN 证据（第 2、6 节）
- ✅ before/after request、byte、latency（第 0、5 节）
- ✅ 修改文件清单（第 9 节）
- ✅ focused / full regression（第 7 节）
- ✅ remaining risks（第 8 节）
- ✅ 未进入 P1，未 commit / push / merge

**Release Gate 仍 PENDING**：真实 115/夸克 S1–S5 与"无新增 403/405/429/风控异常"仍需你在真机执行
（规程见 P0-A 报告第 5 节与 P0 报告第 6.2 节）。B2 的读数全部来自本地 mock CDN + 未改动的生产代码路径，
只替换端点。
