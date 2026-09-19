# 调研：远程归档"取封面"打开成本 —— 有没有比方案 C 更好的做法？

- 日期：2026-09-19（第 62 轮）
- 触发：115 大归档封面长期拿不到（第 60 轮量化：单枚 >140s 且跑不完，单线程 worker 被它堵死）
- 结论摘要：**C（"读文件头 + 只读 EOCD/中心目录"）在现有依赖下不可实现**；
  真正可行且更优的是 **D：为"只取封面"写一个最小 ZIP 读取器（或先试 crate 的流式 API）**，
  把打开成本从 **O(条目数)** 降到 **O(1)**。

---

## 1. 现状与依赖事实

| 项 | 事实 | 来源 |
|---|---|---|
| 依赖 | `zip = { version = "2", features = ["deflate"] }`，锁定 **2.4.2** | `app/rust/Cargo.toml`、`Cargo.lock:3858` |
| 封面路径 | `fetch_cover_from_document` → `document::open_document` → `ZipBook::open` → **`zip::ZipArchive::new`** | `src/api/remote_scan.rs:1599-1607`、`src/document/zip.rs:43-66` |
| 项目既有分析 | "`ZipArchive::new` 会对**每个**条目 `find_data_start`（seek 读 local header）"；"要降到 `<=8` 只能绕开 crate 自行解析中央目录 —— 属 parser 迁移，不在 P0-B2 范围内" | `src/document/zip.rs:193-209` |

## 2. `zip 2.4.2` 的确切行为（源码级，含行号）

