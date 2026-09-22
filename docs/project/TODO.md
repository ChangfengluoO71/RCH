# TODO.md — 任务看板

> 状态看板:Backlog(待办)/ Doing(进行中)/ Waiting(待验证)/ Done(已完成)。
> 里程碑验收标准见 SPEC.md。

---

## 0. 工作流程(每轮闭环)

1. **读文档对齐** — 读 README(当前状态)+ TODO(看板)+ SPEC(目标)
2. **判断类型** — Bug修复 / 功能开发 / 重构 / 架构调整。架构级先征求用户确认
3. **小步开发** — 最小修改、不动已有功能。改 Rust API 后 generate 桥接
4. **验证** — `cargo test/check` + `flutter analyze` + `flutter run` 实测
5. **立即记档** — 更新 LOG、LOG-INDEX(起止行+HASH)、TODO(状态流转)
6. **交用户验证** — 说清改了什么、怎么测
7. **用户确认后** — 更新 README。若涉及 SPEC 变更再单独确认;TODO 归档

---

## 交接单（第 81 轮结束 · 提交 `b044679`）

> 接手第一件事：读本节 → `docs/project/LOG.md` 第 79–81 轮（含 6 段续修，起于第 2874 行）
> → `git log -1`。**不要**重读 79–81 轮的实现细节，LOG 里已写清动机、证据与被门禁挡回的过程。

### A. 仓库状态
- **已提交未推送**：`b044679`（三轮合并，27 文件 / +2642 −222，含 rename `quark_epub_probe.rs → quark_document_probe.rs`）。
  推送需用户授权（不要擅自 push）。
- **工作区唯一脏文件是接手前就存在的**：`app/{linux,macos,windows}/flutter/generated_*` —— 不要提交、不要清理。
- 分支 `p1-cover-completion`。

### B. 已交付且已验证（都带真机/桌面数据，详见 LOG）
1. **EPUB**（第 78 轮，`8a8a183`）：打开只读中央目录，真机 219/204 次读 → **4 次读**。
2. **PDF**（第 79 轮）：`PdfBook::open` pdfium 按需读；真机 57.5 MB 打开+首页 **57.5 MB → 0.29 MB（197×）**，渲染**逐字节一致**；封面按显示宽度渲染（1029 ms → 36 ms）。
3. **MOBI**（第 81 轮）：`MobiBook` 惰性按需读（PalmDB 头 + 记录表 + record0 探测，**两个候选起点** + 先验后回退），打开只读 **<4 KB**。
4. **封面口径**：状态读取按 `selection+profile`（契约 STATE-READ-4）；卡片"有缓存直读 / 没缓存获取 / 跨档回退 / 容器卡回退 legacy"（Dart 8 项测试全过）。
5. **封面重抓**：`remote_cover_reset_to_current_profile`（换档删旧档 + 终态 failed 重排队 attempt 归零）。
6. **诊断**：`cover_budget detail=<bytes/reads/ms>`（就地记录）+ `cover.fetch.dur_us`。
7. **否定结论**：并行 Range GET 无收益（同批 PDF 封面 1.45–1.73 s vs 1.60–1.94 s）⇒ 默认关闭，开关 `RCH_RANGE_PARALLEL_MIN` 保留。

### C. 下一步（按优先级，需用户点头的两条已标注）
1. **大 MOBI/PDF 的"单条记录吞吐"**（`9.mobi` 180 MB 封面 30 s 读不完）：已实测确认瓶颈是**链路/账号总带宽**，代码侧无低风险优化 ⇒ 只剩"减少传输字节"：
   - 封面**部分解码/缩略图**（项目里已有 `cover_partial_decode_failed` 语义；先探这些 MOBI 的封面 JPEG 是否渐进式）——**动画质，需用户确认**；
   - 阅读**按屏宽渲染 + 长条切片**（即"任务 3"）——**动画质，需用户确认**。
2. **重置分批/限流**（item 2）：当前换档会把全库终态失败**一次性**重排队（真机 260 个 background 一起排队 ⇒ 十几分钟占满 4 请求/秒门控、阅读变慢）⇒ 改成只重排"当前可见 + 限量"，其余保持 pending 按需唤醒。
3. **评审遗留**：C-4（惰性打开把远端 I/O 放进 `PDFIUM_FFI_LOCK`，需两阶段打开/锁外预热）、C-6（`page_bytes_for_display` 在 Rust 侧零断言，`FakeBook` 未实现它）、I-2（目录视图 `cover` 用默认档、与卡片档位不同 ⇒ 可能漏触发刷新）、I-3（跨档回退最多 4 次 `readCover`，DB 锁流量放大）。
4. **诊断通道开关**：`pdf_diag.log` / `cover_budget` 目前常开（低开销），若要收敛成"仅调试构建"，加一个设置项。
5. TAR / 7Z / RAR：用户已决定**保持整本下载**（库内 0 样本）；CBZ 与本地/远端文件夹图片**无需改**（已惰性/天然按需）。

### D. 验证命令（照抄即可）
```bash
# Rust（**必须** --test-threads=1，并行会因环境变量/DB 争用假失败 13–17 条）
cd app/rust && cargo build --lib
cargo test --locked -j 2 -- --test-threads=1          # 全量门禁；exit 0 才算过
cargo test --locked --test p0_baseline_read_speed -- --test-threads=1   # P0 延迟基线（改共享路径必跑）
# Dart
cd app && flutter analyze lib/ui/comic_cover.dart lib/store/remote_cover_repository.dart
flutter test test/comic_cover_state_consumer_test.dart test/comic_cover_scan_terminal_test.dart test/cover_scan_terminal_integration_test.dart
# 桌面（带 perf 与 A/B 开关）
flutter build windows --debug
$env:RCH_PERF_LOG='D:\Temp\rch-perf-x.jsonl'; $env:RCH_RANGE_PARALLEL_MIN='524288'
# 手机（必须去掉 release 签名环境变量，否则与已装 debug 签名冲突）
flutter build apk --profile --target-platform android-arm64
adb install -r build/app/outputs/flutter-apk/app-profile.apk
```

