# RCH

**RCH — Local-first Comic Reader & Library Manager**

一个基于 **Flutter + Rust** 构建的跨平台漫画阅读与个人漫画库管理器。

RCH 不只是一个“打开漫画文件”的阅读器，而是希望把**本地漫画、NAS、远程网盘、阅读记录、标签、元数据、缓存和多设备同步**统一到一个 Local-first 的漫画库系统中。

支持 **Windows 与 Android**，核心能力由 Rust 实现，Flutter 负责跨平台 UI。

---

## 📑 目录

* [✨ 核心亮点](#-核心亮点)
* [🚀 快速开始](#-快速开始)
* [⬇️ 下载](#️-下载)
* [📖 用户手册](#-用户手册)
* [📚 功能概览](#-功能概览)
* [🏗️ 技术架构](#️-技术架构)
* [🔄 多设备同步](#-多设备同步)
* [🔐 数据与隐私](#-数据与隐私)
* [🛠️ 开发](#️-开发)
* [📚 项目文档](#-项目文档)
* [🗺️ Roadmap](#️-roadmap)
* [🤝 贡献](#-贡献)
* [📄 License](#-license)

---

## ✨ 核心亮点

* 📚 **多格式漫画阅读** — ZIP / CBZ、EPUB、CB7 / 7Z、CBT / TAR、PDF、CBR / RAR、MOBI / AZW / AZW3、图片文件夹
* ⚡ **压缩包按需读取** — 无需提前完整解压，大型漫画可以直接打开阅读
* 🖼️ **本地漫画库** — 自动扫描漫画目录、生成封面、读取 `ComicInfo.xml` / `metadata.json`
* 📖 **多种阅读模式** — 日漫、美漫、条漫，支持双页模式、缩放、拖动、旋转和阅读进度记忆
* 🤖 **AI 2× 超分** — Windows 端 Real-ESRGAN 端侧推理，支持单页与整本后台超分
* 🏷️ **标签与元数据管理** — 作者、系列、类别、状态、自定义标签、已读状态、阅读记录
* ☁️ **多源远程书架** — WebDAV / SMB / SFTP / 百度网盘 / 115 / 夸克网盘
* 🌊 **Streaming-first** — 支持 Range 的远程源可以边下边读，不支持时自动回退到整本下载
* 💾 **本地缓存** — 页面、整本下载、封面、AI 超分等缓存独立管理
* 🔄 **多设备同步** — 基于 WebDAV 的状态同步与三方合并
* 🔒 **Local-first** — 漫画文件保留在用户自己的存储中，RCH 不提供自己的云端漫画存储服务

---

## 🚀 快速开始

### 普通用户

直接前往：

**[⬇️ GitHub Releases](https://github.com/ChangfengluoO71/RCH/releases)**

下载对应平台的安装包。

安装完成后，建议第一次使用按照以下顺序进行：

```text
安装 RCH
  ↓
添加本地漫画目录
  ↓
确认可以正常阅读
  ↓
根据需要添加远程书源
  ↓
配置缓存与阅读策略
  ↓
需要多设备时再开启同步
```

完整的安装、配置、阅读、远程书源、同步和故障排查流程，请直接阅读：

> 📖 **[RCH 用户手册](user-guide.md)**

---

## ⬇️ 下载

最新版本请前往：

**[GitHub Releases](https://github.com/ChangfengluoO71/RCH/releases)**

当前稳定版本：

**v0.5.1**

| 平台                  | 文件                          | 说明                           |
| ------------------- | --------------------------- | ---------------------------- |
| Windows 10 / 11 x64 | `RCH-0.5.1-windows-x64.exe` | Windows 桌面版                  |
| Android             | `app-release.apk`           | arm64 / armeabi-v7a / x86_64 |

### Windows

下载安装包后直接运行。

如果 Windows SmartScreen 弹出提示，可以选择：

`更多信息 → 仍要运行`

安装完成后可以在：

`设置 → 关于与更新`

检查当前版本。

### Android

直接安装 APK。

如果系统提示禁止安装未知来源应用，请在 Android 设置中允许对应安装权限。

现代 Android 设备优先选择：

`arm64-v8a`

---

### 国内下载较慢

RCH 提供下载通道与镜像切换功能。

安装后进入：

`设置 → 关于与更新 → 下载通道`

可以选择可用镜像，也可以填写自定义镜像前缀。

应用会自动更新镜像列表，并在下载失败时尝试切换通道。

---

## 📖 用户手册

**RCH 的完整操作说明统一维护在仓库根目录的 [`user-guide.md`](user-guide.md)。**

用户手册不是开发文档，而是面向普通用户的完整使用说明。

其中包括：

| 内容      | 说明                         |
| ------- | -------------------------- |
| 安装与首次启动 | Windows / Android 安装、第一次使用 |
| 本地漫画    | 添加目录、压缩包、文件夹漫画             |
| 阅读器     | 阅读模式、双页、缩放、旋转、进度           |
| 标签与管理   | 标签、搜索、排序、已读状态              |
| 远程书源    | WebDAV / SMB / SFTP        |
| 网盘书源    | 百度网盘 / 115 / 夸克            |
| 远程阅读    | 流式 / 整本下载 / 自动策略           |
| 缓存      | 页面、下载、封面、AI 超分缓存           |
| AI 超分   | Real-ESRGAN 2× 使用方式        |
| 多设备同步   | WebDAV 同步与设备配置             |
| 离线索引    | 云端书架离线浏览                   |
| 数据迁移    | 数据目录与缓存迁移                  |
| 版本升级    | 升级注意事项与同步重置                |
| FAQ     | 常见错误与故障排查                  |
| 隐私安全    | Cookie / Token / 本地数据库注意事项 |

### 推荐阅读路径

第一次使用：

**[开始使用](user-guide.md#1-开始使用)** → **[添加本地漫画](user-guide.md#2-添加本地漫画)** → **[阅读漫画](user-guide.md#3-阅读漫画)**

使用 NAS / 服务器：

**[WebDAV](user-guide.md#51-webdav)** · **[SMB](user-guide.md#52-smb)** · **[SFTP](user-guide.md#53-sftp)**

使用网盘：

**[百度网盘](user-guide.md#54-百度网盘)** · **[115 网盘](user-guide.md#55-115-网盘)** · **[夸克网盘](user-guide.md#56-夸克网盘)**

Windows + Android：

**[多设备同步](user-guide.md#9-多设备同步)** → **[离线索引](user-guide.md#10-离线索引)**

遇到问题：

**[常见问题](user-guide.md#13-常见问题)**

> **普通用户只需要阅读 [`user-guide.md`](user-guide.md)。**
>
> README 负责介绍项目；用户手册负责回答“具体怎么用”。

---

## 📚 功能概览

### 打开即读，多种格式

RCH 支持：

```text
ZIP / CBZ
EPUB
CB7 / 7Z
CBT / TAR
PDF
CBR / RAR
MOBI / AZW / AZW3
图片文件夹
```

对于 ZIP / CBZ / EPUB 等压缩格式，RCH 会读取压缩包目录并按页面需要读取内容，而不是启动阅读器时完整解压整个文件。

因此，大型漫画压缩包也可以直接进入阅读。

---

### 阅读器

支持三种主要阅读模式：

* 日漫：右翻
* 美漫：左翻
* 条漫：纵向滚动

同时支持：

* 双页模式
* 首页单独显示
* 缩放
* 拖动
* 页面旋转
* 单页旋转状态记忆
* 鼠标 / 触控板
* 键盘操作
* Android 触控操作

阅读进度会持续保存，重新打开漫画时可以恢复到上次阅读位置。

阅读器还会预取前后页面，并通过本地磁盘缓存减少重复读取。

---

### 漫画库

本地目录中的漫画可以自动扫描并生成海报墙。

包含图片的文件夹可以直接被识别为漫画。

如果目录中存在：

```text
ComicInfo.xml
metadata.json
```

RCH 会尝试读取其中的标题、作者、系列、类别等信息。

本地漫画还支持后台自动转换为 CBZ；如果不希望转换，可以在设置中关闭。

---

### 标签与管理

支持：

* 作者
* 系列
* 类别
* 状态
* AI 超分
* 自定义标签
* 已读状态
* 阅读次数
* 阅读时间

同时支持：

* 关键词搜索
* 标签筛选
* 多条件组合筛选
* 数字感知排序
* 加入时间排序
* 批量标记
* 文件夹递归操作

---

### AI 2× Super Resolution

Windows 端支持 Real-ESRGAN 2× 端侧超分。

支持：

* 单页即时超分
* 整本漫画后台超分
* 多任务队列
* 调整任务顺序
* 取消任务
* 队列持久化
* 原版 / 超分版切换

超分结果保存为独立缓存，不会覆盖原始漫画。

---

### 远程书架

RCH 使用 Provider 抽象统一接入不同存储来源。

当前支持：

| 来源     | 能力                 |
| ------ | ------------------ |
| 本地目录   | 浏览 / 阅读 / 缓存       |
| WebDAV | 浏览 / Range 流式 / 下载 |
| SMB    | NAS / Windows 共享   |
| SFTP   | SSH 远程文件访问         |
| 百度网盘   | 官方 API / OAuth     |
| 115 网盘 | Cookie / 官方 APP ID |
| 夸克网盘   | Cookie 认证          |

详细配置方法全部放在：

**[用户手册 → 添加远程书源](user-guide.md#5-添加远程书源)**

---

### Remote Reading

远程漫画提供三种全局打开策略：

```text
自动
  ↓
优先整本下载
  ↓
失败后尝试流式
```

或者直接选择：

```text
优先下载整本
```

以及：

```text
直接流式
```

当远程服务支持 HTTP Range 时，可以边下载边阅读；不支持时，则回退到整本下载并进入本地缓存。

---

## 🔄 多设备同步

RCH 的同步目标是同步**漫画库状态**，而不是同步漫画文件本体。

当前可同步：

* 书源
* 漫画元数据
* 标题与封面裁剪
* 标签
* 阅读记录
* 阅读进度
* 设置
* 离线索引

默认不同步：

* 漫画文件本体
* 书源密码
* Cookie 等敏感凭据

同步基于 WebDAV 存储状态文件，并使用三方合并处理多设备修改。

典型结构：

```text
                    WebDAV
                  Sync Store
                       │
             ┌─────────┴─────────┐
             ▼                   ▼
          Windows              Android
             │                   │
          Local DB            Local DB
             │                   │
          Local Cache         Local Cache
```

这意味着：

> **同步的是“我的漫画库状态”，而不是“把漫画文件复制到另一台设备”。**

### 配置同步

进入：

`设置 → 同步`

填写相同的：

* WebDAV 地址
* WebDAV 账号
* WebDAV 密码
* 远程目录

然后为每台设备设置不同的设备名称。

完整配置与升级注意事项：

**[用户手册 → 多设备同步](user-guide.md#9-多设备同步)**

---

## 🗂️ 离线索引

离线索引是远程书源目录树的本地快照。

它保存：

* 路径
* 文件名
* 文件大小

因此，即使暂时没有网络，也可以继续浏览已经建立索引的远程目录。

云端书源通常会在以下操作过程中逐渐建立离线索引：

* 浏览目录
* 阅读漫画
* 缓存漫画
* 添加标签

也可以手动执行：

`生成离线索引`

或者：

`全量重建索引`

离线索引还可以随多设备同步传播到其他设备。

详细说明：

**[用户手册 → 离线索引](user-guide.md#10-离线索引)**

---

## 💾 缓存与数据管理

RCH 将不同缓存分类管理，包括：

* 页面缓存
* 整本下载
* 封面缩略图
* AI 超分结果
* 临时文件

进入缓存管理页面，可以查看空间占用并分别清理。

缓存目录可以迁移。

因此，如果系统盘空间不足，可以将整个 RCH 数据根目录迁移到更大的磁盘。

缓存删除不会影响远程源中的原始漫画，也不会删除本地原文件。

---

## 🔐 数据与隐私

RCH 没有自己的账号体系。

阅读记录、标签、元数据、应用设置等主要数据保存在本地 SQLite。

RCH 不提供自己的漫画云存储，因此：

**漫画文件仍然由用户自己管理。**

远程书源只负责访问用户配置的：

* NAS
* WebDAV
* SFTP
* SMB
* 百度网盘
* 115 网盘
* 夸克网盘

部分远程书源需要 Cookie、refresh token 或用户名密码。

这些凭证属于敏感信息。

因此：

**不要公开上传 RCH 数据库，也不要在 Issue、日志或截图中泄露 Cookie / Token。**

详细说明：

**[用户手册 → 隐私与安全](user-guide.md#14-隐私与安全)**

---

## 🏗️ 技术架构

RCH 采用 Flutter + Rust 的跨平台架构。

```text
┌──────────────────────────────────────┐
│              Flutter UI              │
│                                      │
│  Library · Reader · Search · Tags    │
│  Settings · Sync · AI Tasks         │
└──────────────────┬───────────────────┘
                   │
          flutter_rust_bridge
                   │
┌──────────────────▼───────────────────┐
│              Rust Core               │
│                                      │
│  Archive Parsing                     │
│  File / Network I/O                  │
│  Library Index                       │
│  Metadata                            │
│  Cache                               │
│  Remote Providers                    │
│  Synchronization                     │
└───────────────┬──────────────────────┘
                │
        ┌───────┴────────┐
        ▼                ▼
 Local Storage       Remote Sources
        │                │
      SQLite       WebDAV / SMB
      Cache        SFTP / Cloud
```

Flutter 负责：

* UI
* 用户交互
* 页面状态
* 阅读器界面

Rust 负责：

* 文件访问
* 压缩包解析
* 网络 I/O
* Remote Provider
* 本地索引
* 缓存
* 数据处理
* 同步相关核心逻辑

两者通过 `flutter_rust_bridge` 进行连接。

这种架构让 UI 层与复杂的文件、网络和数据访问逻辑保持相对独立。

---

## 🧠 设计原则

### Local-first

本地数据库是应用主要工作状态。

网络不可用时，已经建立的本地数据仍然可以继续使用。

### Provider-based

不同来源通过统一 Provider 抽象进入阅读器。

```text
Local
WebDAV
SMB
SFTP
Cloud
  ↓
Provider
  ↓
Unified Reader
```

阅读器不需要关心漫画来自哪里。

### Streaming-first

支持随机读取时，优先使用 Range / Streaming。

无法进行随机读取时，再退回整本下载。

### Cache-aware

远程数据与本地缓存分离。

缓存可以清理，但不会改变原始漫画来源。

### Metadata over File Copy

RCH 尽量管理：

```text
Metadata
Tags
Progress
History
Index
Cache
```

而不是复制：

```text
Original Comic Files
```

---

## 🛠️ 开发

### 环境要求

开发 RCH 需要：

* Flutter SDK
* Dart SDK
* Rust toolchain
* Android SDK / NDK
* Visual Studio / Build Tools
* `flutter_rust_bridge_codegen`

完整环境配置：

**[SETUP.md](SETUP.md)**

---

### Clone

```bash
git clone https://github.com/ChangfengluoO71/RCH.git
cd RCH
cd app
```

运行 Windows：

```bash
flutter run -d windows
```

运行 Android：

```bash
flutter run -d <android-device>
```

构建 Android Debug APK：

```bash
flutter build apk --debug
```

---

## 📚 项目文档

RCH 的文档主要位于**仓库根目录**，详细技术资料另外放在 `docs/`。

| 文档                             | 用途                                      |
| ------------------------------ | --------------------------------------- |
| **[用户手册](user-guide.md)**      | 普通用户使用 RCH 的完整指南                        |
| **[开发环境](SETUP.md)**           | Flutter / Rust / Android / Windows 开发环境 |
| **[更新日志](CHANGELOG.md)**       | 各版本功能、修复与变化                             |
| **[贡献指南](CONTRIBUTING.md)**    | Issue / Pull Request / 开发规范             |
| **[行为准则](CODE_OF_CONDUCT.md)** | 社区行为规范                                  |
| **[`docs/`](docs/)**           | 项目设计、架构及其他技术文档                          |
| **[AGENTS.md](AGENTS.md)**     | 项目开发与 Agent 协作约定                        |

### 推荐阅读顺序

普通用户：

**[用户手册](user-guide.md)**

准备开发：

**[开发环境](SETUP.md)** → **[`docs/`](docs/)** → **[贡献指南](CONTRIBUTING.md)**

想了解版本变化：

**[CHANGELOG.md](CHANGELOG.md)**

---

## 🗺️ Roadmap

RCH 仍在持续开发中。

后续重点方向包括：

* 更完善的漫画元数据管理
* 更强的搜索与筛选
* 更丰富的远程 Provider
* 更完善的同步与冲突处理
* 更完善的缓存与索引策略
* 更完整的自动化测试
* Android 端更多 AI 能力
* 大型漫画库性能优化
* 阅读器交互体验持续改进

Roadmap 会根据实际开发进度调整。

---

## 🤝 贡献

欢迎提交：

* Bug Report
* Feature Request
* Pull Request
* Documentation Improvement

提交 Issue 前，请先确认是否已经存在相同或相近的问题。

开发环境与项目结构请先阅读：

**[SETUP.md](SETUP.md)**

贡献规范请阅读：

**[CONTRIBUTING.md](CONTRIBUTING.md)**

---

## ⚠️ Disclaimer

RCH 是一个通用的漫画阅读与个人媒体库管理工具。

RCH 不提供漫画内容，也不负责用户通过第三方服务访问的内容。

用户应确保自己拥有访问、下载和阅读相关内容所需的合法权利，并遵守所在地区法律法规以及相关服务提供商的使用条款。

---

## 📄 License

RCH 使用 **MIT License**。

完整许可证：

[LICENSE](LICENSE)

---

## ⭐ Support the Project

如果 RCH 对你有帮助，可以给项目一个 Star。

GitHub：

https://github.com/ChangfengluoO71/RCH

---

**RCH**

> Local-first.
> Read anywhere.
> Keep your library yours.
