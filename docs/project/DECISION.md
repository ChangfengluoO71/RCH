# DECISION.md — 架构决策记录(ADR)

> 只记录**重大、长期有效**的设计决策,不记录日常施工(日常见 LOG.md)。
> 每条含:日期、状态、背景、决策、理由、备选、影响。

---

## ADR-001 技术栈:Flutter(UI)+ Rust(核心引擎)
- **日期**:2026-07-26
- **状态**:已定
- **背景**:Windows 优先、未来 Android(手机/平板);需高性能流式 IO / 解压 / 解码 / 端侧 AI。
- **决策**:Flutter 一套 UI 跨 Windows + Android;Rust 核心引擎经 flutter_rust_bridge v2 桥接(cdylib)。
- **理由**:Flutter 桌面稳定、移动成熟、一套代码两端;Rust 性能强、内存安全、可跨端编译,契合流式与端侧 AI。
- **备选**:Tauri(Rust+Web)、原生 WinUI3、Qt/C++。
- **影响**:核心与界面分离;书源/格式/AI 全在 Rust,UI 只负责呈现。

## ADR-002 流式阅读基石:统一 ByteSource(Range 随机读)
- **日期**:2026-07-26
- **状态**:已定
- **背景**:目标是几 GB 压缩包 / 网盘文件"即点即读、边下边读"。
- **决策**:一切书源抽象为支持 Range 的只读字节流(`ByteSource: len + read_at`);ZIP/CBZ 只读文件尾部中心目录、按需解压单页;格式解析器只面向 ByteSource 编程。
- **理由**:无需整包下载/解压即可读任意页;书源(本地/WebDAV/未来网盘)与格式解析解耦,均可插拔。
- **影响**:奠定流式阅读与书源可扩展的架构基础。

## ADR-009 AI 超分引擎方案:librealesrgan-ncnn-vulkan CLI 子进程

- **日期**:2026-07-30
- **状态**:已定
- **背景**:M1 核心阅读闭环基本完成,用户决定启动 M2 AI 超分。SPEC 原定方案为 NCNN + Vulkan FFI 直调。经调研,NCNN C++ 库需手写大量 FFI 绑定,跨平台编译(NCNN + Vulkan SDK)构建复杂度极高。
- **决策**:第一阶段采用 CLI 子进程方案:预编译 `librealesrgan-ncnn-vulkan` 为独立 exe,Rust 通过 `std::process` 调用,图片经临时文件/管道传递。后续可优化为 FFI 直调或 ONNX Runtime。
- **理由**:CLI 方案解耦彻底,可独立升级模型和推理引擎;开发最快,可先跑通端到端超分流程;后续性能优化时再下沉。
- **备选**:
  - A) NCNN FFI 直调 — 性能最优但构建极复杂,先搁置
  - C) ONNX Runtime + DirectML — Rust 原生集成但模型需转 ONNX,GPU 推理成熟度不如 Vulkan
- **影响**:Rust 侧新增 `ai/` 模块(进程管理/超分/进度);Dart 侧阅读器右键菜单增加超分入口;模型文件(.param/.bin)需随应用分发或首次运行时下载。

## ADR-010 格式扩展引擎:MuPDF 统一 PDF/EPUB + mobi crate

- **日期**:2026-07-30
- **状态**:已定
- **背景**:M3 需支持 PDF/EPUB,M5 需支持 MOBI。用户要求 PDF/EPUB/MOBI 统一用 MuPDF 方案,CBR 暂不做。
- **决策**:
  - PDF/EPUB:采用 `mupdf-sys` crate(绑定 Artifex MuPDF C 库,Apache 2.0 许可),一个引擎覆盖 PDF + EPUB + XPS 等。适配为 `Book` trait 实现,与现有 ZIP/CBZ 路径统一入口。
  - MOBI:采用 `mobi` crate(v0.8.0,纯 Rust),独立实现 `Book` trait。
  - CBR:暂不实施(unrar 许可问题搁置)。
- **理由**:MuPDF 是成熟商业级引擎,PDF 渲染质量业界最佳,EPUB 支持完整。`mupdf-sys` 已有 v0.5.0,Windows 编译链路成熟。MOBI 格式简单(本质是 PalmDOC + HTML),纯 Rust `mobi` crate 足够。
- **备选**:
  - B) pdfium + 自研 EPUB — 两套代码路径,维护成本高
  - C) 纯 Rust(lopdf + epub-parser) — PDF 渲染能力弱
- **影响**：Rust `document/` 模块扩展分发逻辑；Rust 依赖新增 `pdfium-render`。

## ADR-003 AI 高清引擎:NCNN + Vulkan(端侧超分) — 已更新

- **日期**:2026-07-26(原版), 2026-07-30(方案更新)
- **状态**:已定,方案由 FFI 直调改为 CLI 子进程(见 ADR-009)。
- **决策**:端侧超分采用 librealesrgan-ncnn-vulkan CLI,纯本地推理、不上传图片、不耗 Token。
- **影响**:后续里程碑接入;渲染管线预留接口。

