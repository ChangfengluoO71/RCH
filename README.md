# RCH — 本地优先的 Windows 漫画阅读器

打开大压缩包**不用解压、边下边读**；自带 **AI 2x 超分**、**标签管理** 与 **WebDAV 远程书架**，数据全部留在本机。

[![Flutter](https://img.shields.io/badge/Flutter-3.44-blue?logo=flutter)](https://flutter.dev)
[![Rust](https://img.shields.io/badge/Rust-1.97-orange?logo=rust)](https://www.rust-lang.org)
[![Platform](https://img.shields.io/badge/Platform-Windows-brightgreen)]()
[![License](https://img.shields.io/badge/License-MIT-green)](LICENSE)

---

## ✨ 核心亮点

- ⚡ **超大压缩包秒开** — 流式读取，只解压当前页，不等待整包
- 📚 **8+ 种格式通吃** — ZIP/CBZ、EPUB、CB7、CBT、PDF、CBR/RAR、MOBI/AZW/AZW3、文件夹
- 🚀 **翻页流畅不等待** — 三级缓存（内存 / 磁盘 / 预取），读过的页面秒开
- 🤖 **AI 2x 超分** — 右键单页放大，或整本加入后台队列，完成后一键切换超分版
- 🏷️ **标签管理系统** — 元数据标签分层折叠、已读标记、批量打标
- 🎨 **阅读器** — 日漫 / 美漫 / 条漫三种模式，双页拼接、缩放、每页独立旋转
- 📦 **漫画整理** — 本地漫画自动转 CBZ，zip/cbz/文件夹视为同一本，进度标签不丢
- ☁️ **WebDAV 远程书架** — 连接 NAS / 网盘，流式阅读，首次下载后秒开
- 🔒 **本地优先** — 阅读记录、标签、设置全部存本机（SQLite + JSON 备份），无账号、无云依赖

## 🚀 快速开始

**普通用户**：前往 [GitHub Releases](https://github.com/ChangfengluoO71/RCH/releases) 下载最新版安装包（Windows）。

**开发者**：

```bash
git clone https://github.com/ChangfengluoO71/RCH.git
cd RCH/app
flutter run -d windows
```

构建环境（Rust / Flutter / VS 2022 BuildTools / flutter_rust_bridge_codegen）见 [SETUP.md](SETUP.md)。

## 📖 功能介绍

### 打开即读，格式通吃

- **流式解析**：ZIP/CBZ/EPUB 只读取文件尾部中央目录，按需解压单页，几百 MB 的压缩包也是即点即读
- **格式支持**：ZIP / CBZ、EPUB、文件夹（Folder）、CB7 / 7Z、CBT / TAR、PDF、CBR / RAR、MOBI / AZW / AZW3
- **文件夹漫画**：包含图片的文件夹自动识别为漫画，自动读取 `ComicInfo.xml` / `metadata.json` 元数据（标题 / 作者 / 系列 / 类别）
- **本地书架**：浏览本地目录，漫画文件夹在海报墙直接显示封面卡片，点击进入详情页

### 流畅的阅读体验

- 三种阅读模式：**日漫**（右翻双页）、**美漫**（左翻）、**条漫**（竖向滚动）
- **双页拼接** + "首页单独显示"，双页模式下可缩放拖动
- **缩放**：`+` / `-` / `0` 键，或触控板 / 鼠标滚轮
- **页面旋转**：右键"界面旋转"或对单页旋转，每页旋转结果自动记住，下次打开不变
- **进度记忆**：重开直接跳到上次阅读位置；最近阅读 / 最多阅读自动记录
- **智能预取**：后台预读前后 3 页，翻页零等待；磁盘缓存让重复阅读秒开

### AI 2x 超分

- **单页超分**：阅读中右键当前页 → “AI 超分 (2x)”，本页立即放大（Real-ESRGAN 模型，端侧推理）
- **整本超分**：详情页一键加入后台队列，可多本排队、随时取消
- **任务列表**：悬浮窗展开可见全部任务，**拖拽调整排队顺序**，进行中任务置顶；重启后队列与顺序保持
- **版本切换**：超分完成后一键切换到超分版阅读，页码不丢失；阅读中随时切回原版

### 标签与整理

- **元数据标签分层折叠**：作者 / 类别 / 系列 / AI超分 / 状态分组展示，每组独立展开收起
- **已读标记**：打开自动标记"已读"，也可批量打标（含文件夹递归）
- **标签管理页**：统计每个标签关联的漫画数与总阅读次数，支持重命名、删除
- **漫画排序**：书源浏览支持按**字母**（数字感知）或**加入时间**（最新在前）排序
- **自动转 CBZ**：本地刷新时后台将漫画文件夹 / zip 自动打包为 CBZ（全局设置可关闭）
- **后缀别名识别**：`zip` 改名 `cbz`（或文件夹转 CBZ）后仍视为同一本书，进度、标签无缝延续

### WebDAV 远程书架

- 连接支持 WebDAV 的 NAS / 网盘，PROPFIND 浏览目录
- **流式阅读**：支持 Range 的服务器边下边读；否则首次整本下载到本地缓存，之后秒开
- 下载进度条实时显示；封面懒加载，已下载漫画封面不走网络

### 数据与隐私

- 阅读记录、标签、元数据、设置存于本地 **SQLite**，并有 JSON 备份
- **缓存目录可自定义**：可在设置中迁移整个数据根目录（数据库 + 缓存一起搬），重启自动恢复
- **五级缓存管理面板**：页面 / 整本下载 / 封面 / 旧下载 / AI 超分分类展示占用空间，可单独清理
- 全程无账号体系，不上传任何数据

## 📦 版本历史

| 版本 | 日期 | 摘要 |
|------|------|------|
| [v0.3.1](https://github.com/ChangfengluoO71/RCH/releases) | 2026-08-02 | 阅读器页面旋转、自动转 CBZ、标签分层折叠、AI 任务拖拽排序、漫画排序等 |
| [v0.3.0](https://github.com/ChangfengluoO71/RCH/releases/tag/v0.3.0) | 2026-08-02 | AI 超分后台队列、缓存目录迁移重构、标签持久化修复 |
| v0.2.1 | 2026-07-28 | 封面磁盘缓存、封面加载并发控制、Repository 层重构 |
| v0.1.0 | 2026-07-28 | 流式阅读引擎、8 种格式、WebDAV、标签/搜索/封面系统 |

完整变更记录见 [CHANGELOG.md](CHANGELOG.md)。

> **版本规则**：自 0.3.0 起，每次发布仅递增最后一位补丁号（0.3.0 → 0.3.1 → 0.3.2），新功能与修复统一按此推进。

## 🛠️ 开发

- **技术栈**：Flutter（Dart）负责界面，Rust 负责核心（流式解析、缓存、解码、AI 调度），通过 `flutter_rust_bridge` 桥接
- **架构说明**：[docs/architecture.md](docs/architecture.md)；设计目标 [SPEC.md](SPEC.md)；任务看板 [TODO.md](TODO.md)；重大决策 [DECISION.md](DECISION.md)；开发历史 [LOG.md](LOG.md)
- **Rust 单元测试**：`cd app/rust && cargo test`
- **修改 Rust API 后重新生成桥接**：`cd app && flutter_rust_bridge_codegen generate`
- 贡献指南见 [CONTRIBUTING.md](CONTRIBUTING.md) 与 [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md)

## 📄 许可证

[MIT](LICENSE)