### E. 接手须知（踩过的坑，别再踩）
- **改 Rust API 后必须** `flutter_rust_bridge_codegen generate`（在 `app/` 下），否则生成物报参数数量不匹配。
- **不要为了让测试过而改测试/放宽阈值**：第 79 轮我改扫页上限时被 `cover_falls_back_to_the_first_decodable_page` 挡回，正确做法是**改成格式感知**而不是改断言。
- **改共享路径必须跑 P0 基线**：我在每次读里加 `env::var` 曾把 `p0a_single_page_latency_without_background_load` 顶爆。
- **手机 DB 取证**：`adb exec-out` 拉 `database.db` **必须连 `database.db-journal` 一起拉**，并且用可写方式打开让 SQLite 自己回滚；应用正在写时必然拿到撕裂副本（表现为 `no such table: ...`）。
- **Android 注不进 `RCH_PERF_LOG`** ⇒ 手机侧只能靠 `pdf_diag.log` / `scan_diag.log`（都在缓存根目录）。缓存根由用户设置决定：桌面 `D:\Documents\RCH`，手机 `/data/data/com.rch.reader/cache/RCH`。
- **adb 路径**：Git Bash 里 adb 命令前加 `MSYS_NO_PATHCONV=1`，否则 `/data/...` 被当 Windows 路径改写。
- **`is_archive` 分支**（`api/remote_scan.rs`）：ZIP/CBZ/PDF/MOBI 等**归档**走 `fetch_cover_from_document` → `open_document`（惰性 ✓）；head 窗口阶梯只用于**非归档**（单张图片）。
- **封面预算语义**：`cover_read_budget_exceeded` 属**可重试**（6h 补偿）；`attempt>=3` 才终态 ⇒ 让路/释放路径**绝不能**消耗 `attempt`（`release_job_lease_on` 已修正为 `MAX(attempt-1,0)`）。

---

## Doing(进行中)

### 使用反馈修复（2026-08-17 长风落反馈，任务 08-17-usage-feedback）
- [x] 条漫模式滚动时页码实时跟随（AppBar 页码）
- [x] 条漫模式底部页码/进度栏（‹ 页码/总数 ›）+ 翻页/跳转
- [x] 书源界面顶栏与状态栏重叠（SafeArea 修复）
- [x] PC 端应用图标更换（紫底白字 RCH，生成多尺寸 ico + 安装器图标）
- [x] Windows Release 构建验证图标生效
- [x] MuMu 安装新版本验证
- [x] 发布 v0.5.1

### M2 AI 高清引擎
- [x] Phase 1: CLI 单次调用方案（临时文件传图）→ **第 39 轮已实施**
  - [x] 制作/配置 librealesrgan-ncnn-vulkan.exe + 模型文件（app/windows/ai/）
  - [x] Rust `ai/` 模块: 单次 Command 调用 + 超时 60s + sha256 缓存
  - [x] Rust `api/ai.rs`: 暴露 `super_resolve(page_bytes, scale) -> result_bytes`
  - [x] Dart: 阅读器右键菜单 → 触发超分 → SnackBar 进度 → 替换当前页显示
  - [x] 结果缓存: ai/ 目录 sha256 key 缓存，重复请求不走 CLI
  - [x] CMake 集成: install(DIRECTORY ai/) 随安装程序分发
- [x] Phase 2: CLI 目录批量模式 → **第 40 轮已实施**
  - [x] `super_resolve_batch()` 一次 CLI 调用处理整个目录
  - [x] ONNX 模型转换（pth → onnx，推理一致性验证通过）
  - [x] 评估 ort crate 在 FRB cdylib 环境中的可行性（结论：不可用，待稳定版）
- [ ] Phase 3: ONNX Runtime 直接推理（模型已转 ONNX，待 ort crate 稳定后切换 + Upscaler trait）

## Backlog(待办)

### 第 95 轮补充结论（2026-09-21）
- [x] **缺库误标已修**：只有自家加载器文案才判 `cover_native_lib_missing`；
      pdfium 库内错误归 `cover_document_open_failed`（终态 —— 但"刷新"可手动重排）。
- [ ] 真机复验：`4.pdf` 这类之前被误标为"缺库"的书，重排后若仍失败，错误码应变成
      `cover_document_open_failed`，据此可判断是文件本身/pdfium 解析问题（而非部署）。
### 第 94 轮补充结论（2026-09-21）
- [x] **"整本读被字节预算拒掉"已取消**（只豁免 `offset==0` 且一次要完整个文件、≤512 MiB 的情形）；
      病态扫描仍由 384 次读 + 30 s 两条挡住。
- [x] **整本缓存清理已拓展**：`cache/raw` 新增 2 GiB 容量上限 + 最旧优先整包淘汰（保留最新包），
      收口在 `register_book`（每次打开书都会跑，best-effort）。
- [ ] **真机复验**：刷新夸克目录 ⇒ 那几张 MOBI 封面应能抓好（不再 `cover_read_budget_exceeded`）；
      若仍失败，`mobi_diag.log` 现在会直接给 `mobi_lazy_declined reason=…`，据此继续。
