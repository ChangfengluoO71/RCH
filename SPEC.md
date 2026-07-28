# RCH 漫画阅读器 — 设计规范(SPEC) v2.0

> 本文档是 RCH 的**核心指导规范**。修订须经用户确认，详见第 12 节。

- 版本：v2.0（架构重塑）
- 最近更新：2026-07-27
- 修订摘要：采用四层架构；核心原则「UI 永远只读本地缓存，网络只是同步层，AI 只是处理层」；多级缓存体系；统一下载器；SQLite 状态管理；WebDAV 保守请求策略

---

## 1. 项目愿景

RCH 是一款 **Windows 优先、面向多端（Windows / Android）** 的现代漫画阅读器。
核心体验：即点即读、本地优先、远程资源经缓存后阅读。内置端侧 AI 高清引擎。

---

## 2. 核心设计原则

1. **阅读器永远只操作本地资源，网络只是同步层，AI 只是处理层。**
2. **统一文档抽象**：所有格式经 `Document` trait 统一为"页面集合 + 元数据"，UI 不关心底层格式。
3. **本地缓存是核心**：第一次打开时整本下载到本地缓存，之后所有操作基于缓存。
4. **WebDAV 保守策略**：默认不读取封面、不解析压缩包、不读取第一页图片。仅当用户打开漫画时才下载。
5. **统一下载调度**：所有网络请求经 Downloader（队列 / 去重 / 并发限制 / 优先级 / 重试）。
6. **多级缓存分层**：raw / cover / thumb / ai / temp + SQLite 索引。
7. **AI 作为独立服务**：Worker 进程 + 命名管道，崩溃不影响主程序，模型可切换。
8. **AI 全流程基于缓存**：WebDAV → 本地缓存 → AI → AI 缓存 → 显示。
9. **书源可插拔**：统一 `BookSource` 接口。
10. **格式可插拔**：统一 `Document` trait，按扩展名分发。
11. **核心与界面分离**：Rust 引擎 + Flutter UI + FRB 桥接。
12. **流畅性是可验收指标**。

---

## 3. 四层系统架构

```
┌──────────────────────────────────────────────────────────┐
│ UI 层（Flutter）                                          │
│   书架 Library │ 阅读器 Reader │ 设置 Settings             │
├──────────────────────────────────────────────────────────┤
│ 桥接层 flutter_rust_bridge v2                             │
├──────────────────────────────────────────────────────────┤
│ Document Layer（Rust）                                    │
│   document/  统一格式抽象（ZIP/EPUB/PDF/… → 页面集合）    │
│   source/    BookSource 抽象                              │
├──────────────────────────────────────────────────────────┤
│ Cache Layer（Rust）                                       │
│   cache/        多级缓存（raw/cover/thumb/ai/temp）       │
│   downloader/   统一下载调度器                             │
│   db/           SQLite 状态索引                            │
│   ai/           AI Worker 管理 + Upscaler trait            │
└──────────────────────────────────────────────────────────┘
```

**核心规则：**

```
UI 永远只读 Document
Document 永远只读本地 Cache
网络只负责把资源同步到 Cache
AI 只处理 Cache 中的数据
```

---

## 4. 技术栈

| 层 | 选型 | 说明 |
|---|---|---|
| UI | Flutter（Windows + Android） | 一套代码两端 |
| 核心引擎 | Rust（cdylib） | IO/解压/解码/书源/AI |
| 桥接 | flutter_rust_bridge v2 | 代码生成 |
| AI 引擎 | Worker 架构（exe + 命名管道） | librealesrgan-ncnn-vulkan |
| 数据库 | SQLite（rusqlite） | 漫画索引 / 缓存状态 / 阅读进度 / 书源能力 |
| 缓存 | 五级目录 + SQLite 索引 | raw / cover / thumb / ai / temp |

**关键 Rust 依赖：**
`tokio` `reqwest` `zip` `image` `serde` `anyhow` `tracing`
`pdfium-render` `sevenz-rust` `tar` `mobi` `unrar`
`rusqlite` `interprocess` `memmap2`

---

## 5. 缓存体系