## ADR-004 阅读页数据流:Rust 只解压不解码,Flutter 负责像素解码
- **日期**:2026-07-26
- **状态**:已定
- **背景**:① photo_view `customChild` 缩放失效(无法确定缩放边界);② Rust 解码后传 RGBA 体积大(单页约 16MB),缓存页数受限。
- **决策**:Rust 阅读会话仅缓存/预取**原始页字节**(只解压不解码,LRU 24 页 + 预取前后各 3 页);Flutter 用标准 `imageProvider`(MemoryImage + ResizeImage)解码显示,靠 Flutter image cache 缓存。
- **理由**:`imageProvider` 是 photo_view 最成熟用法,缩放正常;Rust 更轻、可预取更多页;职责更清晰(KISS)。
- **影响**:修复缩放失效;简化 Rust 阅读链路。封面缩略图仍由 Rust 解码(book_cover,需缩放+裁剪)。

## ADR-005 WebDAV 不支持 Range 时的回退:整包下载到本地缓存
- **日期**:2026-07-27
- **状态**:已定
- **背景**:用户 115 网盘 WebDAV 不支持 HTTP Range 请求,原方案直接报错"不支持流式阅读",漫画完全无法打开。
- **决策**:探测到不支持 Range 时,自动 GET 整包下载到 `%APPDATA%/RCH/download/<hash>/` 本地磁盘,后续走本地文件流式逻辑;相同文件再次打开时若缓存未清除可直接复用(秒开)。
- **理由**:物理限制(ZIP 中心目录在文件末尾,没有 Range 无法定位各页偏移),整包下载是唯一可正确解析 ZIP 的回退方案;且缓存文件可跨会话复用,不重复下载。
- **备选**:通知用户手动下载后导入(体验差)。
- **影响**:115 等不支持 Range 的服务器可正常阅读(首次需等待下载);WebDavFile 新增 local_cache 模式避免额外装箱。

## ADR-006 应用状态持久化方案:JSON 文件 + Dart 侧管理
- **日期**:2026-07-27
- **状态**:已定
- **背景**:需持久化书源列表(含 WebDAV 凭据)、阅读记录(最近/最多/进度)、漫画元数据(自定义封面/标签/简介/感想)、应用设置(封面质量/主题)。
- **决策**:Dart 侧用 JSON 文件(`library.json`,通过 path_provider 存应用数据目录)管理全量应用状态;Rust 核心引擎完全无状态(只负责打开书/读页/解码/封面生成)。
- **理由**:JSON 简单可控、M1 无额外依赖;Dart 管应用状态(ChangeNotifier + AnimatedBuilder)与 UI 天然一体;后续可换 SQLite 不影响 Rust 引擎。
- **影响**:Rust 保持无状态;Dart Store(单例 ChangeNotifier)统一管理持久化与 UI 通知。

## ADR-007 封面自定义:选页 + 相对裁剪 + BookMeta 持久化
- **日期**:2026-07-27
- **状态**:已定
- **决策**:每本书存 BookMeta(coverPage + CropRect 相对坐标 + tags + summary + comment),封面生成时先按 crop 裁剪再缩放到目标尺寸;自定义封面与默认封面共用同一 decode_cover 管线(Rust)。
- **影响**:海报墙封面支持"选页+裁剪"个性化;metadata 数据模型为后续标签筛选提供基础。

## ADR-008 WebDAV 连接测试与请求精简
- **日期**:2026-07-27
- **状态**:已定
- **背景**:频繁的 PROPFIND/GIST/Range 请求过多，尤其在不支持 Range 的服务器上（如 115）会触发大量请求。
- **决策**:连接测试只发 1 次 PROPFIND(Depth:0,根路径)；打开远程文件时只测 1 次 Range 支持(字节 0-0)，然后复用结果；若整包下载则直接包装成本地文件 ByteSource；HTTP 错误消息带回服务器原始具体原因(如 401/429 等)。
- **影响**:减少冗余请求、解决 115 报错"Too many unsuccessful sign-in attempts"背后的请求过多问题。

## ADR-009 AI 超分:方案调整 — CLI 目录批量 + 目标 ONNX Runtime 嵌入

- **日期**:2026-07-31（更新）
- **状态**:已定（Phase 1+2 完成，Phase 3 规划中）
- **背景**:Phase 1 CLI 单次调用每页重启进程（~2s 模型加载），性能差。原计划 Rust 侧集成 ort crate (ONNX Runtime) 直接推理，但 ort 2.0-rc 的 Session/Error 类型不满足 Send/Sync，无法在 FRB cdylib + anyhow 环境编译。ort 1.x 稳定版均被 yanked。
- **决策**:
  - Phase 2 采用 CLI 目录批量模式：`super_resolve_batch()` 一次 CLI 调用处理整个输入目录
  - 模型已转为 ONNX 格式（~68KB + ~2.5MB .data），等待 ort crate 稳定后切换到 Rust 直接推理（DirectML GPU 加速 + 批处理）
- **理由**:CLI 批量模式保持当前可工作状态；ONNX 模型已就绪，技术债可控
- **影响**:Rust `ai/` 模块保留 CLI 调用；SPEC/CHANGELOG 更新；DECISION 记录方案调整