### 第 93 轮补充结论（2026-09-21）
- [x] **"详情页有封面、墙上显示获取失败"已修**（第93轮）：unified 抛出前补 legacy 纯本地回退。
      真机复验：那几张 MOBI 在墙上应直接出图（不再显示"获取失败"）。
- [ ] **unified 侧对这几本 MOBI 仍失败**（legacy 兜底掩盖了它）：需在**桌面**刷新后读
      `scan_diag.log` 的最新 `cover_fail` 错误码，再决定是压往返预算还是查部署/解码。
### 第 92 轮补充结论（2026-09-21）
- [x] **扫描状态栏三按钮已删**（暂停/继续、增量重扫、全量重扫）；保留"重试远程扫描"。
- [x] **"反复刷新也没用"已修**：重试唤醒必须发生在 `_relist()` 之后（否则无活会话 ⇒ 唤醒空操作）。
      真机复验口径：刷新一次应提示"已重新排队 N 张失败封面"，且 `scan_diag.log` 随后出现该资产的
      新 `cover_fail`（若仍失败）或直接出图（若成功）。
- [ ] 若刷新后仍失败（出现新的 `cover_fail`），按错误码继续查：`cover_read_budget_exceeded`
      ⇒ 看 `cover_budget detail=` 的 reads/ms 是否仍撞预算；`cover_native_lib_missing` ⇒ 部署问题。
### 第 91 轮补充结论（2026-09-21）
- [x] **D7 已做（用户选 ①）**：新增"阅读渲染宽度"设置（省流 1080 / 标准 1600 / 跟随屏幕），
      Rust 页缓存按宽度分目录、换宽清 L1；标准档走历史路径 ⇒ **不作废已有页缓存**。
      验收：`pdf_diag` 阅读页 `width=` 随档位变化；省流档 `out_bytes` 约 1.9 MB → 0.9 MB。
- [ ] 仍待做的 D7 延伸：**长条页切片渲染**（1600×约16000 ≈ 100 MB 中间位图的内存/卡顿峰值），
      属阅读器显示层改造，用户尚未要求。
### 第 90 轮补充结论（2026-09-21）
- [x] **G1 已做**（第90轮）：钉住文件头窗口（零额外请求/字节）+ `META_WINDOWS` 2→4；
      契约 `tests/source_reader_head_pin_contract.rs`；A/B 证明无读放大回归、门禁 25 套件 540 passed。
- [ ] **D7 待用户拍板（画质权衡）**：阅读页 `out_bytes ≈ 1.9 MB/页`（经 FRB 交给 Dart），
      降低渲染宽度可降这笔（1280→约 1.3 MB，1080→约 0.95 MB），但会牺牲清晰度 ⇒ 需要选择：
      ① 增加"渲染宽度/清晰度"设置（省流 1080 / 默认 1600 / 屏宽）；② 维持 1600；
      ③ 另做**长条页切片渲染**（治 1600×约 16000、约 100 MB 中间位图的内存/卡顿峰值）。
- [ ] 注：**"按屏宽渲染"在本机是反向优化**（屏 2560 > 现有 1600），不要再照原计划做。
### 第 89 轮补充结论（2026-09-21）
- [x] **MOBI 页表缓存已实现**（第89轮）：首次打开仍逐条探测，**之后同一本书零探测打开**
      （单测 45 → 5 次远端读，≈9 倍）。验收口径：`mobi_diag.log` 第二次打开应为
      `mode=lazy-cached cache=hit`，`ms` 从 ~10 s 掉到 <1 s。
- [ ] **待真机复验**：桌面已装并重启（PID 15952）；手机包在
      `C:/Users/cfl/Downloads/RCH-0.5.8+100508-20260921.apk`（手机未插，插上即 `adb install -r`）。
- [ ] 首次打开还能更快：`RCH_MOBI_PROBE_WORKERS`（默认 4）可调；缓存命中后与它无关。
### 第 88 轮补充结论（2026-09-21）
- [x] **115/夸克"原文件名显示 id"已修**（第88轮）：判定放宽 + 向前找第一个非 id 段 + 3 个新用例。
- [x] **失败封面可重试**（第88轮）：`remote_cover_retry_failed` 挂到源浏览器"刷新"动作。
      ⇒ 下次真机复验：刷新一次应看到"已重新排队 N 张失败封面"，墙上随之翻牌。
- [ ] **速度分析结论（待真机日志确认）**：首次打开 ≈ 页表探测 200–300 次（4 并发 ≈10 s）；
      "读完流畅" = 逐页缓存（与整本下载无关）。`reader_diag.log`/`mobi_diag.log` 已就位，
      需要用户用最新版读一本 MOBI 取"stream vs fallback-download"的证据。
      下一步最优先仍是**页面表缓存**（首次 ~10 s、之后秒开）。
### 第 87 轮补充结论（2026-09-21）
- [x] **可观测性补齐**：`reader_diag.log`（打开模式/耗时）+ `mobi_diag.log`（惰性/回退、每页 bytes/ms）。
      下一步实测"MOBI 到底走 stream 还是 fallback-download"只需用户再读一本。
- [ ] **页面表缓存（推荐下一步）**：MOBI 首次打开仍要 ~300 次探测（4 并发 ≈10 s）。缓存页表
      ⇒ 首次 ~10 s、之后秒开。需要先定缓存键与失效口径（建议 `source_id + path + 文件长度`）。
- [ ] **"超过 N 秒转整本下载"开关（暂缓，预期收益为负）**：实测吞吐 4.4 MB/s ⇒ 76 MB ≈17 s，
      比现状更慢；且 raw 包读完即删 ⇒ 每次打开重付。若要做，默认关闭。
