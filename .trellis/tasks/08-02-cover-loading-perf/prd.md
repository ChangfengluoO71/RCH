# 海报墙封面加载性能优化

## Goal

缩短海报墙封面的加载时间（首屏与滚动），减少转圈与卡顿。

## Background（已确认的代码事实）

- 封面链路：[comic_cover.dart](C:/Users/cfl/Desktop/RCH/app/lib/ui/comic_cover.dart:161) → `bookCover`/`webdavCover`（Rust）→ 先查 `cover/` 磁盘缓存 → 未命中则 `open_document`+解码+写缓存（本地约 30-80ms/本，`_CoverLoadQueue` 限 4 并发）。
- WebDAV 封面每次先 `webdavHasRawCache` 检查 + `webdavCover` 网络调用（[comic_cover.dart](C:/Users/cfl/Desktop/RCH/app/lib/ui/comic_cover.dart:157)）。
- 封面缓存 key 含 `coverQuality`（[comic_cover.dart](C:/Users/cfl/Desktop/RCH/app/lib/ui/comic_cover.dart:104)）→ 不同质量设置各自缓存；首次全墙加载需逐本解码。

## 待验证的性能瓶颈（开工时先测量）

- 首屏一次性加载过多封面，4 并发排队耗时；
- WebDAV 墙：每封面网络往返（hasRawCache + cover 下载）无批量化；
- 质量切换导致缓存全 miss；
- 本地封面每次 `open_document` 重开文件（无按书会话复用）。

## Requirements

- **R1** 测量定位主瓶颈（首屏时间、单封面耗时、网络请求次数）。
- **R2** 针对性优化：如 WebDAV 封面批量预取、封面会话复用、预加载/懒加载策略、首屏优先排序。

## Acceptance Criteria

- [ ] 首屏封面加载时间明显下降（有前后对比数据）
- [ ] 滚动/翻页不因加载卡顿
- [ ] 磁盘缓存命中后秒出（现有能力不回退）
- [ ] `flutter analyze` 0 issues

## Out of Scope

- 阅读器页内加载（非封面）。