## ADR-009 原版:AI 超分:可插拔 Worker 架构（三层设计）
- **日期**:2026-07-27
- **状态**:已定
- **背景**:用户希望漫画阅读器内置端侧 AI 超分，不上传图片、不耗 Token。原有 SPEC 绑定了 NCNN + Vulkan + Real-ESRGAN，但用户希望支持多种模型（Waifu2x、Anime4K、SwinIR 等）并可随时切换。
- **决策**:采用**三层可插拔架构**：
  - **应用层（UI 触发 + Job Queue）**：阅读器右键菜单 → 提交超分任务 → 进度/取消 UI
  - **调度层（Upscaler Trait）**：抽象 `Upscaler` 接口（`fn upscale(input, output) -> Result`），后端可插拔
  - **执行层（常驻 Worker）**：独立 .exe 进程，启动一次后长期驻留，通过**命名管道**（`\\.\pipe\comic-ai`）或共享内存通信，避免每张图 spawn 一次进程（Vulkan 初始化 ~500ms/GPU 初始化几十 MB）
- **实施路线**：
  - **Phase 1**：常驻 Worker（exe）+ 命名管道通信 + 临时文件传图 → 最快落地
  - **Phase 2**：共享内存传图（memmap2），消除磁盘 IO 和 PNG 编解码
  - **Phase 3**：抽象 `Upscaler` trait，支持多种模型切换 + ONNX Runtime 后端
- **理由**：
  - Worker 独立进程：模型 bug 不会拖崩主程序；崩溃可自动重启
  - 常驻进程：避免每张图重复加载模型/Vulkan/GPU（3-4x 性能差距）
  - 可插拔：未来 AI 去噪、AI 去摩尔纹、AI OCR、AI 翻译均可沿用同一 Worker 机制
- **备选方案**：直接 FFI 调用 NCNN C++ 库（编译维护成本高）、ONNX Runtime 直连（灵活但开发复杂度高）
- **影响**：Rust 侧新增 `ai/` 模块（Upscaler trait + Worker 管理）；Dart 侧阅读器右键菜单新增强制超分入口

## ADR-010 格式扩展:统一 Document Trait + 按格式独立实现（不绑 MuPDF）
- **日期**:2026-07-27
- **状态**:已定
- **背景**:当前仅支持 ZIP/CBZ，需扩展 PDF、EPUB、MOBI、CBR 等格式。需慎重选择渲染引擎，平衡功能覆盖与维护成本。
- **决策**:统一 `Document` trait（`page_count + page_bytes + metadata`），每种格式独立实现，不依赖单一重型引擎：
  - **CBZ/ZIP**：已有，`zip` crate → 图片序列
  - **PDF**：`pdfium-render` crate（Google PDFium 的 Rust 绑定），成熟稳定，支持 Windows/Android
  - **EPUB(漫画)**：复用 `zip`（解包）+ `quick-xml`（解析 OPF spine），然后提取图片——漫画 EPUB 本质是 ZIP+HTML+图片，无需排版引擎
  - **CBR**：`unrar` crate 或调用系统 `UnRAR.dll`（注：RAR 许可，UnRAR 源码可用但不可再分发修改版）
  - **CBT**：`tar` crate
  - **CB7**：`sevenz-rust` crate（纯 Rust，无需绑定 7z SDK）
  - **MOBI**：后台自动转换为 EPUB（如集成 Calibre `ebook-convert` CLI 或 `kindleunpack`），再走统一 EPUB 路径——Rust 生态 MOBI 解析不成熟，自研 parser 成本过高
  - **Folder**：直接枚举目录中的图片文件
- **理由**：
  - **不选 MuPDF**：对漫画阅读器来说，真正需要 PDF 渲染的场景有限；CBZ/EPUB(图片型)/Folder 等核心格式用纯 Rust 库即可，维护成本更低
  - **不选 epub-parser 等重型 EPUB 库**：漫画 EPUB 只需解 ZIP + 解析 OPF 取图片顺序，无需排版引擎
  - 格式独立实现，一个出问题不影响其他；新增格式只增不改（符合 SPEC 插件原则）
- **影响**：Rust `document/` 模块按扩展名分发格式（重构已完成）

## ADR-011 图片解码:统一 image crate + 按需扩展
- **日期**:2026-07-27
- **状态**:已定
- **决策**:继续使用 `image` crate（已集成）支持 jpg/png/gif/bmp/webp/tiff；AVIF 按需扩展（`avif-decode` 或 `libavif`）。
- **影响**:所有格式解析器统一输出原始图片字节，由 `image` crate 解码显示。

## ADR-012 四层架构：阅读器永远只操作本地资源
- **日期**：2026-07-27
- **状态**：已定
- **背景**：用户提出将项目做成"现代、高性能、稳定的漫画阅读器"，需要重新确立系统设计的顶层原则。
- **决策**：采用四层架构（UI → Document → Cache → Network/AI）。核心原则：**阅读器永远只操作本地资源，网络只是同步层，AI 只是处理层。**
- **理由**：彻底避免远程随机读取的性能不可靠；兼容不支持 Range 的服务器；缓存是 AI 和所有高级功能的基础。
- **影响**：架构图改为四层；新增 M9 里程碑（缓存基础设施）；新增 `downloader/`、`db/` 模块。