- [ ] **P0 flake 记账**：`p0a_single_page_latency_without_background_load` 是**下限**断言
      （要求至少一页 ≥500 ms，`tests/p0_baseline_read_speed.rs:862`），机器快就挂。
      可选修法：改为断言机制量（range 请求数/门控等待）而不是墙钟下限 —— **需用户拍板**（属 P0 装置）。
### 第 86 轮补充结论（2026-09-21）
- [x] **"详情页出图后墙不刷新"已修（第86轮）**：无 asset id 的卡片不再早退，改挂 revision 监听 +
      唤醒后重跑取图（本地优先）。**待补自动化回归**：需要一个能让无 asset id 卡片走到取图出口的
      widget 测试夹具（测试环境里 `_readLocalDiskCover` 的原生读会先失败）。
- [ ] **手机端剩余封面失败（pdf/zip 为主）仍待 G1**：见第 85 轮结论。
### 第 85 轮补充结论（2026-09-21，实测 + 用户真机反馈）
- [x] **MOBI 阅读慢（首开 RTT 受限）已在第85轮加速**：`concurrent_probe`（默认 4 worker，
      `RCH_MOBI_PROBE_WORKERS` 可覆盖）。若仍嫌慢，下一步是**页面表缓存**（一次探测、长期复用）。
- [ ] **手机端剩余"大量封面获取失败"主要在 PDF/ZIP**（真机日志分布 pdf=75 / zip=17 / mobi=9；
      MOBI 封面已由第83轮解决）：每条封面 5–18 次串行读 × 136–182 ms ⇒ 撞 30 s 挂钟预算。
      **与 G1 同源**：优先做 G1（元数据读合并 + 钉住/复用头窗口）。
- [ ] **详情页出图后海报墙不刷新**（真机反馈）：墙上的卡片只在**源级 cover revision** 变化时重读。
      待查：`mark_job_ready_owned_on` / worker 发布路径是否 bump `view_revision` + notify；
      若缺，补 bump + 一条"发布即唤醒"的回归测试。

### 第 84 轮补充结论（2026-09-21，实测）
- [ ] **G1 是 PDF/EPUB/ZIP 翻页慢的主因（已量化）**：`reads=5` 的 196 页平均 **909 ms**
      （≈182 ms/次串行 Range），而 229 KB 的页 411 ms、4.57 MB 的页 1030 ms ⇒ **与字节数脱钩**。
      目标：每页往返 5–6 次 → 1–2 次（翻页 ~0.9 s → ~0.2–0.4 s）。旁证：`pdf_open mode=lazy`
      本身正常（12–18 次读 / 19–27 KB / 1.1–1.6 s）。
- [ ] **MOBI 阅读慢 ≠ G1**：第83轮只修了**封面**入口（`open_cover`），阅读仍在 `open_lazy` 逐条探测
      全部候选记录（N≈页数）。三选一：**(a) 页面表缓存**（推荐，需定键与失效口径）、
      (b) 渐进打开（先探前 K 条、其余后台补齐并更新页数）、(c) 信任 `first_image_index..end`
      不探测（最省但可能多出资源记录造成的空页）。
- [ ] **D7（长条页字节量）**：宽 170 的页仍 `ask_bytes` 1.9–3.1 MB；按屏宽渲染/切片可减少传输，
      但属**画质口径**，需用户拍板（与交接单 C.1 同源）。

### 第 82 轮审计遗留（2026-09-21）
- [x] **PDF 打不开（Release 缺 `pdfium.dll`，第83轮已修）**：`app/windows/CMakeLists.txt` 新增
      `install(FILES pdfium.dll)`（来源 `app/windows/pdfium/win-x64/`，已 gitignore），Debug/Release
      都自动带上；此前只有 Debug 有那份历史手工拷贝。**教训：切构建档位后必须核对运行时依赖是否齐全。**

- [x] **D1a MOBI 封面少探测（2026-09-21 第83轮已修）**：原方案"合并探测窗口"经测算**无效**（记录头隔着整张图 ⇒ 读次数不变），改为 `MobiBook::open_cover` + `document::open_cover_document`：封面只探测到第一张可解码图片就停（1–3 次读 vs 200–300 次），阅读路径不变；回归测试 `cover_open_probes_only_until_the_first_image`。**原条目保留供追溯**：：`document/mobi.rs:215-230`
      现在对**每条候选记录各发一次 16 B 远端读**（200–300 次 Range × ~136 ms）⇒ 撞穿封面 30 s
      预算（`cover_read_budget_exceeded` 281 条）并让打开变慢；zip/epub/pdf 都接了
      `SourceReader`（256 KiB 预读 + 小窗口合并），**只有 MOBI 裸读**。目标：任意页数下
      `mobi_open reads ≤20 / bytes ≤16 KB`。
- [ ] **D1b 探测失败不要整体回退**：`mobi.rs` 里任一次 `read_exact_at(...).ok()?` 失败即放弃惰性
      ⇒ 回退**整本读入**（76 MB）；只有头部/记录表读不到才应回退。
- [ ] **G1 减少串行往返（"别的文件也慢"的共同根因）**：每页 7.8–16.1 次 Range、每次 RTT p50
      88–150 ms；EPUB 会话 906/1265 次是 <512 B 小读（占 62% 耗时）；同一 `offset=0` 被重取
      108–135 次（`SourceReader` 仅 2 个元数据窗口槽）。改造方向：元数据读合并、钉住/复用头窗口、
      按条目一次性读。**并行 Range 已被第 81 轮 A/B 否掉（慢 ~8%），不要再提**。
