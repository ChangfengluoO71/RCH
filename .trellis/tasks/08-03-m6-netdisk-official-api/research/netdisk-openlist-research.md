# 网盘适配调研：OpenList / AList（2026-08-03）

> 结论先行：**OpenList 是 AList 的无缝替代品（配置/API/数据库 100% 兼容）**，聚合 40+ 网盘（阿里云盘、百度网盘、夸克、OneDrive、115、Google Drive、WebDAV 等），支持 Windows 部署，且自带 WebDAV 服务。RCH 集成它 = 一次对接覆盖全部主流网盘。

## 1. 项目背景

- **AList**（`AlistGo/alist`）：Go 写的网盘聚合/文件列表程序，40+ 驱动，Web UI + WebDAV + 开放 HTTP API。2025 年被卖给"不够科技"公司，官网 404，社区信任受损。
- **OpenList**（`OpenListTeam/OpenList`）：社区 fork 的 AList 平替，宣称"配置、API、数据库 100% 兼容，直接替换就能用"，长期治理、完全开源。有 Windows 版本（zip / 桌面客户端 openlist-desktop，也支持 rclone 本地挂载）。另有 `qnap-openlist-webdav` 等 WebDAV 网关变体。
- 生态工具：`openlistapp`（GUI 客户端 + 内置 OpenList 服务端，Windows/Android 等，免部署）。

## 2. 为什么对 RCH 有吸引力

RCH 已有 WebDAV 书源。用户自建 OpenList/AList 后：

- **零代码路径**：直接以 WebDAV 书源接入（OpenList/AList 自带 WebDAV 服务）——今天就能用。
- **原生路径（推荐做）**：新增书源类型「网盘(Alist/OpenList)」，直连其 HTTP API，一次集成覆盖 40+ 网盘，体验优于 WebDAV（无 WebDAV 兼容坑、直链下载、可做进度/缓存）。

## 3. AList/OpenList API 契约（客户端只需 3 个端点）

Base URL 如 `http://192.168.1.10:5244`，鉴权头 `Authorization: <token>`（用户从管理后台"设置→令牌"复制，或用 `POST /api/auth/login` 换取）。

| 端点 | 方法/请求 | 关键响应 |
|---|---|---|
| `/api/fs/list` | `POST {"path":"/","password":"","page":1,"per_page":200,"refresh":false}` | `data.content[]`：`name` / `size` / `is_dir` / `modified`(RFC3339) / `thumb`；分页 `has_more` / `page` / `per_page` |
| `/api/fs/get` | `POST {"path":"/xx.cbz","password":""}` | 文件信息 + **`raw_url`**（直链下载地址） |
| `/api/auth/login` | `POST {username,password}` | `data.token` |

下载：`raw_url`（或 `/d/<path>` 302 跳转）支持 HTTP Range 与否取决于底层驱动（阿里云盘/OneDrive 直链一般支持）。**打开策略沿用 RCH 现有三态**：`auto`=先整本下载 raw/ 缓存（进度）→失败回退 Range 流式；`download`=强制整本；`stream`=直链 Range 流式（不支持则整本）。

## 4. 集成方案对比

| 方案 | 工作量 | 覆盖 | 说明 |
|---|---|---|---|
| A. 原生 Alist/OpenList API 书源（推荐） | 中（≈ SFTP 任务量） | 40+ 网盘一次覆盖 | 镜像 WebDAV/SFTP 模式：`alist_connect/list/open/cover` + raw 缓存；类型 `type='alist'` |
| B. 文档化"自建 OpenList + WebDAV 接入" | 零代码 | 同上 | 现有 WebDAV 书源即可，先给用户立即可用方案 |
| C. 直连单网盘官方 API（阿里云盘开放平台等） | 大/每网盘 | 单个 | OAuth/令牌刷新/限速/维护成本高，仅在用户无自建网关时再评估 |
| D. 打包/内置 OpenList 进程 | 大 | 40+ | 把网关打进 RCH 安装包并自动拉起，重（进程管理/升级/体积），后续可选 |

## 5. 建议范围（若立项）

- **R0（随文档）**：README/SETUP 增加「网盘接入」小节：自建 OpenList/AList → 用现有 WebDAV 书源。
- **R1（主功能）**：书源类型 `alist`（兼容 AList + OpenList）：
  - Rust：`source/alist.rs`（HTTP 客户端：list/get raw_url/download）、会话表 + `api/source.rs` 的 `alist_connect/disconnect/list/open_sftp…` 等价物（`alist_*`）、raw/ 缓存命名空间 `alist|{base}|{path}`。
  - Dart：BookSource 类型 + 添加书源对话框第 5 段「网盘(Alist)」+ 分发点接线（复用 needsSession 体系）。
  - 打开策略：与 WebDAV/SFTP 共用 `bookOpenStrategy`。
- **R2（暂缓）**：直连单网盘官方 API、内置 OpenList。

## 6. 开放问题

- **O1** token 获取方式：用户粘贴管理后台令牌（简单，推荐）vs 应用内账号密码登录换取（体验好但多两个端点）。
- **O2** 分页：漫画目录可能超 200 条；`per_page` 拉满 500 + 循环翻页，或一次性拉取（推荐循环翻页直到 `has_more=false`，目录多时避免一次性大响应）。
- **O3** raw_url 的 Range 探测：连接时对根目录文件探测一次，缓存能力标记（沿用 WebDAV capability 模式）。
- **O4** OpenList 桌面版是否值得在文档里作为一键方案推荐（Windows 用户友好）。

## 7. 相关链接

- OpenList: https://github.com/OpenListTeam/OpenList （README 中文：https://raw.githubusercontent.com/OpenListTeam/OpenList/main/README.md）
- OpenList 桌面版: https://github.com/OpenListTeam/openlist-desktop
- AList API 文档（fs）: https://main--alist-doc.netlify.app/guide/api/fs.html
- AList 部署: https://alistgo.com
