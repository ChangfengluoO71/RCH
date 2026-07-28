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

## ADR-009 AI 超分:可插拔 Worker 架构（三层设计）
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
