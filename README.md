# RCH 漫画阅读器

Windows 优先、面向多端(Windows / Android)的现代漫画阅读器。核心体验是**本地优先的流式阅读**——无论什么格式、无论来自什么来源，都能边加载边阅读。

> **四层架构**：UI → Document → Cache → Network/AI。**阅读器永远只操作本地资源，网络只是同步层，AI 只是处理层。**
> 当前阶段：**格式引擎已完成（8种格式），缓存基础设施已完成。**
> 目标设计见 [SPEC.md](SPEC.md);任务看板见 [TODO.md](TODO.md);重大决策见 [DECISION.md](DECISION.md);开发历史见 [LOG.md](LOG.md)。

## 当前功能

### 格式支持（8种格式全部完成）
| 格式 | 引擎 | 依赖 |
|---|---|---|
| ZIP / CBZ | `zip` + `flate2` | 无 |
| EPUB | `zip` + OPF spine | 无 |
| Folder | 目录枚举 | 无 |
| CB7 | `sevenz-rust` | 无 |
| CBT | `tar` | 无 |
| PDF | `pdfium-render` | pdfium.dll |
| CBR / RAR | `unrar` | unrar.dll |
| MOBI / AZW / AZW3 | `mobi` crate | 无 |

### 书源
- **本地书源**:浏览本地目录,海报墙展示 ZIP/CBZ/EPUB,目录可下钻
- **WebDAV 书源**:连接远程服务器(NAS/网盘),PROPFIND 列目录,流式阅读(支持 Range 的服务器)或整本下载到 raw/ 缓存(首次下载后自动缓存,后续秒开)。下载期间显示真实百分比进度条
- **WebDAV 封面生成优先走 raw/ 本地缓存**：已整本下载的漫画封面秒出，不走网络
- 书源详情页:查看服务器信息/路径,编辑备注,删除书源(连带清理记录)
- 书源管理:添加/编辑/删除/备注,凭据持久化,密码字段遮盖

### 阅读器
- **流式阅读**:ZIP/CBZ/EPUB 只读文件尾部中心目录,按需解压单页,大文件即点即读
- **流畅翻页**:L1 内存 LRU 缓存 + L2 磁盘缓存(读过的页写盘) + 后台并行预取
- **`+/-/0` 键缩放**(日漫/美漫/条漫三模式统一)
- **WebDAV 下载进度**: 首次下载显示百分比进度条 + 进度数字，每 300ms 轮询 Rust 端进度

### 主界面
- **左侧导航**:最近阅读 / 最多阅读 / 标签管理 / 书源列表 / 设置
- **海报墙书架**:网格封面缩略图 + 标题 + 文件大小
- **WebDAV 双模式**:海报墙 / 简略列表自由切换
- **最近阅读 / 最多阅读**:自动记录打开次数与进度,点击继续阅读(自动跳到上次看的页)
- **统一搜索栏**:输入 `#` + 文字 → 内联标签补全列表 → 点击补全 → Chip 展示 → 点 × 移除
- **跨书源搜索**:搜索栏右侧地球图标切换 → 跨所有书源搜索漫画（文字 + 标签联合过滤）
- **筛选当前视图**:筛选模式下文字/标签过滤最近阅读、最多阅读、书源浏览、标签管理

### 漫画详情与自定义
- 详情页:**元数据标签**(标题/中文标题/作者/类别/系列) + 自由标签 + 简介 + 感想 + 大封面
- **标签补全**:输入自动联想已有标签
- **封面自定义**:翻页选封面页 + 框选裁剪(保存相对区域)
- 海报墙封面支持自定义(质量可调:低/中/高)

### 标签管理
- 独立标签管理页:统计每个标签的关联漫画数和总阅读次数
- **元数据标签**(红色图标)和**普通标签**(黄色图标)分区显示,元数据标签可折叠
- 标签详情:点击标签名在右侧查看该标签下的所有漫画(海报墙)+ 点击卡片进入详情
- 标签操作:重命名/删除(元数据标签也支持,同步更新 author/genre/series 三栏)