- [x] **D4 磁盘已有封面读不出来（2026-09-21 第83轮已修）**：Rust `read_cached_cover` 在无 durable ready 行时用 `remote_cover_ref.owner_key` 反推 content_revision 读同一份字节（纯只读），Dart 卡片在非 ready 时先做一次纯本地读再回退占位；回归测试 `ref_backed_read_serves_bytes_when_durable_rows_were_purged`。**原条目保留供追溯**：：`remote_cover_variant=0` 而 `blob=1061/ref=1061`（1.2 GB）；
      读图被 durable `state='ready'` 门控（`cover_service.rs:110-148`）。方案：用 ref+blob 重建
      variant 行，或读路径加只读回退。
- [ ] **D3 读路径不再回写状态**：`cover_service.rs:159-208` 的 `ready→pending` 对账 + bump + notify
      发生在**卡片读缓存**路径上 ⇒ 读一次就把自己的文案打回"等待"。应移出读路径或按字节过期节流。
- [ ] **D6 locked-frame 内不推 notifier**：卡片挂载期的封面加载会在 locked frame 内推
      `RemoteScanCoordinator._setStatus` 的 ValueNotifier（`errors.log` 136 条 `widget tree was locked`）。
- [ ] **D7 正文页=单条 5–15 MB 记录不可分片**（阅读侧按屏宽渲染/长条切片，属**画质口径**，需用户拍板；
      与交接单 C.1 同源）。
- [ ] **D8 误导性文案**：流式阅读时也会显示「正在下载漫画…首次阅读需下载整本」（夸克分支无条件起
      进度轮询，非下载时进度函数返回 1.0）⇒ 文案应按真实策略显示。
- [ ] **观测盲区**：MOBI 路径**零埋点**（PDF 有 `pdf_diag.log`）⇒ 建议补 `mobi_diag.log`
      （`mobi_open reads/bytes/ms`、`mobi_page index/ms/reads/bytes`），否则 D1 修完无法验收。


### P1 后续改进（2026-09-18 登记）

- [ ] **持久化 115-web / Quark 的远程 raw-cache 身份**（provider 返回的文件名）。
      这是让 `Family 2`（115-web / Quark 历史 raw-key 封面）从 *known unrecoverable*
      变为可恢复的**唯一**路径；未持久化前，raw 文件被删即不可恢复。
- [ ] **ZIP 依赖升级 / 仅中心目录打开归档的可行性调研**（当前需读中央目录；升级后可能支持流式打开）。
- [ ] **SFTP source authority 归属整合**（cache authority 与 endpoint 解析的职责边界收口）。
- [ ] **Cover cache identity 统一**（远程 raw cache 与 cover blob 的身份/路径规则统一）。

### 远程封面 RG-B ③（2026-09-19 登记）

- [x] **①** 封面失败安全子原因归因（`provider:notFound/forbidden/unauthorized/rateLimited/timeout/decodeFailed/noCover/other`）—— `730ce77`
- [x] **③-1** 分级窗口（24/64/128MB）+ 删除 `cover_size_limit` 放弃分支 + 顶部裁带 + 具体失败码落库 + `bytes_fetched` 埋点 —— 第 51 轮
- [x] **②-b** 有界回填取数（新工具 `examples/cover_failure_triage.rs`，只作用于 DB 副本）—— 第 52 轮；**结论：275 个夸克失败＝267 PDF（缺 `pdfium.dll`，部署）+ 8 MOBI 行（真问题）**
- [x] **② 自愈策略**（第 53 轮）：可修复失败（`cover_native_lib_missing` / 字节·像素上限 / 窗口截断 / `provider:rateLimited·timeout·unauthorized·forbidden`）获得一次 6h 长期补偿；永久失败仍不自愈
- [x] **测试根隔离 + CI 串行**（第 53 轮）：`cache_root()` 测试构建锚定 `<TEMP>/RCH-test-<pid>`；CI 改 `cargo test --locked -- --test-threads=1`；实测默认根库两次全量跑均 0 写入
- [x] **重建 + 重启 + 验证**（第 58 轮，用户授权代为执行）：`flutter build windows --debug` → 启动清理 **53 行命中离线预测**、孤儿 53→0；⟳ 刷新按钮**视觉确认**（截图）、三个测试源已消失
- [ ] **⚡ 触发自愈**：点一下**夸克 / 115** 建立会话（重启后会话是进程内的，现在显示"需连接"）⇒ 才会走新的"按指纹解析 + (1b) 重挂 + 具体失败码"路径，存量 284(夸克)/439(115) 才会开始塌缩
- [x] **③ 格式专用（MOBI 真问题）**（第 54 轮）：`MobiBook` 只保留魔数可解码的图片记录（`image_records()` 的非图片黑名单会漏进 KF8/AZW3 的 CSS/HTML 资源）+ 封面有界换页兜底（≤3 页）+ 魔数嗅探诊断（`cover.probe`）；实测 33.9MB MOBI 由 `cover_decode_failed` 转为 **ready（8s）**
- [x] **真实库扫描审查 + 115 自愈**（第 55 轮，报告 `../../.trellis/tasks/09-14-remote-cover-cleanup/research/rg-b/2026-09-19-scan-audit-real-library.md`）：115 源 225 个 `notFound`＝**同指纹双源 `library_index.id` 冲突**（直连 115 与同步镜像 `sync_…` 指纹相同 ⇒ PK id 相同 ⇒ 行被对方持有，解析器又按 `source_id` 查）⇒ 已修：按指纹语义解析 + preview 去 `Running` 门槛 + 自愈重挂 (1b) + `scan_diag` 诊断通道；真实库离线核对命中 225/225 + 214/214，副本端到端实跑 **10/10 出封面**
- [x] **删除真实库测试源**（用户已在应用内删除；第 57/58 轮补齐 `remote_*` 残行：启动清理 53 行 ✓）
- [ ] **可选（需先停应用）**：同指纹双源的行做一次显式归一（当前靠"按指纹解析"兼容，不改数据也能工作）
- [x] **封面快通道（方案 D）**（第 62/63 轮）：最小 ZIP 读取器（EOCD+中央目录+首图片，**请求数 O(1)**）—— A/B 实测 2.08GB CBZ：192 次读被预算截断 → **8 次读 9 秒拿到封面**；预算码改可重试 + (1b) 桶纳入；调研报告 `../../.trellis/tasks/09-14-remote-cover-cleanup/research/rg-b/2026-09-19-zip-cover-open-research.md`
- [x] **启动自动重连书源**（第 64 轮）：`warmUpSessions` 预热已保存凭据的源（零点击恢复会话/扫描/封面），实测吞吐 ≈14 枚/分钟（≈6×）
- [ ] **待确认**：存量 `cover_read_budget_exceeded` 失败行（115 共 17）为何未被 (1b) 重挂（该次 session-ready 未见 `cover_reconcile` 埋点）
- [ ] **③ 格式专用（真问题）**：PDF 封面 = 整包下载（实测 17MB / 33.9MB 每枚）⇒ range 化 pdfium 读取
- [ ] **可选**：BMP/TIFF/HEIF 现在只能**命名**（`cover.probe.magic`）但解不开 ⇒ 若分布里出现这些标签，给 `image` 加对应特性
- [ ] **④ R1** 卡片直读缓存优先 + 递归祖先链（现有 351 个 cover blob vs "可用" 10–29）
- [ ] **② 死角**：`blocked` / `retry_wait` 的冷却恢复路径
- [ ] **相邻**：阅读器远程图片页上限 `MAX_REMOTE_PAGE_BYTES`(32MB) 与长条图（本轮未动，需单独确认）


