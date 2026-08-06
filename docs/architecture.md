# RCH 架构

> 版本：v0.1.0 | 最后更新：2026-07-28

本文档记录 RCH 的当前系统架构。目标架构见 [SPEC.md](../SPEC.md)。

---

## 总体架构

```
┌──────────────────────────────────────────────────┐
│                  Flutter UI 层                    │
│  main.dart → home_page / reader_page /           │
│  source_browser / book_detail / settings         │
├──────────────────────────────────────────────────┤
│             flutter_rust_bridge v2               │
│         (代码生成: Dart ↔ Rust 类型映射)           │
├──────────────────────────────────────────────────┤
│                Rust 核心引擎 (cdylib)              │
│                                                  │
│  ┌─────────────┐  ┌──────────────────────┐       │
│  │  api/        │  │  document/            │       │
│  │  FRB 暴露的   │  │  Document trait       │       │
│  │  公开接口    │  │  ├─ ZipDocument        │       │
│  │             │  │  ├─ EpubDocument       │       │
│  └──────┬──────┘  │  ├─ FolderDocument     │       │
│         │         │  ├─ SevenZDocument     │       │
│  ┌──────┴──────┐  │  ├─ TarDocument       │       │
│  │  source/     │  │  ├─ PdfDocument       │       │
│  │  BookSource  │  │  ├─ RarDocument       │       │
│  │  ├─ Local    │  │  └─ MobiDocument      │       │
│  │  └─ WebDAV   │  └──────────────────────┘       │
│  └──────┬──────┘                                   │
│         │                                          │
│  ┌──────┴──────────────────────────────┐          │
│  │  cache/ + downloader/                │          │
│  │  五级缓存 (page/raw/cover/ai/temp)    │          │
│  │  + 统一下载调度器                      │          │
│  └──────────────────────────────────────┘          │
│                                                  │
│  ┌─────────────┐  ┌──────────────────────┐       │
│  │  util/       │  │  db/ (SQLite)        │       │
│  │  工具函数    │  │  缓存索引/书源能力     │       │
│  └─────────────┘  └──────────────────────┘       │
└──────────────────────────────────────────────────┘
```

## 数据流

```
用户操作 → Flutter UI
              ↓
         FRB API 调用
              ↓
         Rust api/ 层 (参数校验、错误转换)
              ↓
    ┌─────────┼─────────┐
    ↓         ↓          ↓
  source/  document/   cache/
  (获取    (解析      (查询/
  字节源)  格式)      写入缓存)
    ↓         ↓          ↓
    └─────────┼─────────┘
              ↓
         返回给 Flutter (Uint8List / String / JSON)
              ↓
         UI 渲染
```

### 核心原则

1. **阅读器永远只操作本地资源**：网络只是同步层，AI 只是处理层
2. **Document 抽象隔离格式差异**：UI 不关心底层是 ZIP 还是 PDF
3. **ByteSource 抽象隔离来源差异**：本地文件和 WebDAV 统一为 Range 可读字节流

## 模块职责

| 模块 | 文件 | 职责 |
|---|---|---|
| `api/` | `api/mod.rs`, `api/simple.rs` | FRB 公开接口，参数校验，错误转换 |
| `source/` | `source/mod.rs`, `source/local.rs`, `source/webdav.rs` | 书源抽象，提供 `ByteSource` |
| `document/` | `document/mod.rs` + 各格式文件 | `Document` trait 统一格式接口 |
| `cache/` | `cache/mod.rs` | 多级缓存读写，目录管理 |
| `downloader/` | `downloader/mod.rs` | 统一下载队列/去重/重试 |
| `db/` | `db/mod.rs` | SQLite 缓存索引、书源能力记录 |
| `util/` | `util/mod.rs` | 自然排序、文件工具等 |

## 格式分发

```rust
// document/mod.rs 中的分发逻辑
match extension {
    "zip" | "cbz"          => ZipDocument::open(source),
    "epub"                 => EpubDocument::open(source),
    "cb7" | "7z"           => SevenZDocument::open(source),
    "cbt" | "tar"          => TarDocument::open(source),
    "pdf"                  => PdfDocument::open(source),
    "cbr" | "rar"          => RarDocument::open(source),
    "mobi" | "azw" | "azw3" => MobiDocument::open(source),
    _ if is_comic_folder() => FolderDocument::open(source),
}
```

## 缓存体系

```
%APPDATA%/RCH/
├── cache/
│   ├── raw/       # 整本漫画原始文件 (WebDAV 下载后)
│   ├── cover/     # 封面缓存 (按质量/裁剪区域分)
│   ├── ai/        # AI 超分结果 (按模型/倍率分)
│   └── page/      # L2 页面缓存 (读过的页写盘)
├── library.json   # 书源/阅读记录/元数据/设置
└── database.db    # SQLite: 漫画索引/缓存 Hash/ETag

注：temp/ 位于系统临时目录（%TEMP%/RCH/temp），不占用上述数据目录。
```

## 阅读器数据流

```
用户打开漫画
    ↓
api::open_book(path, source_id)
    ↓
source::get_source(id) → BookSource
    ↓
BookSource::open(path) → ByteSource
    ↓
document::open(source) → Box<dyn Document>
    ↓
Document::page_count() → N
Document::read_page(i) → Uint8List  ───→ Flutter Image widget
                              ↓
                         cache::put(page_key, bytes)  (L2 磁盘缓存)
```

## 翻页缓存策略

```
当前页 P
    ↓
L1 内存缓存: P-2, P-1, P, P+1, P+2  (LRU, 5页)
    ↓ 未命中
L2 磁盘缓存: cache/<hash>/page_<i>.bin
    ↓ 未命中
从 ByteSource 读取 → 解码 → 返回 → 写入 L1 + L2
    ↓ 后台
预取 P+3, P+4, P+5 ... (并行, 低优先级)
```

## 技术栈

| 层 | 技术 |
|---|---|
| UI | Flutter 3.44 (Dart) |
| 引擎 | Rust 1.80 (cdylib) |
| 桥接 | flutter_rust_bridge v2 |
| 数据库 | SQLite (rusqlite) |
| 图片解码 | image crate (Rust) |
| 格式解析 | zip, epub, sevenz-rust, tar, pdfium-render, unrar, mobi |
| 持久化 | JSON (library.json) + SQLite (database.db) |
| AI 引擎 | librealesrgan-ncnn-vulkan (Worker 子进程) |