## ADR-013 多级缓存体系 + SQLite 状态管理
- **日期**：2026-07-27
- **状态**：已定
- **背景**：当前缓存简单（L1 内存 LRU + L2 磁盘单页），需升级为完整缓存体系。
- **决策**：五级目录缓存（raw/cover/thumb/ai/temp）+ SQLite（rusqlite）管理索引/ETag/书源能力/进度。缓存按整本漫画存储。现有 `library.json` 保持兼容，逐步迁移到 SQLite。
- **影响**：Rust `cache/` 重构；新增 `db/` 模块；`rusqlite` 依赖。

## ADR-014 WebDAV 保守请求策略
- **日期**：2026-07-27
- **状态**：已定
- **背景**：大型 WebDAV 书库浏览时若每本生成封面会产生雪崩式 HTTP 请求。
- **决策**：浏览目录仅获取元数据；默认不生成封面（占位）；仅用户打开漫画时才下载整本到缓存。并发 ≤2，429 退避。
- **影响**：封面懒加载已实现；整本下载策略待实施。

## ADR-015 MOBI 直接解析（替代后台转 EPUB）
- **日期**：2026-07-27
- **状态**：已定
- **背景**：ADR-010 原定 MOBI 后台转 EPUB。当前无 Calibre CLI，且 mobi crate v0.8.0 可直接解析。
- **决策**：用 `mobi` crate（纯 Rust）直接解析 MOBI/AZW/AZW3，提取 `image_records()` 作为书页。
- **影响**：MOBI 已实现（`document/mobi.rs` 63行）。

## ADR-016 应用数据层：建立 Repository（Single Source of Truth）

- **日期**：2026-07-28
- **状态**：已实施
- **背景**：标签补全 bug 暴露了架构问题——当前状态来源散落在 `BookMeta.tags`、`library.json`、搜索框 Controller、Overlay 之间，没有统一的数据入口。`allTags()` 靠遍历所有 `metas` 收集标签，新增标签若不关联到漫画就无法被补全识别。`_save()` 用空 `catch (_) {}` 静默吞掉写入失败。标签是 `Vec<String>` 到处传，没有独立实体。
- **决策**：在 Flutter 侧新增 `Repository` 层（`app/lib/repository/`），统一管理 Tags。UI 不直接遍历 BookMeta，所有标签读写走 `TagRepository`。`TagRepository` 仍用 JSON 持久化但对外暴露语义化 API（`all()` / `allNames()` / `ensure()` / `link()` / `setBookTags()` / `rename()` / `delete()`），为后续 SQLite 迁移铺路。`LibraryStore` 的标签相关方法（`allTags()` / `tagStats()` / `recordsByTag()` / `renameTag()` / `deleteTag()`）全部委托给 `TagRepository`。
- **理由**：
  - Single Source of Truth：任何地方增删改标签都走 `TagRepository`，不绕过持久化层。
  - 补全 / 搜索 / 统计都依赖同一个 Tag 来源，解决"新增标签不出现"的根因。
  - `TagRepository.search()` 直接返回标签列表（无网络、无 IO），补全零延迟。
- **备选**：直接在现有模型上修修补补（已证明不行，标签补全修了多轮无效）。
- **影响**：
  - 新增 `app/lib/repository/tag_repository.dart` + `repository.dart`（facade）。
  - `models.dart` 新增 `Tag` 和 `BookTag` 类（ADR-017）。
  - `LibraryStore.updateMeta()` 自动同步 TagRepository；`library.json` 新增 `tags` 和 `book_tags` 字段（向后兼容旧格式）。
  - `_save()` 改为返回 `Future<bool>`，失败时不再静默吞掉。
  - `home_page.dart` 的 `_showOverlay()` 补全列表走 `TagRepository.allNames()`。

## ADR-017 标签独立建模：Tag 实体 + BookTag 关联

- **日期**：2026-07-28
- **状态**：已实施
- **背景**：当前标签是 `BookMeta` 的 `List<String> tags` 字段，标签名就是标签的全部信息。这导致：① 重命名标签需要遍历所有书的 tags 列表替换；② 删除标签需要遍历所有书；③ 标签没有独立统计；④ 补全列表依赖遍历所有书的 tags 去重。
- **决策**：引入 `Tag` 实体（id + name + createdAt）和 `BookTag` 关联关系（bookKey + tagId）。底层用 JSON 持久化，`library.json` 新增 `tags` 和 `book_tags` 两个字段，代码层面按独立模型组织。`TagRepository` 内部用 `Map<String, Tag>` 和 `Set<BookTag>` 维护。
- **理由**：
  - 补全列表直接从 Tag 表取（`TagRepository.allNames()`），即使标签没有关联任何漫画也存在。
  - 重命名 / 删除是 O(tag) 而非 O(metas × tags)。
  - 为后续 SQLite 迁移铺路。
- **影响**：
  - `models.dart` 新增 `Tag` 和 `BookTag` 类。
  - `TagRepository` 管理独立标签集合（`_tags` + `_bookTags`）。
  - `library.json` 格式升级（向后兼容：无 `tags` 字段时从 `metas.tags` 回填）。

## ADR-018 架构评价与后续方向（2026-07-28 全量回顾）