### 规划完成(待开工) — 2026-08-02 批量规划
- [ ] `08-02-m5-book-sources` — M5 书源扩展(SMB / SFTP)
- [ ] `08-02-tag-meta-hierarchy` — 元数据标签按作者/类别/系列/状态分层折叠
- [ ] `08-02-reader-zoom-pan-bug` — 修复缩放后移动区域只在第一页生效
- [ ] `08-02-extension-alias-dedup` — 后缀名变更识别(zip→cbz 视为同一本)
- [ ] `08-02-export-cbz` — 本地漫画转 CBZ(文件夹/ZIP 打包)
- [ ] `08-02-avif-support` — AVIF 格式支持
- [ ] `08-02-reader-page-rotate` — 阅读器页面旋转(M4 子集)
- 已删除:阅读器核心 backlog(双页拼接自动模式 / 滚轮缩放 / 自定义按键绑定),用户确认不做

### 后续里程碑
- [x] ADR-016: Repository 层实施（Tags 已完成，Books/History/Settings 待后续）（第26-29轮已实施 TagRepository）
- [x] ADR-017: 标签独立建模（Tag 实体 + BookTag 关联，解决标签补全 Bug）（第26-29轮已实施）
- [ ] M4 复杂场景(智能拼页 / 裁边;旋转已拆为 `08-02-reader-page-rotate`)
- [ ] M5 书源扩展(SMB / SFTP / 更多网盘) — 已建任务 `08-02-m5-book-sources`(SMB+SFTP)
- [ ] M6 Android 适配(手机 / 平板)
- [x] M7 标签筛选:按标签过滤书架(标签数据模型 + 跨书源搜索已落地)
- [ ] M8 Smart Scraping: catalog-only recognition + automatic sync integration (M8-M1 proposal slice frozen after real-sample validation; enrichment and canonical confirmation remain)
  - [ ] M8-A0 Automation Coordinator & Sync Integration: implemented; verify startup, debounce, periodic and sync-before-scrape behavior
  - [x] M8-M1 Catalog-Only Name & Role Extraction (`catalog-rules-v3`): frozen after after8 347-row local/Quark validation; proposal-only, zero remote book-source I/O
  - [ ] M8-M2 Canonical Identity & Migration: ordered DDL, works / external IDs / work links
  - [ ] M8-M3 Optional Provider Enrichment: independent AniList + Bangumi runtime
  - [ ] M8-M4 Candidate & Explainable Ranking
  - [ ] M8-M5 Review, Confirmation & Sync-Dirtiness
  - [ ] M8-M6 Corpus Validation: 100 real comics and role-confusion matrix
  - Later: M8.1 Provider Expansion; M8.2 Advanced Evidence; M8.3 Metadata Taxonomy; M8.4 Discovery; M8.5 Export & Interop

---

## Done(已完成)
- [x] 立项与总体规划(SPEC / LOG / DECISION / 文档体系建立)
- [x] ADR-016/017 标签系统重做 + 搜索系统统一（第27-29轮）
- [x] M9 缓存基础设施: 五级缓存 + 统一下载器 + SQLite 数据层（第30-32轮）
- [x] ADR-018 Repository 层扩展到 Book + Record（第36轮）
- [x] 核心阅读闭环: 书源 + ZIP/CBZ 流式 + 三模式 + 双页拼接 + WebDAV
- [x] 8种格式引擎: EPUB/Folder/CB7/CBT/PDF/CBR/MOBI
- [x] 标签系统: 补全/筛选/管理/批量/元数据标签/已读标记
- [x] 搜索系统: 内联补全 + Chip + 跨书源全局搜索
- [x] 缓存体系: 五级分类 + 独立管理面板 + 封面懒加载
- [x] 封面自定义: 选页 + 裁剪 + 质量可调
- [x] 数据层: SQLite 迁移 + Repository 层 + 封面磁盘缓存 + 并发加载限流（第36-38轮）
- [x] 文档体系: (LOG / LOG-INDEX / README / TODO / DECISION / SPEC)
- [x] 书源同步导出补全: 手动导出到文件 + 加密书源凭据包 + Android 导出降级（第42轮）

