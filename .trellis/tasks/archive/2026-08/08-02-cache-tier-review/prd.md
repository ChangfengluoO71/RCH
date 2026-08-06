# 缓存层分析 — 缩略图(thumb)与旧下载目录(download)的作用与去留

## Goal

分析"缩略图"（thumb/）与"旧下载目录"（download/）两个缓存层是否仍有作用，决定删除/保留/迁移，并清理缓存管理面板中的冗余入口。

## Background（已确认的代码事实）

1. **thumb/ 是死缓存层**：`CacheDir::Thumb` 有定义、目录会被 `ensure_all_cache_dirs` 创建（[cache.rs](C:/Users/cfl/Desktop/RCH/app/rust/src/cache.rs:463)）、有大小统计与清理 API（[api/cache.rs](C:/Users/cfl/Desktop/RCH/app/rust/src/api/cache.rs:69)），但**全仓没有任何写入 thumb/ 的代码**（[decode.rs](C:/Users/cfl/Desktop/RCH/app/rust/src/decode.rs:18) 的 `thumbnail()` 是内存缩放，不落盘）→ 永远 0MB，纯占位。
2. **download/ 是 WebDAV 整本下载的回退目录**：主路径是 `download_to_raw_cache`（写 raw/，[api/source.rs](C:/Users/cfl/Desktop/RCH/app/rust/src/api/source.rs:119)）；`download_full` 写 `cache_root()/download`（[webdav.rs](C:/Users/cfl/Desktop/RCH/app/rust/src/source/webdav.rs:315)），在 raw 缓存下载失败时作为回退（[api/source.rs](C:/Users/cfl/Desktop/RCH/app/rust/src/api/source.rs:133)）。缓存面板标注为"旧版下载回退目录"。

## Requirements

- **R1** thumb/：确认无写入后，删除该层（目录创建、大小统计、清理 API、缓存面板行）。
- **R2** download/：确认回退路径是否仍可达；旧文件处理策略（保留供回退读取 vs 清理/迁移 raw）。
- **R3** 缓存面板文案与实际作用对齐。

## Acceptance Criteria

- [x] thumb 层从代码与 UI 中移除，`cache_sizes` 不再包含 thumb
- [x] download 策略确定并落地（删除目录、回退并入 raw/），WebDAV 回退下载不失效
- [x] `cargo test`、`flutter analyze` 通过

## Open Questions（开工前确认）

- **O1** thumb 直接删除，还是保留目录待未来缩略图功能使用？推荐删除（无写入代码，保留只会误导）。
- **O2** download 旧文件：保留（回退仍可读）还是清理（raw 已是主路径）？推荐保留但清理面板文案改为"整本下载回退"，并提供一键清理。

## 决策记录（2026-08-06 实施）

- **O1 → 删除 thumb**：无任何写入代码的占位层，从枚举、大小统计、清理 API、目录创建与 UI 中整体移除。
- **O2 → 删除 download/**：WebDAV 无 Range 服务器回退（`download_full`）改为写入 raw/ 缓存，与主路径共用同一 hash 缓存；"清空全部缓存"会一并清理旧版遗留的 download/ 与旧页面缓存哈希目录。
- "磁盘总占用"改为各缓存分类之和（不含数据库/日志/支持目录）。