- **日期**：2026-07-28
- **状态**：参考性记录（不作施工指令）
- **背景**：标签补全 bug 修了多轮无效，最终追溯到架构层的 Repository 缺失和数据模型缺陷。本轮对项目做了全量架构回顾。

### P0 建议立即处理

1. **Repository 缺失（最大风险）** — 多个 Widget 直接操作 `LibraryStore`，缺少统一数据入口。ADR-016 已新建 `TagRepository`，建议后续扩展到 `BookRepository` / `HistoryRepository` / `SettingsRepository`。
2. **JSON 快到极限** — `library.json` 存放 BookMeta / 阅读记录 / 设置 / 标签 / 书源，每次保存全量重写。建议 `library.json` 只保留 settings，其余迁 SQLite。
3. **BookMeta 责任过重** — 封面、crop、tag、comment、summary、title 全塞在一个类。建议拆 `Book` / `BookExtra` / `BookStat` / `BookTag`。

### P1 未来几个月会碰到

4. **Downloader 不只是下载** — 以后负责下载/取消/恢复/优先级/限速/ETag/断点续传，建议改名 `TransferManager`。
5. **Cache 生命周期未定义** — 缺少 CachePolicy ADR（何时删 raw/cover/thumb/ai）。
6. **Reader 与 Downloader 耦合** — 建议中间加 `BookProvider`。

### P2 容易被忽略

7. **Rust API 越来越大** — 建议按 namespace 组织（reader/source/cache/ai/document/）。
8. **Metadata 无版本号** — 建议 `library.json` 加 version 字段 + migration。
9. **搜索以后一定重写** — 建议 SQLite FTS5 或 Rust 索引。
10. **AI 与 Reader 生命周期** — 建议统一 `TaskManager` 管理 Downloader/AI/OCR/同步。

### P3 暂不处理

11. 插件系统（BookSource Plugin / AI Plugin / OCR Plugin），等 M3。

### 架构评分

| 维度 | 评分 | 说明 |
|------|------|------|
| 整体架构设计 | 9/10 | 四层架构、ByteSource、Document、Cache、AI Worker 方向清晰 |
| 核心引擎设计 | 9.5/10 | Rust 职责划分合理 |
| 数据层设计 | 6.5/10 | library.json / BookMeta / LibraryStore 承载过多，已开始重构 |
| 长期可维护性 | 8/10 | 补上 Repository + 独立 Tag 实体 + 数据迁移机制后可大幅提升 |

## ADR-016 应用数据层：建立 Repository（Single Source of Truth）

- **日期**：2026-07-28
- **状态**：已定
- **背景**：标签补全 bug 暴露了架构问题——当前状态来源散落在 `BookMeta.tags`、`library.json`、搜索框 Controller、Overlay 之间，没有统一的数据入口。`allTags()` 靠遍历所有 `metas` 收集标签，新增标签若不关联到漫画就无法被补全识别。`_save()` 用空 `catch (_) {}` 静默吞掉写入失败。标签是 `Vec<String>` 到处传，没有独立实体。
- **决策**：在 Flutter 侧新增 `Repository` 层，统一管理 Books、Tags、History、Settings。UI 不直接访问 JSON 或遍历 BookMeta，所有读写走 Repository。Repository 内部仍可暂时用 JSON 持久化，但对外暴露语义化 API（`tags()` / `addTag()` / `search()`），为后续 SQLite 迁移铺路。
- **理由**：
  - Single Source of Truth：任何地方增删改标签都走 `Repository`，不绕过持久化层。
  - 补全 / 搜索 / 统计都依赖同一个 Tag 来源，解决"新增标签不出现"的根因。
  - 以后迁移 SQLite 时只需改 Repository 内部实现，UI 层不变。
  - Widget 不再承担业务逻辑（搜索、过滤、补全），只负责渲染。
- **备选**：直接在现有模型上修修补补（已证明不行，标签补全修了多轮无效）。
- **影响**：
  - 新增 `app/lib/repository/` 模块（LibraryRepository + TagRepository）。
  - `home_page.dart`、`source_browser.dart` 等 UI 层的搜索/补全逻辑下沉到 Repository。
  - `LibraryStore` 逐步收敛为 Repository 的 JSON 后端。
  - 后续 ADR-017 标签独立建模可基于此层实现。

## ADR-017 标签独立建模：Tag 实体 + BookTag 关联

- **日期**：2026-07-28
- **状态**：已定
- **背景**：当前标签是 `BookMeta` 的 `List<String> tags` 字段，标签名就是标签的全部信息。这导致：① 重命名标签需要遍历所有书的 tags 列表替换；② 删除标签需要遍历所有书；③ 标签没有独立统计（使用频率、创建时间等）；④ 补全列表依赖遍历所有书的 tags 去重。标签本质是独立实体，不应寄生在 `BookMeta` 中。
- **决策**：引入 `Tag` 实体（id + name）和 `BookTag` 关联关系（book_key + tag_id）。底层可以暂时仍用 JSON 序列化（`library.json` 中新增 `tags` 和 `book_tags` 两个字段），但代码层面按独立模型组织：
  ```
  Tag { id: String, name: String }
  BookTag { book_key: String, tag_id: String }
  ```
  - `TagRepository.tags()` → 所有标签（独立存储，不依赖 BookMeta）
  - `TagRepository.rename(id, newName)` → 一次修改，所有关联书的显示自动更新
  - `TagRepository.booksForTag(id)` → 反查漫画
