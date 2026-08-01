# Changelog

All notable changes to RCH will be documented in this file.

格式基于 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，版本号遵循 [SemVer](https://semver.org/lang/zh-CN/)。

---

## [0.3.0] — 2026-08-02

### Added

- **M2 AI 超分**：右键单页 2x 超分；整本超分改为**后台任务队列**（可多本排队、持久化续跑、全局悬浮进度窗、完成时阅读该书弹"加载超分版本"提示、任务取消）
- **阅读器版本切换**：阅读中可随时切换原版 / 超分版本，页码不丢
- **缓存目录切换重构**：文件夹选择 + 整个根目录（数据库+缓存）自动迁移，数据留在用户所选目录，启动恢复、中断续迁
- **标签持久化修复**：阅读记录轻量落盘、防抖全量保存、退出前强制 flush、一次性 JSON 对账；旧 hash 标签 ID 自动归一化
- WebDAV 会话缓存独立到 store 层

### Changed

- 整本超分由前台逐页改为后台队列（优先逐页进度，Debug 构建下进度逐页可见）
- AI 进程调用加固：60s 超时 + kill、Windows 无黑窗、临时文件唯一化、管道防阻塞
- JPEG 输出质量 75 → 90；缓存 key 使用模型名（换模型不串缓存）
- 移除打包内未使用的 ONNX 残留文件（约 2.5MB）

### Fixed

- 重启后 `AI超分`/`已读`/普通标签丢失
- "数据保存失败"（`UNIQUE constraint failed: tags.name`）
- AI 超分黑窗闪现、进程挂死无超时、并发踩临时文件
- "取消 AI 超分"误清全部书缓存（改为只清当前书）
- 恢复默认缓存目录被误拒（默认根与支持目录嵌套问题）
- 后台超分进度跳变/卡 0（逐页更新 + 悬浮窗定时刷新 + 失败红字提示）

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