## Waiting（待验证 · 2026-09-20）

### 1. EPUB 打开优化（**第 78 轮已实施**，待用户实测确认）
- [x] 三步一次成型落地 —— `app/rust/src/document/epub.rs` **整体重写**（未用片断编辑）：
      `CdArchive` 接线（只读 EOCD + 中央目录；不可解析/不支持的压缩方式整体回退 crate）·
      **起点惰性缓存**（打开期零 local header 读；某条目首次被访问时才读它的 local header ——
      与数据合并成一次 `read_at` —— 并把算出的数据区起点缓存给后续读取）· 页表只存**条目索引** ·
      章节 xhtml **按需解析**（open 只登记 spine 表，首翻才读 + 缓存）
- [x] ZIP 构件（`app/rust/src/document/zip.rs`）**只增不改**：新增"local header 与数据合并成一次
      `read_at`"的构件（`read_entry_bytes_tracked` / `read_entry_at` / `decode_entry_bytes`），
      **仅 EPUB 快路径使用**；`read_entry_bytes` 与"每页 2 次读"的 P0 契约一字未动
      （只删了 `first_image_bytes_via_central_directory` 里一行未使用的 `let len`，零行为改动）
- [x] 门禁（2026-09-20 实测）：`cargo build --lib` 0 error / 0 lib 警告 →
      `document::` 35 passed（`document::epub` **8/8**：原 4 条回归 + 新增 4 条）→
      `cargo test --locked -j 2 -- --test-threads=1` 全量通过 → `p0b2_zip_read_amplification` 9/9
- [x] 独立评审（fresh-context、只读）后的修正：把"按名找条目"收敛成**两个后端同一条规则**
      （精确 → 大小写不敏感），删掉 77 轮 shim 里那处会**静默选错条目**的"后缀匹配"；
      回退路径去掉新加的 512 MiB 上限；`CdArchive::new` 的读取错误也回退 crate；
      补 3 条测试（打开只读尾部 / `<img src>` 大小写不一致 / 快路径不可用时回退）
- [x] 验收口径实测：**300 条目漫画 EPUB 打开 4 次读**（≤4 ✓；容器 + OPF 合并读）
- [x] **真机复核修复（同日）**：应用实测"打开 EPUB 还是很慢" ⇒ perf 日志显示快路径在生产上弃权
      （393 次 `requested=30` 的逐条目 local header 读）。新增只读诊断工具
      `examples/quark_epub_probe.rs`（从 DB 副本读 cookie；结构体检 + 改前/改后 A/B）定位到根因：
      真机文件 **EOCD 之后多出 5 B 尾巴**，而 shim 的判定要求"EOCD 正好落在文件末尾"。
      修复：严格候选优先、放宽候选需中央目录头签名验证（干净归档零额外读）。真机两本实测
      **219 / 204 次读 → 4 次读**（30.8 s / 28.2 s → 0.59 s / 0.58 s）；全量门禁 exit 0。
- [x] 探针 A/B（`examples/read_profile.rs --file … [--legacy]`，合成 300 条目 EPUB 654 KB）：
      打开 **47 次读 / 651,836 B → 4 次读 / 90,196 B**；首次翻页 2 次读（章节 + 图片）
- [ ] **待用户实测**：真实 115 上漫画 EPUB 的首次打开耗时是否达标；
      若某本 EPUB 页数或翻页异常（章节不含 `<img>` 的 nav 文档会占一页），回报即可
      —— 取舍与理由见 LOG 第 78 轮

## 下次开工（Backlog）

### 1.5 挂账（第 78 轮评审后登记）
- [ ] **中央目录复用已读的尾部窗口**：EOCD 与中央目录同在文件最后 64 KiB 内，可省掉每次打开
      1 次往返（EPUB 打开 4 → 3 次读）。第 78 轮**已实现又撤销**：它会把
      `p0a_disk_cache_hit_bypasses_the_network_gate` 的测量窗口推进上一相位的预取线程噪声里
      （HEAD 5/5 ok → 4/5 FAIL）。要与 `tests/p0_baseline_read_speed.rs` 的测量口径一起处理。
- [ ] 若仍有 EPUB 打开异常上报：优先看 `LOG.md` 第 78 轮"有意取舍"三条（章节页数模型 /
      无图章节报错 / 跨章节图片不去重），并考虑给 nav 文档（EPUB3 `properties="nav"`）做 spine 过滤。

### 2. 其余格式（**按真实库分布**排序，逐个照 EPUB/PDF 的做法）
实测分布（`library_index` 全库）：`.zip` 786 / **`.pdf` 609** / `.cbz` 240 / `.mobi` 19 /
`.epub` 20 / `.rar`·`.cbr` **0** ⇒ 顺序按证据重排：
- [x] **PDF（第 79 轮已完成 + 真机续修）**：`PdfBook::open` 改**惰性按需读**（pdfium
  `FPDF_FILEACCESS` 回调 + `SourceReader` 摊薄 + "填满或报错"适配器；失败回退整份读入）。
  真机 `1.pdf`（57.5 MB，230 页）打开+首页 **57,506,080 B → 291,756 B（197×）**，渲染**逐字节一致**。
  真机续修（同日，用户反馈驱动）：① **后台封面给阅读让路**（前台读页 20 s 窗口内 background
  封面释放租约让路，visible 不受影响）；② `Document::page_bytes_for_display` 让封面按 340px
  渲染（1029 ms/3.36 MB → **36 ms/265 KB**）。