- **理由**：
  - 补全列表直接从 Tag 表取，即使标签没有关联任何漫画也存在（解决"新标签不出现"）。
  - 重命名 / 删除是 O(1)，不需要遍历所有书的 tags。
  - 为后续 SQLite 迁移（真正的外键关联）建模铺路。
  - 符合单一职责：BookMeta 只管封面和元数据，Tag 只管标签。
- **影响**：
  - `models.dart` 新增 `Tag` 和 `BookTag` 类。
  - `LibraryStore` / `TagRepository` 管理独立标签集合。
  - `library.json` 格式升级（向后兼容：无 `tags` 字段时从 `metas` 中回填）。
  - `allTags()` 改为 `TagRepository.tags()` 直接返回。

## ADR-016 应用数据采用 Repository + Single Source of Truth

- **日期**：2026-07-28
- **状态**：已定
- **背景**：标签补全问题暴露出状态来源分散（BookMeta、LibraryStore、Widget、搜索框等）。
- **决策**：所有业务状态只能通过 Repository 修改；Widget 禁止直接修改模型；Repository 是唯一持久化入口。
- **影响**：后续搜索、标签、历史记录、设置全部改为 Repository API。

## ADR-017 标签独立建模

- **日期**：2026-07-28
- **状态**：已定
- **决策**：引入 Tag 实体与 BookTag 关联关系；补全、搜索、统计统一从 TagRepository 获取。
- **影响**：废弃 allTags() 遍历 BookMeta 的实现。

## ADR-020 Library Index 同步架构（同步功能重构）

- **日期**：2026-08-09
- **状态**：已定（待实施）
- **背景**：当前同步只是"数据库行增量搬运 + 幽灵元数据全局搜索"：包内没有书源下的漫画目录，目标设备浏览云端书源必须连服务器；"书源凭据包"（rchbundle）与同步包双协议并存；fingerprint 从未被写入导致跨设备书源匹配与凭据写回全部失效；settings 分块明文携带 `sync_webdav_password`。RCH 已从"阅读器"进入"本地优先 + 多设备知识库"阶段（第 32 轮 SQLite 状态中心、第 36 轮 Repository 分层、第 42 轮导出能力），同步将成为核心资产层而非附属功能。
- **决策**：
  1. **同步粒度**：书源下全部目录作为 `library_index`（可缓存索引资产）进入同步包；不是每次 push 全量扫描，而是"首次用户主动刷新生成全量 + 之后按快照增量"。
  2. **包格式升级 rchpkg v2**：`manifest.json + sources/ + library_index/(entries.jsonl + snapshots.json) + metadata/ + records/ + tags/ + settings/ + credentials.enc(可选)`；v1 包保持可读。
  3. **三级同步模型**：L1 源级信息（名称/类型/地址/设备/备注/能力）、L2 目录索引（文件夹/名称/路径/大小/修改时间/封面索引）、L3 内容状态（进度/标签/收藏/评分/备注，即现有 records/metas/tags）。
  4. **fingerprint 落地**：`fingerprint = hash(type + normalized_endpoint + root_identifier)`，不含账号；凭据独立按 fingerprint 绑定，换账号不产生新书源。
  5. **统一流程**：删除 rchbundle 与 WebDAV 自动同步模式；全局设置只有"导出同步包 / 拉取同步包"两个操作；settings 分块改为白名单，剔除 `sync_*` 明文密钥。
  6. **UI**：设备 → 书源 → 漫画三级树，默认折叠；**本机已配置的相同云端书源（fingerprint 命中）合并为普通书源不折叠**；离线浏览默认支持；阅读仅当本机具备本地资源或凭据时允许。
- **理由**：目标是"任何设备打开 RCH 都能看到其他设备有什么书源、有什么漫画、哪些书读过"；catalog 是知识库核心资产，不能只同步已读/已编辑路径；云端大目录每次全量枚举会触发限流/WAF（115 已踩过），故采用"主动刷新 + 快照增量"。
- **备选**：A) 仅同步已读/已编辑路径（省流量但书单不完整，体验割裂）；B) 每次 push 全量枚举（实现简单但大书源请求量不可控）。
- **影响**：新增 `library_index` / `source_snapshot` 表；fingerprint 计算与存量回填；rchpkg schema v2 读写与 v1 兼容；SourceBrowser 离线目录模式；设备名设置；settings 白名单；配套任务 `.trellis/tasks/08-09-sync-rework`。

> 2026-08-09 修订：同步形态收敛为手动包交换（导出/拉取），**同步包不携带任何书源凭据**（目标设备不连服务器，凭据同步无意义）；云端同源按 fingerprint 合并为普通书源。详见 ADR-022。

## ADR-021 Library Index 与 Metadata 分层分离

