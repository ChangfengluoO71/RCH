# 数据层同步就绪改造 — 本地优先 + WebDAV/云盘标准包

## Goal

把数据层设计成"同步就绪"：SQLite 唯一本地真源 + 版本化标准包交换格式 + 本地优先（local-first）增量同步。先落地 WebDAV/云盘备份（备份即同步），后续按阶段扩展到多端自动同步，为标签特色提供跨端一致的基座。**本任务为规划任务，默认不实施；每个阶段开工前单独确认并拆子任务。**

本 PRD 明确覆盖三大高价值实体：**标签系统、书源配置、漫画详细信息**（阅读记录、可同步设置一并纳入），并规划两种用户自有传输通道：主动 WebDAV 与网盘同步盘目录（被动双向同步）。个人开发、无服务器，同步完全走用户自有存储。

## Background（背景与已确认决策）

- 现状混乱根源：JSON（早期方式）与 SQLite（后来引入）双写双读，两个数据源互相缺数据。长期方向是把 JSON 降级为"只导出不读"，SQLite 成为唯一真源。
- 标签是应用特色：标签按名字做 ID（`name.trim().toLowerCase()`，Dart/Rust 已对齐），跨端天然稳定，同步难度低。
- 同步真正的难点：**书标识**（当前 key = `来源类型|来源ID|路径`，本地书跨设备路径必然不同，需内容级稳定 ID，路径降级为别名）和**删除/冲突**（需 updated_at + 墓碑）。
- 已确认方向：**本地优先（local-first）+ WebDAV/云盘标准包**。冲突按实体 LWW（updated_at 最新者胜）+ 墓碑；标签重命名 = 删除旧名 + 新建新名（墓碑保证传播）。
- 前置依赖：`08-01-fix-tag-persistence` 先落地真源收敛 + 一次性对账，本任务在其基础上做 schema/格式/同步层，不重复改持久化链路。
- 已确认决策（2026-08-06）：**书源凭据不同步（O-CRED = b）**——标准包永不携带敏感凭据，目标端拿到书源配置后重新填账号密码/令牌。
- 已确认决策（2026-08-06）：**远端本地书源（O-LOCAL）**——其他设备的本地（local/SMB）书源同步到目标端后以"远端书源"条目存在，仅显示与编辑元数据、不可打开阅读；跨书源全局搜索可配置是否跨设备包含这些条目。

## 已确认事实（代码实证，2026-08-06）

- 持久化：SQLite 为唯一本地真源（`app/rust/src/db/mod.rs`）；`library.json` 仅作一次性迁移与备份（`app/lib/main.dart`、`library_store.dart`）。
- 表结构：`book_sources`、`book_metas`、`read_records`、`tags`、`book_tags`、`app_settings`、`source_capability`、`cache_index`、`ai_tasks`、`schema_version`。
- 书源 ID 不稳定：新增书源时 `id = '{type}_{DateTime.now().millisecondsSinceEpoch}'`（`home_page.dart`），同一逻辑书源在跨设备必然不同 ID，导致所有以 sourceId 拼装的 key 跨设备失效。
- 书 key = `type|sourceId|normalizeComicPath(path)`（`models.dart bookKeyOf`）；本地书源 path 跨设备必然不同。
- 漫画详细信息全部为标量/JSON：`coverPage`、`cropX/Y/W/H`、`author`、`genre`、`series`、`title`、`chineseTitle`、`summary`、`comment`、`rotations`（`book_metas` + `models.dart BookMeta`）。自定义封面 = 页码 + 相对裁剪框（`cover_editor_page.dart`），**无二进制资产**，天然可同步。
- 书源含敏感凭据：`password`、`refresh_token`、`client_id`、`client_secret`、`root_id`、`cookie`（`book_sources` 表 + `models.dart BookSource`），同步必须决策凭据策略。
- `source_capability` / `capabilityLabel` 为探测得到的派生状态，不应同步；`ai_tasks` 为设备本地任务队列，不应同步。
- 设置中 `cacheDir`（自定义缓存目录）设备相关，不应同步；其余阅读/界面偏好可同步。
- 标签 id 已归一化为基于名称的稳定 ID（`tag_repository.dart _normalizeTagIds`），跨端稳定。

## 同步范围矩阵

| 实体 | 是否同步 | 跨端稳定性现状 | 同步要点 |
|---|---|---|---|
| `tags` | ✅ | id=名称稳定 | 增删改 + 墓碑；重命名 = 删旧名 + 建新名 |
| `book_tags` | ✅ | 依赖 book key | 随稳定书 ID 重映射（O-BI） |
| `book_metas`（漫画详细信息） | ✅ | key 依赖 sourceId+path | 全字段标量；稳定书 ID 关联 |
| `read_records`（阅读进度） | ✅ | key 同上 | LWW + 时钟（last_read_at） |
| `book_sources`（书源） | ✅ | id 设备本地 | fingerprint 稳定化 + 凭据不同步（已决策）+ 远端本地书源幽灵条目（已决策） |
| `app_settings` | ⚠️ 白名单 | — | 排除 cacheDir 等设备相关项 |
| `source_capability` / `capabilityLabel` | ❌ | — | 目标端重新探测 |
| `cache_index` / `ai_tasks` | ❌ | — | 本地缓存/任务，不同步 |

## Requirements（按阶段规划，非一次性实施）