```
<userdata>/RCH/
├── cache/
│   ├── raw/            # 整本漫画原始文件（下载后存储）
│   ├── cover/          # 封面缓存（按质量/裁剪分）
│   ├── thumb/          # 缩略图缓存
│   ├── ai/             # AI 超分结果（按模型/倍率分）
│   └── temp/           # 临时文件（CB7/CBR 解压中间产物）
├── library.json        # 书源/阅读记录/元数据（已有，兼容旧版本）
└── database.db         # SQLite（新）：漫画索引 / 缓存 Hash / 书源能力 / ETag
```

**缓存策略：**
- raw：整本存储，Hash 命名，支持增量校验
- cover：L1 内存 LRU + L2 磁盘，WebDAV 默认不生成
- AI：按原图 Hash + 模型名 + 倍率存储，不覆盖原图

---

## 6. 下载器规范

建立统一下载调度中心（`downloader/`），所有网络请求必须经过它：

- 队列管理（FIFO + 优先级插队）
- 请求去重（同一 URL + Range 合并）
- 并发限制（WebDAV 默认 ≤2 并发）
- 下载优先级：当前阅读页 > 下一页预取 > 封面生成 > 后台缓存
- 重试策略：429 退避 / 401 停止 / 超时重试 3 次
- 网络状态检测

---

## 7. WebDAV 保守策略

| 操作 | 默认行为 |
|---|---|
| 浏览目录 | 只列文件名 / 大小 / 修改时间 |
| 封面 | 不请求（显示占位图标） |
| 解析压缩包 | 仅在用户点击打开时 |
| 读取第一页 | 仅在用户点击打开时 |
| 下载 | 整本下载到 raw/，后续全走本地 |
| Range 支持 | 自动探测，不支持则整本下载回退 |

---

## 8. AI 超分架构

```
WebDAV → raw/ → page_bytes → AI Worker → ai/ → 阅读器显示
```

- Worker 常驻进程，命名管道通信
- 默认关闭，用户右键触发
- 仅处理当前页 + 预加载 1 页
- 结果独立缓存，不覆盖原图
- Worker 崩溃自动重启，不影响阅读

---

## 9. 格式支持矩阵

| 格式 | 引擎 | 状态 |
|---|---|---|
| ZIP / CBZ | `zip` + `flate2` | 已完成 |
| EPUB | `zip` + OPF spine 自研 | 已完成 |
| Folder | 目录枚举 | 已完成 |
| CB7 | `sevenz-rust` | 已完成 |
| CBT | `tar` | 已完成 |
| PDF | `pdfium-render` | 已完成（需 pdfium.dll） |
| CBR / RAR | `unrar` | 已完成（需 unrar.dll） |
| MOBI / AZW / AZW3 | `mobi` crate | 已完成 |
| AVIF | `libavif` | 未来 |

---

## 10. 里程碑

| 里程碑 | 内容 | 状态 |
|---|---|---|
| **M1** 核心阅读闭环 | 书源 + ZIP/CBZ 流式 + 三模式 + 书架 | 基本完成 |
| **M2** AI 高清引擎 | Worker 架构 + Upscaler trait | **待实施** |
| **M3** 格式扩展 | PDF/EPUB/CB7/CBT/Folder | **已完成** |
| **M4** 复杂场景 | 智能拼页/旋转/裁边 | 待规划 |
| **M5** 格式+书源扩展 | MOBI/CBR/SMB/SFTP/网盘 | 格式已完成 |
| **M6** Android 适配 | 手机/平板 | 待规划 |
| **M7** 标签系统 | 标签筛选书架 | 数据模型已有 |
| **M8** 智能拓展 | AI 扫描 + 元数据分层 | 待规划 |
| **M9** 缓存基础设施 | 下载器 + SQLite + 五级缓存 | **新增，待实施** |

---

## 11. 非目标

- 不做在线漫画站爬虫/聚合
- 不做账号体系与云同步

---

## 12. 变更约束

1. SPEC 为最高设计指导，修订须经用户确认。
2. 每轮施工更新 LOG + LOG-INDEX。
3. 用户确认后更新 README 反映现状。