- [ ] **PDF 任务 3（待评估）**：阅读页按屏宽渲染 + webtoon 长条切片（真机实测 1600px 长条页
  单页 0.7–6.8 s、输出最大 7.3 MB；会动画质，需单独确认）
- [ ] **MOBI（19 个，第 81 轮进行中）**：改成**惰性按需读**。现状：`vec![0u8; len]` 整份读入，
  且 `pages: Vec<Vec<u8>>` 把每张图再复制一份（峰值 ≈ 2× 文件大小）⇒ 每枚封面 = 整本
  74–180 MB（截图实证：≤79 MB 有封面、143/146/180 MB 撞 `COVER_FETCH_LIMIT_BYTES=128MB` 被拒）。
  做法：自己解析 PalmDB 头（78B）+ 记录偏移表（在文件头部）⇒ 只读"头 + 记录表 + 封面记录"，
  `mobi` crate 的整份解析保留为**回退**（KF8/AZW3 走 INDX 的情况）。
- [ ] **7Z / TAR**：**用户确认暂不改**（保持整本下载阅读；本身格式不多）
- [ ] **RAR/CBR**：**用户确认暂不改**；库内 **0 样本**（真要做得另设计：RAR 无中央目录，
  只能"头部顺序扫描 + 按需单文件提取"，且 libunrar 只接受文件名/临时文件）
—— 全格式审查表见 LOG 第 77 轮

### 2b-1. CBZ 与"文件夹图片"体检（第 80 轮补记，结论：**都不用改**）

- **CBZ**：`.cbz` 走的就是 `zip.rs`（dispatcher `mod.rs:57` 把 `.zip` 与 `.cbz` 归为同一条）
  ⇒ **已经是惰性**：优先只读 EOCD + 中央目录自建页表、逐页按需读（第 70/78 轮），
  封面还有第 63 轮的"封面专用最小 ZIP 读取器"（请求数与文件大小/条目数无关）。用户库里 240 本
  CBZ 已在受益。
- **本地文件夹图片**（`folder.rs`）：直接用 `std::fs` —— 打开 = 一次 `read_dir`，
  取页 = 只读那一张图片文件 ⇒ **天然按需**，不存在"整包"问题。
- **远端文件夹图片**（`remote_folder.rs`）：打开 = 列目录（一次列表调用），
  `page_bytes(index)` = 按需取那一张 ⇒ 同样**天然按需**。

**总计**：用户库 1674 本（zip 786 / pdf 609 / cbz 240 / epub 20 / mobi 19）里，
除 MOBI 外的 1655 本**都已经或即将是惰性的**；剩下 TAR/7Z/RAR 三个格式库内 **0 样本**，
按用户决定保持整本下载。

### 2b. 全格式"整份读入"体检结论（第 80 轮补记，回答"差异大不大、能否一起改"）

| 格式 | 现状 | 改造难度 | 结论 |
|---|---|---|---|
| `zip.rs` / `epub.rs` / `pdf.rs` | **已惰性** | — | 无需改 |
| **`mobi.rs`** | 整份读入 + 每图再复制一份 | **中**：PalmDB 头与记录表在文件头部 ⇒ 可只读所需记录；但 `mobi` crate 只吃整份，KF8/AZW3 的图片要走 INDX ⇒ 需自解析头 + `first_image_index` 并保留回退 | **本轮做** |
| `sevenz.rs` | 整份读入 | **难**：7z 头在文件尾 + 默认固实压缩，单文件数据可能依赖前面的块 | 不做 |
| `tar.rs` | 整份读入 | **中**：顺序格式，可 seek 扫头，但必须分批读（否则一条目一次请求 = 第 70 轮 ZIP 老毛病）| 待有样本 |
| `rar.rs` | 整份读入（还要落临时文件）| **最难**：`unrar` 只接受文件名/临时文件 | 不做（0 样本）|

**结论**：症状同形、**难度差异很大** ⇒ **不一起改**；本轮只做 MOBI。

### 3. 小尾巴
- 桌面端**白天主题目视复核**：设置 → 外观与布局 → 主题 → 白天 ⇒ 截图逐屏 `read_image` 复核；
  待定项：`home_page` 1 处 + `book_detail_page` 2 处（疑似背景填充）、`source_browser` 7 处深色文字、
  `comic_cover.dart` 一处 const 子树内的 TODO
- 夸克扫码链路（一次性 ticket 修复后）待用户实扫验证；手机端「保存到相册」已修成真 PNG 待复验

## Backlog（2026-09-22 新增）

- [ ] **WebDAV 封面：索引 `size` 无法落库（扫描发布阶段）** ✗
  现状：海报墙**能显示封面** ✓（走本地/详情页回退），但状态行统计 **失败 231** ✗（`cover_size_missing` 212）。
  已证实：服务器 Depth:1 带全量大小 ✓ → 应用解析 ✓ → provider 列表 257/257 带 size ✓ → 暂存 JSON 含 size ✓ →
  自愈判定成立并会重列 ✓ → `stage_directory` 两道门无报错 ✓；但 `library_index.size` 仍为 NULL ✗、
  `remote_scan_preview` 无 webdav 行 ✗。
  任务卡：`.trellis/tasks/09-22-webdav-cover-index-size/prd.md`（含下一步与验证命令 ✓）
  注意：定位完成后需删除临时探针 `heal_probe` / `list_dir_probe` / `stage_cancelled` / `stage_failed` ✓
