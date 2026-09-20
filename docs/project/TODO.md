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
- [x] **真实库扫描审查 + 115 自愈**（第 55 轮，报告 `docs/reports/rg-b/2026-09-19-scan-audit-real-library.md`）：115 源 225 个 `notFound`＝**同指纹双源 `library_index.id` 冲突**（直连 115 与同步镜像 `sync_…` 指纹相同 ⇒ PK id 相同 ⇒ 行被对方持有，解析器又按 `source_id` 查）⇒ 已修：按指纹语义解析 + preview 去 `Running` 门槛 + 自愈重挂 (1b) + `scan_diag` 诊断通道；真实库离线核对命中 225/225 + 214/214，副本端到端实跑 **10/10 出封面**
- [x] **删除真实库测试源**（用户已在应用内删除；第 57/58 轮补齐 `remote_*` 残行：启动清理 53 行 ✓）
- [ ] **可选（需先停应用）**：同指纹双源的行做一次显式归一（当前靠"按指纹解析"兼容，不改数据也能工作）
- [x] **封面快通道（方案 D）**（第 62/63 轮）：最小 ZIP 读取器（EOCD+中央目录+首图片，**请求数 O(1)**）—— A/B 实测 2.08GB CBZ：192 次读被预算截断 → **8 次读 9 秒拿到封面**；预算码改可重试 + (1b) 桶纳入；调研报告 `docs/reports/rg-b/2026-09-19-zip-cover-open-research.md`
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

### 2. 其余格式（按收益/工作量排序，逐个照 EPUB 的做法）
**RAR（最严重：打开就整包下载到临时文件）→ PDF（整份读入 + pdfium 需自定义 range 读回调）→
MOBI / 7Z / TAR（整份读入）** —— 全格式审查表见 LOG 第 77 轮。

### 3. 小尾巴
- 桌面端**白天主题目视复核**：设置 → 外观与布局 → 主题 → 白天 ⇒ 截图逐屏 `read_image` 复核；
  待定项：`home_page` 1 处 + `book_detail_page` 2 处（疑似背景填充）、`source_browser` 7 处深色文字、
  `comic_cover.dart` 一处 const 子树内的 TODO
- 夸克扫码链路（一次性 ticket 修复后）待用户实扫验证；手机端「保存到相册」已修成真 PNG 待复验