- **日期**：2026-08-09
- **状态**：已定
- **背景**：同步重构引入 library_index 后，容易把"路径/大小/mtime"（物理发现层）与"作者/标签/评分/备注"（用户认知层）混进同一张表；未来智能刮削（M8）依赖这条分层：library_index → LLM 识别 → book_meta → tags。
- **决策**：`library_index` 只承担**物理资产发现层**职责（source / path / parent / size / modified_at / cover_ref），不写入作者、标签、评分、备注等认知信息；认知层继续由 `book_metas`（用户编辑元数据）与 `read_records`（阅读状态）承载。二者通过稳定身份（`source_fingerprint + path`）关联，互不生成、互不覆盖。
- **理由**：发现层可离线重建（重新扫描），认知层是用户资产不可重建；混层会导致扫描覆盖用户编辑或元数据污染目录索引。
- **影响**：所有 library_index 读写代码禁止触碰 book_metas 字段；新增 UI/同步逻辑按分层校验；为 M8 智能刮削预留干净边界。

## ADR-022 同步收敛：手动包交换 + 无凭据 + 云端同源合并

- **日期**：2026-08-09
- **状态**：已定（修订 ADR-020 的凭据与传输决策）
- **背景**：用户确认跨设备浏览只显示包内信息、不连服务器；阅读只在本机有资源/凭据时进行。因此"导出带凭证"没有实际价值——目标设备既不用 A 的凭据读 A 的服务器，也不需要 B 的凭据。同时两台设备可能配置相同的云端书源（同一 WebDAV/夸克/115），需要合并而不是重复折叠。
- **决策**：
  1. **同步包永不携带书源凭据**（password/refresh_token/client_secret/cookie 不进入包，也无口令加密块）；每台设备自持资源与凭据。
  2. **同步入口收敛**：全局设置只有"导出同步包 / 拉取同步包"两个操作；删除书源列表 rchbundle 按钮、SyncPanel 的 WebDAV 模式 / push / pull / 测试连接 / 清理归档 / 同步口令；删除 credentials.enc 与 SourceBundle 相关代码路径。
  3. **云端同源合并**：拉取时包内云端书源 fingerprint 命中本机已有书源 → 合并为一个普通书源（id 重映射 + key 前缀重写，LWW），显示不折叠、阅读用本机凭据；未命中的书源（含 local/smb 幽灵）进入来源设备折叠组（仅索引）。
  4. fingerprint 仅用于书源身份匹配/合并，不再用于凭据写回。
- **理由**：同步的本质是"跨设备信息编辑"（目录 + 元数据 + 进度），不是"跨设备授权"；凭据留在本机既安全又简单。同源合并避免同一服务器源在两台设备上被折叠成两份。
- **影响**：删除凭据相关 Rust/Dart 路径；SyncManager 大幅精简；UI 分区（普通书源区 + 设备折叠区）；阶段 4/5 实施范围按此调整。

> 2026-08-09 修订：传输层保留 WebDAV 同步文件夹（latest.rchpkg 单文件全量快照），本地文件作为手动备份选项；新增自动流程。详见 ADR-023。

## ADR-023 传输层：WebDAV 同步文件夹 + 自动流程 + 崩溃恢复

- **日期**：2026-08-09
- **状态**：已定
- **背景**：同步入口收敛为"导出/拉取"两个操作后，跨设备传输仍需要载体。用户确定用 **WebDAV 同步文件夹**（`RCH/sync/latest.rchpkg`，覆盖式全量快照），并希望放心启用自动流程：启动自动拉取、退出自动导出、闪退/库损坏时按最新包还原。
- **决策**：
  1. 传输 = WebDAV 同步文件夹，单文件 `latest.rchpkg`；导出 = 全量快照 PUT（先 `latest.rchpkg.tmp` 再 MOVE，防半包）；拉取 = GET + merge（LWW + 墓碑）。保留"另存/打开本地文件"作为手动备份选项。**不直接复制 SQLite 文件**（运行中复制不具崩溃一致性、文件级覆盖无合并、全量上传带宽不可行）。
  2. 自动流程（近实时）：本地变更防抖（~3s）→ pull-merge-push（先 GET 远端 → 本地合并 → 再 PUT）；启动自动拉取合并（失败不阻塞）；前台/定期轮询远端 ETag（默认 60s）变化才拉取；退出兜底同步。把"最后写者覆盖丢数据"窗口缩到两端同时 PUT 的极端场景（已知限制）。
  3. 崩溃恢复：服务器始终持有上次成功导出的全量包；崩溃设备下次启动拉取合并即可还原；本地库打开/完整性异常时用远端包 force 恢复。
  4. WebDAV 同步账号凭据属于本机传输配置（app_settings `sync_*`），不进同步包（settings 白名单已剔除）。
- **理由**：全量快照 + LWW 合并 + pull-merge-push 是简单可靠的模型；WebDAV 传输 API（upload/download/make_dir）阶段 2 已存在，直接复用。
- **影响**：SyncPanel 保留 WebDAV 配置与测试连接，移除旧增量 push/pull、清理归档、同步口令；新增生命周期钩子（启动 pull / 退出 push）与原子写入；Android 退出钩子受限时采用"尽力而为 + 手动兜底"。