### 批量标签管理
- 书源浏览页「进入选择模式」→ 漫画/文件夹出现复选框 → 全选(含文件夹)/单选 → 批量打标签
- 文件夹勾选后打标签自动递归展开所有子目录漫画
- 标签输入框带自动补全,元数据标签(红色)和普通标签(黄色)在补全列表中区分颜色

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
- WebDAV 封面懒加载:只列目录元数据,不主动生成封面
- L2 磁盘缓存:原始页字节写 `%APPDATA%/RCH/cache/`,重复阅读秒开
- WebDAV 封面生成优先走 raw/ 本地缓存（已下载漫画秒出，不走网络）
- 封面缩略图:Rust decode_cover(支持裁剪),自定义封面页+裁剪区域,失败自动重试
- 应用数据持久化:JSON(library.json),存书源/阅读记录/元数据/设置
- 下载进度: Rust 端 AtomicU64 线程安全进度追踪 + Flutter 端每 300ms 轮询更新
- 五级缓存目录: raw / cover / thumb / ai / temp, 分类独立大小查询与清理

## 待建设（按优先级）

### M9 缓存基础设施（下一优先）
- [x] 五级缓存目录: raw/cover/thumb/ai/temp + 分类清理 API + 分级缓存管理面板
- [x] WebDAV 整本下载到 raw/ 缓存 + 百分比进度条（每 300ms 轮询）
- [x] Rust 侧 SQLite (rusqlite): 缓存索引/书源能力/ETag
- [ ] 统一下载器（Downloader）: 队列/去重/并发限制/优先级/重试

### M2 AI 超分
- [ ] Phase 1: 常驻 Worker 进程 + 命名管道通信
- [ ] Phase 2: 共享内存传图
- [ ] Phase 3: Upscaler trait 多模型切换

### M1 低优先级暂缓
- [ ] 双页自动/滚轮解耦/按键扩展

## 格式支持现状
| 格式 | 引擎 | 状态 |
|---|---|---|
| ZIP / CBZ | `zip` + `flate2` 流式解析 | 已完成 |
| EPUB | `zip` + OPF spine 自研 | 已完成 |
| Folder | 目录枚举 | 已完成 |
| CB7 | `sevenz-rust` 纯 Rust | 已完成 |
| CBT | `tar` | 已完成 |
| PDF | `pdfium-render` | 已完成(需 pdfium.dll) |
| CBR / RAR | `unrar` | 已完成(需 unrar.dll) |
| MOBI / AZW / AZW3 | `mobi` crate 纯 Rust | 已完成 |

## 已知问题
- 115 等有限请求频率限制的网盘在短时间内连接多次可能触发"Too many requests"
- 封面缩略图首次生成需打开 ZIP 取页,多本书同时加载时略慢(质量调低可加快)
- 双页拼接自动模式(宽图判定)已被移除,仅保留固定配对模式
- 滚轮缩放功能已全局关闭,改用 `+/-/0` 键缩放
- EPUB 元数据(title/author)尚未从 OPF 中提取,当前标题用文件名
- PDF/CBR 需要运行时 DLL(pdfium.dll / unrar.dll),未打包时对应格式无法打开
- WebDAV 封面缓存清除后,未整本下载的漫画封面需重新从远程请求,可能较慢
- WebDAV 封面缓存清除后，未整本下载的漫画封面需重新从远程请求，可能较慢

## 如何运行

### 环境准备
见 [docs/SETUP.md](docs/SETUP.md)(Rust / Flutter / VS 2022 BuildTools / flutter_rust_bridge_codegen)

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
├─ docs/SETUP.md                        # 环境搭建
└─ app/                                 # Flutter 应用
   ├─ lib/
   │  ├─ main.dart                      # 入口(主题 + 启动加载)
   │  ├─ store/                         # 数据持久化(models + library_store)
   │  └─ ui/                            # 页面组件
   │     ├─ home_page.dart              # 主界面(左侧导航 + 右侧内容)
   │     ├─ source_browser.dart         # 书源浏览(海报墙/列表双模式 + 选择/批量)
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
