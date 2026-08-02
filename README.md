# RCH

**Windows-first, local-first streaming comic reader.**

打开 ZIP/CBZ/7Z/文件夹，边加载边阅读。

[![Flutter](https://img.shields.io/badge/Flutter-3.44-blue?logo=flutter)](https://flutter.dev)
[![Rust](https://img.shields.io/badge/Rust-1.80-orange?logo=rust)](https://www.rust-lang.org)
[![License](https://img.shields.io/badge/License-MIT-green)](LICENSE)
[![Platform](https://img.shields.io/badge/Platform-Windows%20%7C%20Android-brightgreen)]()

- ⚡ **超大压缩包秒开** 
- 📖 **连续阅读不等待** — 智能预取 + 三级缓存
- 🖼️ **8 种格式全覆盖** — ZIP/CBZ/PDF/EPUB/MOBI/文件夹适配
- 🪟 **漫画信息自定义** — 封面选取裁剪/深度标签编辑系统/
- 📱 **自带漫画搜索筛选系统** — 支持“标签 + 文字”双重精确筛选
- 🧠 **webdav支持** — WebDAV 封面懒加载 + 缓存进度显示 + 缓存一键清理
- 💾 **封面磁盘缓存** — 封面解码后写入磁盘，重启秒出；并发队列限流避免 IO 风暴

## 快速开始

```bash
git clone https://github.com/ChangfengluoO71/RCH.git
cd RCH/app
flutter run -d windows
```

**版本规范**：从 `0.3.0` 起，每次发布仅递增最后一位（`0.3.0 → 0.3.1 → 0.3.2 …`），新功能与修复统一按此推进。

> **四层架构**：UI → Document → Cache → Network/AI。**阅读器永远只操作本地资源，网络只是同步层，AI 只是处理层。**
> 当前阶段：**格式引擎已完成（8种格式），缓存基础设施已完成。**
> 目标设计见 [SPEC.md](SPEC.md);任务看板见 [TODO.md](TODO.md);重大决策见 [DECISION.md](DECISION.md);开发历史见 [LOG.md](LOG.md)。

## 当前功能

### 格式支持（8种格式全部完成）
| 格式 | 引擎 | 依赖 |
|---|---|---|
| ZIP / CBZ | `zip` + `flate2` | 无 |
| EPUB | `zip` + OPF spine | 无 |
| Folder | 目录枚举 + ComicInfo.xml + metadata.json | 无 |
| CB7 | `sevenz-rust` | 无 |
| CBT | `tar` | 无 |
| PDF | `pdfium-render` | pdfium.dll |
| CBR / RAR | `unrar` | unrar.dll |
| MOBI / AZW / AZW3 | `mobi` crate | 无 |

### 书源
- **本地书源**:浏览本地目录,海报墙展示 ZIP/CBZ/EPUB,目录可下钻
- **漫画文件夹识别**:包含图片的子目录自动检测为漫画，海报墙中显示封面卡片（优先 cover.jpg → 首页缩略图），点击直接进详情/开始阅读
- **漫画文件夹元数据**:自动读取 `ComicInfo.xml` 或 `metadata.json`（标题/作者/系列/类型），元数据优先级 ComicInfo.xml > metadata.json > 目录名
- **WebDAV 书源**:连接远程服务器(NAS/网盘),PROPFIND 列目录,流式阅读(支持 Range 的服务器)或整本下载到 raw/ 缓存(首次下载后自动缓存,后续秒开)。下载期间显示真实百分比进度条
- **WebDAV 封面生成优先走 raw/ 本地缓存**：已整本下载的漫画封面秒出，不走网络
- **目录型漫画元数据**：自动读取 ComicInfo.xml / metadata.json（标题/作者/系列/类型）
- **目录型漫画封面**：优先读取 cover.jpg/png/webp/jpeg，无显式封面时取首页做缩略图
- 书源详情页:查看服务器信息/路径,编辑备注,删除书源(连带清理记录)
- 书源管理:添加/编辑/删除/备注,凭据持久化,密码字段遮盖

### 阅读器
- **流式阅读**:ZIP/CBZ/EPUB 只读文件尾部中心目录,按需解压单页,大文件即点即读
- **流畅翻页**:L1 内存 LRU 缓存 + L2 磁盘缓存(读过的页写盘) + 后台并行预取
- **`+/-/0` 键缩放**(日漫/美漫/条漫三模式统一)
- **WebDAV 下载进度**: 首次下载显示百分比进度条 + 进度数字，每 300ms 轮询 Rust 端进度
- **AI 超分**: 右键菜单单页 2x 超分 + 详情页整本超分，端侧推理，结果缓存 ai/ 目录

### 主界面
- **左侧导航**:最近阅读 / 最多阅读 / 标签管理 / 书源列表 / 设置
- **海报墙书架**:网格封面缩略图 + 标题 + 文件大小（漫画文件夹自动显示封面卡片）
- **WebDAV 双模式**:海报墙 / 简略列表自由切换
- **最近阅读 / 最多阅读**:自动记录打开次数与进度,点击继续阅读(自动跳到上次看的页)
- **统一搜索栏**:输入 `#` + 文字 → 内联标签补全列表 → 点击补全 → Chip 展示 → 点 × 移除
- **跨书源搜索**:搜索栏右侧地球图标切换 → 跨所有书源搜索漫画（文字 + 标签联合过滤）
- **筛选当前视图**:筛选模式下文字/标签过滤最近阅读、最多阅读、书源浏览、标签管理

### 漫画详情与自定义
- 详情页:**元数据标签**(标题/中文标题/作者/类别/系列/已读) + 自由标签 + 简介 + 感想 + 大封面
- **已读标记**:详情页按钮一键切换已读/未读；打开漫画自动标记已读；支持批量操作
- **标签补全**:输入自动联想已有标签
- **封面自定义**:翻页选封面页 + 框选裁剪(保存相对区域)
- 海报墙封面支持自定义(质量可调:低/中/高)

### 标签管理
- 独立标签管理页:统计每个标签的关联漫画数和总阅读次数
- **元数据标签**(红色图标)和**普通标签**(黄色图标)分区显示,元数据标签可折叠
- **已读标签**是内置元数据标签(红色图标)，打开漫画自动添加，也可在详情页手动切换
- 标签详情:点击标签名在右侧查看该标签下的所有漫画(海报墙)+ 点击卡片进入详情
- 标签操作:重命名/删除(元数据标签也支持,同步更新 author/genre/series 三栏)

### 批量标签管理
- 书源浏览页「进入选择模式」→ 漫画/文件夹出现复选框 → 全选(含文件夹)/单选 → 批量打标签
- 文件夹勾选后打标签自动递归展开所有子目录漫画
- 标签输入框带自动补全,元数据标签(红色)和普通标签(黄色)在补全列表中区分颜色
- **批量标注已读**：输入"已读"即可批量标记选中的所有漫画

### 缓存管理
- 设置页五级缓存分类独立管理:页面缓存/整本下载(raw/)/封面缩略图(cover/)/旧下载(download/)/AI超分(ai/),每项展示实时占用空间和用途说明
- 每个分类独立清理按钮（清理前二次确认），清页面/raw/旧下载/AI 缓存不影响已加载的封面，仅「封面缩略图」按钮或「清空全部」才清除封面内存缓存
- 封面质量切换自动清封面内存缓存
- WebDAV 封面懒加载:未打开过的远程漫画不主动请求封面（显示"未缓存"占位）
- 封面加载失败自动重试（失败不缓存，下次重建 Widget 重试），失败显示云端断开图标而不永久转圈
- 清理失效记录仅删无效数据，不清封面缓存

## 已实现能力(后端)
- `ByteSource` 统一字节源抽象(本地/WebDAV/Range/整包回退)
- `Document` trait 统一格式抽象:**8种格式**(ZIP/CBZ/EPUB/Folder/CB7/CBT/PDF/CBR/MOBI)
- Folder 格式:支持 ComicInfo.xml / metadata.json 双元数据源 + cover.jpg 优先封面 + 漫画文件夹智能检测(`is_comic_folder`)
- WebDAV 封面懒加载:只列目录元数据,不主动生成封面
- L2 磁盘缓存:原始页字节写 `%APPDATA%/RCH/cache/`,重复阅读秒开
- WebDAV 封面生成优先走 raw/ 本地缓存（已下载漫画秒出，不走网络）
- 封面缩略图:Rust decode_cover(支持裁剪),自定义封面页+裁剪区域,失败自动重试
- 封面磁盘缓存:cover/ 目录存储解码后的 RGBA，重启秒出；并发队列限流 4 FFI 避免 IO 风暴
- 应用数据持久化:SQLite 主存储 + library.json 备份(sources/metas/records/tags/settings)
- 下载进度: Rust 端 AtomicU64 线程安全进度追踪 + Flutter 端每 300ms 轮询更新
- 五级缓存目录: raw / cover / thumb / ai / temp, 分类独立大小查询与清理
- AI 超分: 2x Real-ESRGAN animevideov3 (CLI 批量推理), 结果写入 ai/ 磁盘缓存

## 待建设（按优先级）

### M2 AI 超分
- [x] Phase 1: CLI 单次调用 + ai/ 缓存（已完成）
- [x] Phase 2: CLI 目录批量推理（已完成）
- [ ] Phase 3: ONNX Runtime 直接推理（模型已转 ONNX，待 ort crate 稳定）

### 小功能批量规划（2026-08-02，详见 [TODO.md](TODO.md)）
- [ ] M5 书源扩展（SMB / SFTP）
- [ ] 元数据标签分层折叠（作者/类别/系列/状态）
- [ ] 修复缩放后移动区域只在第一页生效
- [ ] 后缀名变更识别（zip → cbz 视为同一本）
- [ ] 本地漫画转 CBZ（文件夹 / ZIP 打包）
- [ ] AVIF 格式支持
- [ ] 阅读器页面旋转（M4 子集）


## 如何运行

### 环境准备
见 [SETUP.md](SETUP.md)（Rust / Flutter / VS 2022 BuildTools / flutter_rust_bridge_codegen）

### 启动(Windows)
```bash
cd app
flutter run -d windows
```

### Rust 单元测试
```bash
cd app/rust
cargo test
```

### 修改 Rust API 后重新生成桥接
```bash
cd app
flutter_rust_bridge_codegen generate
```

## 目录结构
```
RCH/
├─ CLAUDE.md / SPEC.md / README.md      # 约定 / 目标设计 / 当前状态
├─ LOG.md / LOG-INDEX.md / DECISION.md  # 开发历史 / 索引 / 架构决策
├─ TODO.md                              # 任务看板与工作流程
├─ CHANGELOG.md                         # 版本变更记录
├─ CONTRIBUTING.md                      # 贡献规范
├─ CODE_OF_CONDUCT.md                   # 社区行为准则
├─ SETUP.md                             # 环境搭建
├─ LICENSE                              # 开源许可 (MIT)
├─ docs/
│  └─ architecture.md                   # 系统架构文档
└─ app/                                 # Flutter 应用
   ├─ lib/
   │  ├─ main.dart                      # 入口(主题 + 启动加载)
   │  ├─ store/                         # 数据持久化(models + library_store)
   │  └─ ui/                            # 页面组件
   │     ├─ home_page.dart              # 主界面(左侧导航 + 右侧内容)
   │     ├─ source_browser.dart         # 书源浏览(海报墙/列表双模式 + 漫画文件夹识别 + 选择/批量)
   │     ├─ reader_page.dart            # 阅读器(+/-/0键缩放 + 双页 + 下载进度)
   │     ├─ book_detail_page.dart       # 漫画详情(元数据/标签/简介/感想)
   │     ├─ cover_editor_page.dart      # 封面编辑器(选页 + 裁剪)
   │     ├─ comic_cover.dart            # 统一封面组件(懒加载/失败重试/内存缓存)
   │     ├─ cache_manager.dart          # 缓存管理面板(五级分类独立管理)
   │     ├─ common.dart                 # 公共工具(fmtSize/fmtNum/rgbaToImage)
   │     └─ opener.dart                 # 打开书统一入口(含 WebDAV 会话缓存)
   ├─ rust/                             # Rust 核心引擎
   │  └─ src/ (api/ source/ document/ reader/ decode/ util/ cache/ downloader/)
   └─ rust_builder/                     # cargokit 构建集成
```
