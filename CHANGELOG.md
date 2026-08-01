# Changelog

All notable changes to RCH will be documented in this file.

格式基于 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，版本号遵循 [SemVer](https://semver.org/lang/zh-CN/)。

---

## [Unreleased]

### Added

- **M2 AI 超分 Phase 1**：端侧 AI 超分引擎，右键单页超分 + 漫画详情页整本超分，自动打"AI超分"元数据标签
- **M2 AI 超分 Phase 2**：CLI 目录批量模式 — `super_resolve_batch()` 一次调用处理整本漫画

---

## [0.2.1] — 2026-07-28

### Added

- **封面磁盘缓存**：`book_cover` / `webdav_cover` 解码后写入 `cover/` 目录，再次访问秒出
- **封面加载并发控制**：`_CoverLoadQueue` 限制最多 4 个 FFI 调用同时执行，避免几百个封面同时竞争线程池

### Changed

- **Repository 层扩展**：`BookRepository` + `RecordRepository` 接管数据 CRUD，`LibraryStore` 精简为 facade + ChangeNotifier + 跨模块协调层
- **ComicCover StatelessWidget → StatefulWidget**：Future 在 initState 创建一次，父 rebuild 不再触发重复解码

### Fixed

- 修复封面缩略图磁盘缓存始终 0MB 的问题（`cover/` 目录从未写入）
- 修复海报墙大量转圈的问题（StatelessWidget + FutureBuilder 反模式 + 无并发限制）

---

## [0.1.0] — 2026-07-28

### Added

- **8 种格式引擎**：ZIP/CBZ、EPUB、Folder、CB7、CBT、PDF、CBR/RAR、MOBI/AZW/AZW3
- **流式阅读**：ZIP/CBZ/EPUB 只读文件尾部中心目录，按需解压单页，大文件即点即读
- **三级缓存**：L1 内存 LRU 缓存 + L2 磁盘缓存 + 后台并行预取
- **WebDAV 书源**：连接远程服务器，PROPFIND 列目录，Range 流式阅读或整本下载缓存
- **本地书源**：浏览本地目录，海报墙展示，漫画文件夹智能识别
- **漫画文件夹元数据**：自动读取 ComicInfo.xml / metadata.json（标题/作者/系列/类型）
- **封面系统**：封面自动检测（cover.jpg 优先），自定义封面页+裁剪区域，质量可调
- **阅读器**：三种阅读模式（日漫/美漫/条漫），`+/-/0` 键缩放，双页拼接
- **标签系统**：元数据标签（作者/类别/系列）+ 自由标签，补全联想，批量管理
- **搜索系统**：统一搜索栏，`#` 标签补全，跨书源搜索
- **主界面**：左侧导航（最近阅读/最多阅读/标签管理/书源列表/设置），海报墙书架
- **漫画详情页**：元数据/标签/简介/感想编辑，封面自定义
- **缓存管理面板**：五级缓存（page/raw/cover/download/ai）分类独立管理
- **下载进度**：WebDAV 下载百分比进度条，每 300ms 轮询 Rust 端进度
- **自定义按键绑定**：5 个阅读器动作可配置