源码：`C:\Users\cfl\.cargo\registry\src\rsproxy.cn-…\zip-2.4.2\`

1. **EOCD 搜索**（`spec.rs:621`）：`MagicFinder::<Backwards>::new(&EOCD_SIG_BYTES, 0, end_exclusive)`
   —— 窗口**固定 2048 B**（`read/magic_finder.rs:115`），步进**固定 2045 B**（`magic_finder.rs:81-94`），
   **无几何扩张、下界硬编码 0** ⇒ 最坏**扫全文件**。
   上游同症状 issue：[zip2#231](https://github.com/zip-rs/zip2/issues/231)（"找到 EOCD 后仍继续向后扫"）、
   [zip2#280](https://github.com/zip-rs/zip2/issues/280)（打开即扫描大段文件）。
2. **公开 API 无法短路 EOCD 搜索**：`Config` 只有 `archive_offset`（`read/config.rs:1-23`），
   而 `ArchiveOffset::Known` 只作用于 CDFH 子搜索器（`spec.rs:698-704`）⇒ **对 EOCD 搜索零影响**。
   ⇒ **方案 C 里"直接读 EOCD"这一半，用 crate 做不到。**
3. **每条目额外读 local header**：`read_central_header`（`read.rs:685-719`）逐条目
   `central_header_to_zip_file`（`read.rs:1235`）→ **`find_data_start`**（`read.rs:1259` → `read.rs:362-378`）
   ⇒ **≈5 read + 2 seek / 条目**。500 条 ≈ 2500 次，5000 条 ≈ 25000 次，**与 CD 字节数无关**。
   ⇒ **方案 C 里"只读中心目录"这一半，也做不到**（`by_index` 必须先用完整 CD 填充 `shared.files`，`read.rs:1115-1119`）。
4. **流式 API 存在但受限**：`read_zipfile_from_stream`（`read.rs:1890-1924`，公开、只需 `Read`、**全程不 seek**）
   —— 但**位 3（data descriptor）条目直接报错**：`types.rs:710-715`
   `UnsupportedArchive("The file length is not available in the local header")`。
5. **CD 路径支持位 3**（压缩尺寸取自 CD，`read.rs:1308`）⇒ 自研路径若要稳，**位 3 必须走 CD**。

## 3. RCH 侧的真实放大倍数（实测）

`src/source/mod.rs`：`META_MAX_FETCH = 16 KiB`（:108）、`READ_AHEAD = 256 KiB`（:103）。

**实测（第 60 轮有界探针，2,238,456,091 B 的 `.zip`）**：

| 指标 | 实测 |
|---|---|
| `cdn.range` 网络读 | 574 次 / ~150s 仍未完成（被预算/超时截断） |
| 单次范围读延迟 | 平均 **243.5 ms**（115 CDN） |
| 读偏移分布（已发布代码） | 367 次读 / **186 个不同偏移**，散布 **0 → 2.24 GB**，相邻间隔 40KB–318KB **不规则**，单次**平均 1.5 KB**（最小 64 B） |

⇒ 偏移散布 + 每次读极小 ⇒ **主因是"每条目一次 local-header 校验读"**（不是 EOCD 反扫；
第 60 轮曾误判为"尾部 233MB 连续回扫"，那是当时 512KB 块缓存方案的假象，**已回退并在此更正**）。

**量级**：一本 ~5000 条目的漫画 ⇒ ~1 万次请求 × 243ms ≈ **40 分钟/枚**；
后台 worker 是**单线程** ⇒ 一枚就能把整条队列堵死（295 个 job 实测全堵）。

## 4. 方案对比

| 方案 | 做法 | 打开成本 | 可行性（现有依赖） | 结论 |
|---|---|---|---|---|
| **A 保持现状 + 预算**（第 60 轮已上线） | 三重上限（24MB/192 次/45s）快速失败 + 具体码 | 有界 ✗ 但**大归档拿不到封面** | ✅ 已实现 | 保留为**兜底** |
| **B 调大预算** | 例如 512 次/64MB/90s | 每枚更慢，仍 O(条目数) ✗ | ✅ 易改 | 只缓解 |
| **C 头 4 字节 + 只读 EOCD/CD** | 依赖 crate 只读元数据 | —— | ❌ **不可实现**：crate 无 API 跳过 EOCD 搜索（§2.2）、`by_index` 必付全 CD + 每条目校验（§2.3） | **否决** |
| **D 封面专用最小读取器**（推荐） | 尾部 ≤64 KiB 一次读 → 找 `PK\x05\x06` → 22 B EOCD → 读 CD（~60 B/条目，一次大范围读）→ **内存里**挑首张图片 → 读该条目 30 B local header + 数据 | **4–6 次请求，O(1) 与文件大小/条目数无关** ✅ | ✅ 自研 ~150 行，不换依赖 | **推荐** |
| **D′ 先试 crate 流式 API** | `read_zipfile_from_stream` 读首条目（不 seek）→ 失败（位 3）再走 D | 2 次请求（位 3 清零时） | ✅ 公开 API | **D 的廉价前置**（建议与 D 组合） |
| **E 升级 `zip` crate** | 换新版本期待修好 | ? | ⚠️ 未证实：#231 已关闭、[#280](https://github.com/zip-rs/zip2/issues/280) 描述的是**每条目校验**导致的扫描（正是 §2.3 行为，属设计而非回归）⇒ 升级**大概率不解决** O(条目数) | 暂不作为主线 |
| **F 换解析器**（如 [rc-zip](https://github.com/bearcove/rc-zip)，sans-io） | 全量替换 zip 解析 | ✅ | ❌ 影响阅读器/EPUB/写档全部路径，远超本轮范围 | 不采纳 |
| **G 缓存条目偏移** | 扫描期把"首图条目偏移"写进 DB，封面 O(1) 直达 | 2 次请求 | ⚠️ 需**改表结构**（须你确认）+ 失效处理 | 未来可选 |
| **H 并行 worker** | 多线程取封面 | 不解决单枚 O(条目数) ✗ | ✅ | 只提升吞吐，不改单枚成本 |

## 5. 建议（待你点头后再动手）

1. **实现 D′+D 的封面快通道**（`src/document/zip.rs` 内新增，只服务封面路径）：
   - 步骤 1：`read_zipfile_from_stream` 试读首个条目（2 次请求；位 3 则跳过）；
   - 步骤 2：尾部 ≤64 KiB 一次读（ZIP 规范里注释上限 64 KiB ⇒ **标准完备**）找 EOCD
     → 读 CD → 内存里定位首张图片条目 → 读其 local header + 数据；
     **位 3 用 CD 里的压缩尺寸**（§2.5）；
   - 步骤 3：都失败再退回现有 `ZipArchive` 路径（并受预算保护）。
2. **保留预算**（第 60 轮的三重上限）作为兜底，快通道落地后可把上限放宽。
3. **不换依赖、不改表结构、不动阅读器主路径** —— 快通道只在"取封面"这一条路径上生效，
   阅读器打开书籍仍走现有 `ZipBook`（保持行为不变）。
4. 需要你确认的点：这是**新增一条 ZIP 解析路径**（项目此前把它标为"parser 迁移、超出范围"），
   因此按规则必须先经你同意；若同意，我会先写**单测覆盖**（正常 CBZ / 首条目为
   `ComicInfo.xml` / 位 3 条目 / ZIP64 / 非 ZIP 头）再落地。

## 6. 附：本轮不改代码

第 60/61 轮的预算与租约回收已在线上运行；本文件仅为调研结论与方案对比。
