# RCH

**RCH — Local-first Comic Reader & Library Manager**

一个基于 **Flutter + Rust** 构建的跨平台漫画阅读与个人漫画库管理器。

RCH 的核心目标不是单纯“打开漫画文件”，而是把**本地漫画、远程书源、阅读记录、标签、元数据、缓存和多设备同步**统一到一个本地优先（Local-first）的系统中。

支持 Windows 与 Android，同一套核心代码跨平台运行。

[![Flutter](https://img.shields.io/badge/Flutter-3.x-02569B?logo=flutter)](https://flutter.dev/)
[![Rust](https://img.shields.io/badge/Rust-Core-000000?logo=rust)](https://www.rust-lang.org/)
[![Platform](https://img.shields.io/badge/Platform-Windows%20%7C%20Android-blue)](#)
[![License](https://img.shields.io/badge/License-MIT-green)](LICENSE)

---

## 为什么是 RCH？

传统漫画阅读器通常把“阅读”作为唯一核心功能：打开文件、翻页、记录进度。

但当漫画数量达到数百甚至数千本之后，真正的问题开始变成：

* 漫画分散在不同目录、NAS 和网盘中；
* 大型压缩包不希望每次完整解压；
* 远程文件希望直接阅读，而不是先下载整本；
* 不同格式的漫画需要统一管理；
* 标签、阅读记录和元数据需要长期保存；
* Windows 与 Android 上的数据希望保持一致；
* 本地漫画与云端漫画希望使用相同的浏览和阅读体验。

RCH 因此采用了 **Local-first + Provider-based + Rust Core** 的设计。

漫画文件仍然属于用户自己的存储系统，RCH 负责建立统一的索引、元数据、阅读状态和访问层。

换句话说：

> **RCH 管理的是你的漫画库，而不仅仅是漫画文件。**

---

## 核心特性

### 📚 一个阅读器，多种漫画格式

无需为了不同格式安装不同阅读器。

目前支持：

* ZIP / CBZ
* EPUB
* CB7 / 7Z
* CBT / TAR
* PDF
* CBR / RAR
* MOBI / AZW / AZW3
* 普通图片文件夹

对于 ZIP / CBZ / EPUB 等压缩格式，RCH 可以直接读取压缩包中央目录，并按需读取页面，而不是启动时完整解压整个文件。

因此，即使漫画压缩包达到数百 MB，也可以直接进入阅读。

---

### ⚡ Streaming-first 阅读

RCH 的阅读链路并不要求“先把整本漫画下载下来”。

对于支持随机访问或 HTTP Range 的数据源：

```text
Remote Source
     │
     ▼
Range / Streaming Access
     │
     ▼
Archive Parser
     │
     ▼
Page-level Read
     │
     ▼
Local Cache
     │
     ▼
Flutter Reader
```

用户可以在远程漫画尚未完整下载的情况下开始阅读。

对于不支持流式读取的远程服务，则自动回退到整本下载并缓存到本地。

阅读策略可以统一配置为：

* 自动
* 优先整本下载
* 直接流式阅读

---

### 🖥️ Windows + 📱 Android

RCH 使用 Flutter 构建跨平台 UI，同时将文件解析、I/O、缓存、索引等核心能力放在 Rust 层。

```text
┌──────────────────────────────────┐
│           Flutter UI             │
│                                  │
│ Library · Reader · Search        │
│ Tags · Settings · Sync           │
└────────────────┬─────────────────┘
                 │
        flutter_rust_bridge
                 │
┌────────────────▼─────────────────┐
│             Rust Core            │
│                                  │
│ Archive Parsing                  │
│ File / Network I/O               │
│ Library Index                    │
│ Metadata                         │
│ Cache                            │
│ Remote Providers                 │
│ Synchronization                  │
└───────────────┬──────────────────┘
                │
        ┌───────┴────────┐
        ▼                ▼
    Local Storage    Remote Sources
        │                │
      SQLite       WebDAV / SMB
      Cache        SFTP / Cloud
```

这种架构使 Flutter 负责交互和展示，而 Rust 负责复杂的数据访问与处理逻辑，从而避免将大量文件系统和网络逻辑堆积在 Dart 层。

---

## 🧩 Remote Sources

RCH 将不同存储系统抽象为统一的 **Book Source / Provider**。

目前支持：

| Source | 阅读 | 下载 | 流式 | 备注               |
| ------ | -: | -: | -: | ---------------- |
| 本地目录   |  ✓ |  ✓ |  ✓ | 本地优先             |
| WebDAV |  ✓ |  ✓ |  ✓ | 支持 Range 时可流式    |
| SMB    |  ✓ |  ✓ |  ✓ | NAS / Windows 共享 |
| SFTP   |  ✓ |  ✓ |  ✓ | 密码认证             |
| 百度网盘   |  ✓ |  ✓ |  ✓ | 官方 API           |
| 115 网盘 |  ✓ |  ✓ |  ✓ | Cookie / 官方 API  |
| 夸克网盘   |  ✓ |  ✓ |  ✓ | Cookie 认证        |

远程书源不会改变用户原始文件的位置。

RCH 保存的是：

```text
Source
 ├── Connection
 ├── Remote Path
 ├── Library Metadata
 ├── Local Index
 └── Cache
```

而不是把所有漫画复制到 RCH 自己的目录中。

---

## 🔄 多设备同步

从 v0.5.0 开始，RCH 使用基于 WebDAV 的状态同步机制。

同步的是**漫画库状态**，而不是漫画文件本体。

支持同步：

* 书源配置
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

同步模型：

```text
              WebDAV
             Sync Store
                 │
        ┌────────┴────────┐
        ▼                 ▼
     Windows            Android
        │                 │
     Local DB          Local DB
        │                 │
     Local Cache       Local Cache
```

不同设备拥有各自的本地文件，因此同步解决的是：

> **“我的漫画库状态是什么？”**

而不是：

> “把所有漫画文件复制到另一台设备。”

RCH 使用三方合并模型处理多设备修改，尽量避免简单的“后写覆盖前写”。

---

## 🗂️ Local-first 数据模型

RCH 的核心数据保存在本地 SQLite。

主要包括：

* 漫画元数据
* 标签
* 阅读历史
* 阅读进度
* 书源配置
* 本地索引
* 缓存状态
* 应用设置

因此，即使网络不可用，本地已经建立的数据仍然可以继续使用。

远程书源还支持**离线索引**。

当用户浏览、阅读或标记远程漫画后，RCH 可以逐渐建立本地目录快照，使部分云端书架即使在没有网络连接时，也可以继续显示已经索引过的内容。

---

## 🖼️ 阅读体验

### 三种阅读模式

支持：

* 日漫：右翻
* 美漫：左翻
* 条漫：纵向滚动

同时支持双页模式，并可以将首页单独显示。

### 页面控制

支持：

* 缩放
* 拖动
* 页面旋转
* 单页旋转状态记忆
* 鼠标 / 触控板操作
* 键盘快捷键
* Android 触控操作

### 阅读进度

RCH 会记录：

* 当前页
* 阅读位置
* 阅读次数
* 最近阅读时间
* 已读状态

重新打开漫画后可以直接恢复到上次阅读位置。

### 预取与缓存

阅读器会在后台预取附近页面，并使用本地缓存减少重复读取。

目标是让：

> **网络 I/O、文件解析、页面缓存与 UI 翻页相互解耦。**

---

## 🤖 AI 2× Super Resolution

RCH 集成端侧 AI 超分辨率能力。

目前 Windows 支持 **Real-ESRGAN 2×** 推理。

支持：

* 当前页面即时超分
* 整本漫画后台超分
* 多任务队列
* 任务取消
* 队列持久化
* 超分版本与原版切换

典型工作流：

```text
Original Page
     │
     ▼
Real-ESRGAN
     │
     ▼
2× Upscaled Page
     │
     ▼
Local Cache
     │
     ├── Original
     └── Super Resolution
```

超分结果作为独立缓存保存，不会覆盖用户原始漫画文件。

---

## 🏷️ 漫画库管理

RCH 不只是文件浏览器。

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

标签可以进行分组和折叠管理。

同时支持：

* 关键词搜索
* 标签筛选
* 多条件组合过滤
* 字母排序
* 加入时间排序
* 批量标记
* 文件夹递归操作

---

## 🗃️ 文件夹漫画

普通图片文件夹也可以直接作为漫画库。

RCH 会自动识别包含漫画图片的目录，并尝试读取：

```text
ComicInfo.xml
metadata.json
```

用于获取：

* 标题
* 作者
* 系列
* 类别
* 其他元数据

对于部分本地漫画目录，RCH 还可以后台自动转换为 CBZ，以便统一管理。

同时支持文件后缀别名识别，例如：

```text
book.zip
book.cbz
```

在满足对应文件条件时可以被视为同一本漫画，从而保持已有的阅读记录和标签。

---

## 💾 缓存管理

RCH 将不同类型的缓存分开管理：

* 页面缓存
* 整本下载
* 封面缩略图
* AI 超分结果
* 临时文件

用户可以在应用内查看各类缓存的空间占用，并单独清理。

同时支持迁移整个数据根目录。

因此可以将数据库与缓存一起迁移到容量更大的磁盘，而不需要手动修改大量路径。

---

## 🔐 隐私

RCH 遵循 Local-first 原则。

默认情况下：

* 不需要 RCH 账号
* 阅读记录保存在本地
* 标签保存在本地
* 元数据保存在本地
* 数据库保存在本地
* 缓存保存在本地
* 不上传用户漫画文件

远程书源的访问仍然直接连接用户配置的服务。

同步功能也由用户自行配置 WebDAV 存储。

**RCH 本身不是一个云端漫画服务。**

---

# 🛠️ Installation

## Windows

从 [GitHub Releases](../../releases) 获取最新 Windows x64 安装包。

Windows 10 / 11 x64 均可使用。

安装后可以通过：

```text
设置 → 关于与更新
```

检查新版本。

---

## Android

下载最新 APK 后安装即可。

当前提供：

* arm64-v8a
* armeabi-v7a
* x86_64

绝大多数现代 Android 手机优先使用 `arm64-v8a`。

---

# 🚀 Development

## Requirements

开发 RCH 需要：

* Flutter SDK
* Dart SDK
* Rust toolchain
* Android SDK / NDK（Android 开发）
* Visual Studio / Build Tools（Windows）
* `flutter_rust_bridge_codegen`

完整环境配置请参阅：

[SETUP.md](SETUP.md)

---

## Clone

```bash
git clone https://github.com/ChangfengluoO71/RCH.git
cd RCH
```

进入 Flutter 项目：

```bash
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

# 🏗️ Project Structure

```text
RCH/
├── app/
│   ├── lib/              # Flutter UI / application layer
│   ├── rust/             # Rust core
│   ├── android/          # Android platform
│   └── windows/          # Windows platform
│
├── docs/                 # Architecture / design documentation
├── .github/
│   └── workflows/        # CI / automation
│
├── CHANGELOG.md
├── CONTRIBUTING.md
├── SETUP.md
├── AGENTS.md
└── README.md
```

RCH 将 UI、业务逻辑、平台能力和底层 I/O 尽可能进行分层。

Flutter 主要负责：

* UI
* 用户交互
* 页面状态
* 阅读器呈现

Rust 主要负责：

* 文件访问
* 压缩包解析
* 网络 I/O
* Remote Provider
* 索引
* 缓存
* 数据处理
* 同步相关核心逻辑

两侧通过 `flutter_rust_bridge` 连接。

---

# 🧠 Architecture Principles

RCH 的设计主要围绕几个原则展开。

### Local-first

本地数据库和缓存是系统的主要工作状态，而不是远程服务的临时镜像。

### Provider-based

不同的文件来源通过统一 Provider 抽象接入。

```text
             Book Source
                  │
        ┌─────────┼─────────┐
        ▼         ▼         ▼
      Local     WebDAV     SFTP
        │         │         │
        └─────────┼─────────┘
                  ▼
           Unified Reader
```

这样阅读器不需要知道漫画来自本地磁盘、NAS 还是网盘。

### Streaming-first

能 Range / Streaming 就不强制完整下载。

不支持随机访问的服务再回退到完整下载。

### Cache-aware

网络资源和本地缓存是两个不同层次。

缓存可以被删除，而不会影响原始漫画或漫画库状态。

### Metadata over File Copy

RCH 尽量保存：

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

因此，RCH 更接近一个**漫画库管理层**，而不是一个新的文件存储系统。

---

# 📖 Documentation

开发环境配置：

[`SETUP.md`](SETUP.md)

版本变化：

[`CHANGELOG.md`](CHANGELOG.md)

贡献指南：

[`CONTRIBUTING.md`](CONTRIBUTING.md)

---

# 🗺️ Roadmap

RCH 仍处于快速迭代阶段。

后续重点方向包括：

* 更完善的跨设备同步
* 更丰富的远程 Provider
* 更完整的漫画元数据体系
* 更强的本地搜索与筛选
* 更完善的阅读器交互
* Android 端 AI 能力探索
* 更完善的缓存与索引策略
* 性能优化与大型漫画库测试
* 更完善的自动化测试

Roadmap 会随着实际开发进度调整。

---

# 🤝 Contributing

欢迎提交：

* Bug Report
* Feature Request
* Pull Request
* Documentation Improvement

在提交 Issue 前，建议先确认是否已经存在类似问题。

开发环境和项目结构请参考：

[`CONTRIBUTING.md`](CONTRIBUTING.md)

---

# ⚠️ Disclaimer

RCH 是一个通用的漫画阅读与个人媒体库管理工具。

RCH 不提供任何漫画内容，也不负责用户通过第三方服务访问的内容。

用户应当确保自己拥有访问、下载和阅读相关内容所需的合法权利，并遵守所在地区以及相关服务提供商的使用条款。

---

# 📄 License

RCH 使用 [MIT License](LICENSE)。

---

## Star History

如果 RCH 对你有帮助，可以给项目一个 Star。

GitHub:

https://github.com/ChangfengluoO71/RCH

---

**RCH**

> Local-first.
> Read anywhere.
> Keep your library yours.