> 2026-08-09 修订：最终采用 **Sync State + Three-Way Merge**（ADR-024），rchpkg 降级为备份格式（ADR-025）；ADR-023 的"latest.rchpkg 包文件同步"不再作为日常同步路径。

## ADR-024 同步协议：WebDAV Sync State + Three-Way Merge

- **日期**：2026-08-09
- **状态**：已定（架构收敛）
- **背景**：用户最终确定 RCH 同步体验应接近 Obsidian + Remotely Save（自动、无感、近实时）；rchpkg 包同步与整库复制均不满足"可靠 + 可诊断 + 可合并"。最终采用三方比较/语义合并。
- **决策**：
  1. 日常同步 = WebDAV 上的**状态文件**（`manifest.json` 为提交点 + `state/<entity>-<rev>.*` 版本化文件 + `devices/<id>.json`）；**不复制 SQLite，不以 rchpkg 为同步协议**。
  2. 本机维护 **Sync Base**（SQLite `sync_base`/`sync_meta`）：上次成功时远端状态；只有同步成功才推进；下载/合并/上传失败不推进。
  3. **三方合并**（Base + Local + Remote → Sync Plan → Merged）：metas 字段级、tags/book_tags 并集+墓碑、records/sources/settings LWW+墓碑、library_index 单端胜+墓碑；不引入 CRDT/Vector Clock/Event Sourcing。
  4. 并发：Push 采用 **CAS 循环**（manifest revision 冲突 → 重拉重并重写），绝不拿旧状态覆盖远端；极端同写窗口最后写者胜（已知限制）。
  5. 身份：source_fingerprint + 同步层稳定 book_id（fingerprint+规范化路径）；本地 SQLite key 不变，映射层转换。
  6. 凭据默认不上传；可选开启 → 独立加密文件（AES-GCM，按 fingerprint 绑定）。
- **理由**：状态文件可备份/检查/恢复；三方合并避免"谁大谁赢"的整库覆盖；CAS + manifest 提交点把并发窗口缩到最小；Sync Plan 提供可诊断性。
- **影响**：新增 `app/rust/src/sync/`（identity/base/state/merge/webdav）；Dart `SyncEngine`（防抖/生命周期/轮询）；删除旧 rchpkg 同步路径；UI 设备分组 + 离线索引浏览。

## ADR-025 备份与同步分离：rchpkg 降级为备份格式

- **日期**：2026-08-09
- **状态**：已定
- **背景**：rchpkg 曾被设计为同步协议（schema v2 + library_index 入包）。架构收敛后，日常同步走 Sync State + 三方合并（ADR-024），rchpkg 不再承担实时同步职责。
- **决策**：rchpkg 保留为 **RCH Backup Package**——完整备份 / 迁移 / 离线恢复 / 灾难恢复 / 版本升级；支持 JSONL、可选加密凭据、v1 兼容；**日常自动同步不依赖 rchpkg**。删除 rchbundle 与 rchpkg 同步路径（push/pull/归档/cursor 增量）。
- **理由**：备份需要"可整体还原、可携带凭据"的格式；同步需要"可合并、可诊断"的状态协议；两者职责不同，混用导致双向妥协。
- **影响**：rchpkg 导出统一为全量快照；备份/恢复 UI 与同步 UI 分离；`cursor_export` 不再用于日常同步。

## ADR-026 同步参与者身份（Sync Actor Identity）

- **日期**：2026-08-09
- **状态**：已定（Phase 4.6）
- **背景**：同步协议已有 Git 式骨架（library_id/revision/manifest/sync_base/SyncPlan），缺少"谁产生了这个 revision"；Phase 5 压测与 Phase 6 设备分组都依赖参与者身份，且 manifest 格式应一次定稿。
- **决策**：
  1. manifest schema v3 增加 `writer: {device_id, device_name}`——**revision 元数据，不参与业务合并**（无 LWW、不是业务实体）。
  2. 本地新增 `sync_devices` 注册表（不复用旧 `devices` 表）：device_id / device_name / platform / created_at / last_seen_at / last_revision。
  3. 远端 `devices/<device_id>.json`：{device_id, name, platform, last_seen_at}，每台设备只写自己的文件。
  4. device_id = 首次运行生成 **UUID v4**，永久稳定；改名只改 device_name（禁主机名/MAC）。
  5. SyncPlanItem 增加 local_revision / remote_revision / winner / reason，支持"哪台设备改了什么"的可诊断展示。
- **理由**：writer 是事件信息而非状态；设备身份必须跨重启、跨改名稳定，否则历史记录断裂。
- **影响**：`db::get_or_create_device_id_on` 改 UUID；新增 `sync/actor.rs`、`sync/history.rs`；`sync_now` 带 platform；设置页后续展示参与者与同步历史。

## ADR-027 同步可观测性：sync_history

- **日期**：2026-08-09
- **状态**：已定（P1-9）
- **背景**：同步系统复杂度已超过普通导出/导入，用户需要能回答"为什么我的漫画没同步"。
- **决策**：新增 `sync_history` 表，每次同步记一条：start/end、revision_before/after、pull/push/merge/conflict 计数、error、实体变更摘要 JSON；失败也记录；FRB 暴露最近 N 条。
- **影响**：设置页"同步历史"展示；排查问题不再靠猜。