- **P0 同步元数据落库**（依赖 fix-tag-persistence 完成后）：tags / book_tags / records / metas / settings 增加 `updated_at`、`deleted`（墓碑）；新增 `device_id` 与同步游标表；书表增加稳定 ID 列（内容 hash + 可选 UUID 覆盖，O-BI 默认方案）。
- **P0 书源同步列**：`book_sources` 增加 `fingerprint`（跨端稳定标识）与 `updated_at`、`deleted`；新增设备本地映射表 `source_alias(source_id, fingerprint)`，现有 id 保持不动、避免破坏存量 key。
- **P0 幽灵书源列**：`book_sources` 增加 `remote_only`（仅远端显示）与 `origin_device_id`（来源设备）；远端本地书源在目标端不可连接、不可阅读、不渲染封面。
- **远端本地书源（O-LOCAL 已决策）**：其他设备同步过来的本地（local/SMB）书源在目标端以"远端书源"条目存在；用户可查看/编辑标签、简介、感想、标题、作者、类别、系列等全部元数据（编辑随同步回传），但无法打开阅读；封面图片不传输不渲染（显示占位），`coverPage`/`crop` 字段仍随包同步以保往返无损。
- **跨设备搜索设置（新增需求）**：全局设置新增开关"跨书源搜索是否包含其他设备的远端本地书源"（默认建议开启）；开启后现有 `globalSearch()`（`library_store.dart`）命中远端本地书源时，结果标注设备来源与"仅元数据、不可阅读"状态，仍可进入详情页编辑信息。
- **P1 标准包格式**：版本化交换格式（manifest + 按实体分块的增量 + 墓碑 + schema 版本），本地备份、云备份、设备间同步共用；替代当前 library.json 的读写角色；导出/导入往返无损（round-trip）。分块按实体：tags / book_tags / metas / records / sources / settings；**sources 分块剔除全部敏感凭据**（password / refresh_token / client_secret / cookie，保留 username / root_id / client_id 等非敏感字段）。
- **P2 备份即同步（阶段 A）— 双传输模式**：共用同一标准包格式与合并引擎，仅传输层不同。
  - **模式 A 主动 WebDAV**：应用直连用户 WebDAV（如坚果云，已有基建），上传/下载/恢复标准包；支持手动/定时触发。
  - **模式 B 网盘同步盘目录**：用户把本地目录挂到网盘客户端（OneDrive/坚果云/百度同步空间等）实现双向同步；应用向该目录写标准包，并在启动/定时/文件变化时读取远端包。需"选择同步目录"设置；写包采用原子写（先写临时文件再 rename）避免与网盘客户端上传冲突；识别并忽略/择优网盘客户端产生的冲突副本（如 `xxx (冲突副本)` / `xxx(1)`）；目录约定与模式 A 共用（O-CLOUD）。
- **P3 增量同步（阶段 B）**：设备 A 推自上次游标以来的 delta，设备 B 拉取并按 LWW + 墓碑合并；需要处理多设备时钟与离线编辑。书源合并不覆盖目标端已存在的本地凭据、不复活已删书源；导入后未填凭据的书源进入"待填凭据"状态并在连接时给出引导；远端本地书源编辑回传不产生目标端阅读记录；漫画详情/标签按稳定书 ID 合并，跨设备匹配不依赖路径。
- **P4 可选扩展（阶段 C）**：自建/第三方后端 + 账号体系 + 实时推送；仅当出现多端同时离线编辑需求再评估 CRDT（Automerge/Yjs 等）。

## Acceptance Criteria（规划任务不执行，以下为各阶段开工时的验收锚点）

- [ ] P0：SQLite 表含同步元数据（含 `book_sources.fingerprint` / `source_alias`）且现有读写不受影响（回归通过）
- [ ] P1：标准包导出→导入后数据与标签关联无损；标准包不含任何敏感凭据，导入后书源为"待填凭据"状态；schema 版本不兼容时可拒绝/提示
- [ ] P2：WebDAV 与同步盘目录两模式上传/下载/恢复全链路可用；失败可观测、可重试；冲突副本不导致数据损坏
- [ ] P3：两端各自离线编辑后合并不丢标签/漫画详情/书源、不复活已删项（墓碑生效）；其他设备本地书源以"仅元数据"条目同步显示、可编辑不可打开；跨设备搜索开关生效且结果标注设备来源
- [ ] P4：仅在需求确认后评估，默认不进入范围

## 已定决策与默认方案（开工前可微调，非阻塞）

- **O-BI 书稳定 ID**：默认内容 hash（无侵入、同内容同 ID）+ 可选显式 UUID 覆盖（需写回文件/ComicInfo.xml 时启用）；路径降级为别名。
- **O-CLOUD 目录约定**：默认 `RCH/sync/latest.rchpkg` + 时间戳归档（`RCH/sync/archive/*.rchpkg`）。
- **O-UI 入口**：默认设置页"备份/同步"分组，含传输模式选择、同步目录选择、跨设备搜索开关与最近同步状态。

## Out of Scope

- 本任务的 P0-P4 实施（仅登记规划，开工前拆分）
- `08-01-fix-tag-persistence` 的修复工作（另任务）
- 服务端实现（阶段 C 前无服务端）
- 漫画/图片二进制的增量同步（书库文件由用户书源/网盘自行同步，应用只同步元数据与配置）
