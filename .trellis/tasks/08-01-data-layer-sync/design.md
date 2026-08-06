# 数据层同步 — 技术设计（共享基座）

本设计供 data-layer-sync 父任务及其子任务（P0-P3）共用，子任务细化各自阶段的设计。

## 1. 核心概念

- **本地优先（local-first）**：SQLite 是唯一本地真源；标准包只是交换介质，任何设备断网都能继续读写。
- **稳定书 ID（stable_id）**：默认 = 文件内容 hash（sha256，`sha2` crate 已引入）；可选显式 UUID 覆盖（写入 ComicInfo.xml 时启用）。路径、书源 ID 均降级为别名，跨设备靠 stable_id 命中同一本书。
- **书源 fingerprint**：远程书源 = sha256(规范化 type + `|` + url + `|` + username + `|` + 根路径)；本地书源 = sha256(type + `|` + 根目录名，如 `Comics`)，保证同一逻辑书库跨设备稳定。本地 source_id 通过 `source_alias` 映射到 fingerprint，存量 key 不动。
- **冲突语义**：按实体 LWW（updated_at 毫秒，最新者胜）+ 墓碑（deleted=1 + updated_at）。标签重命名 = 删旧名 + 建新名，墓碑保证删除跨设备传播。
- **凭据策略（已决策）**：标准包永不携带 password / refresh_token / client_secret / cookie；保留 username / root_id / client_id。目标端导入后书源为"待填凭据"状态。
- **幽灵书源（已决策）**：其他设备的本地书源在目标端为 `remote_only` 条目，可编辑元数据并回传，不可连接/阅读，封面占位；跨设备搜索开关控制 `globalSearch()` 是否包含。

## 2. 数据模型扩展（P0 落地）

六张同步实体表增加同步列，三张新表支撑设备/游标/别名：

| 表 | 新增列 | 说明 |
|---|---|---|
| `book_sources` | fingerprint / remote_only / origin_device_id / updated_at / deleted | fingerprint 跨端稳定标识 |
| `read_records` | stable_id / updated_at / deleted | 阅读进度 LWW |
| `book_metas` | stable_id / updated_at / deleted | 漫画详情 LWW |
| `tags` | updated_at / deleted | 标签墓碑 |
| `book_tags` | updated_at / deleted | 关联墓碑（查询过滤 deleted=0） |
| `app_settings` | updated_at / deleted | 设置白名单同步 |
| `devices`（新） | id / name / created_at / last_seen_at | 设备注册表（含 self） |
| `sync_state`（新） | key / value / updated_at | device_id、同步游标、传输配置 |
| `source_alias`（新） | source_id / fingerprint / device_id / updated_at | 本地 source_id ↔ fingerprint 映射 |

迁移策略：`init_tables` 中 CREATE TABLE IF NOT EXISTS 包含新列（新库直接建齐），老库通过 PRAGMA table_info + ALTER TABLE ADD COLUMN 幂等补列（沿用现有 rotations/port 升级模式）。

## 3. Upsert 语义（P0 落地）

现状 `INSERT OR REPLACE` 会重置未在插入列中的新列（fingerprint/stable_id/deleted 被清空），因此改为：

```sql
INSERT INTO ... (原列..., updated_at) VALUES (...)
ON CONFLICT(<pk>) DO UPDATE SET 原列=excluded.原列, ..., updated_at=excluded.updated_at
```

新列（fingerprint/stable_id/remote_only/origin_device_id/deleted）不被 DO UPDATE 触碰，实现"保留"；updated_at 每次写入刷新为当前毫秒时间戳，为 LWW 提供数据。

## 4. 标准包格式（P1 细化）

- 单文件 `*.rchpkg`（zip）：`manifest.json`（schema_version / device_id / created_at / 实体块清单）+ 按实体分块 JSON（tags / book_tags / metas / records / sources / settings）+ 墓碑块 + 敏感字段剔除。
- 增量：每设备同步游标（sync_state 中 `cursor_{device}`），导出自上次游标以来的变更。
- 目录约定：`RCH/sync/latest.rchpkg` + `RCH/sync/archive/*.rchpkg`。

## 5. 合并引擎（P3 细化）

- 拉取远端包 → 按实体 LWW 合并 → 墓碑应用（deleted=1 项删除本地行或写墓碑）→ stable_id 重映射 book_tags/book_metas/read_records → fingerprint 匹配书源（新 fingerprint 创建幽灵条目或 source_alias）。
- 合并时书源凭据列永不从包写入；目标端已有凭据不被覆盖。

## 6. 传输模式（P2 细化）

- 模式 A：主动 WebDAV（复用 webdav_session / 已有基建），手动/定时 push/pull。
- 模式 B：网盘同步盘目录，应用向本地同步文件夹原子写包（临时文件 + rename），启动/定时/文件变化扫描读取；识别网盘客户端冲突副本（`xxx (冲突副本)` / `xxx(1)`）。

## 7. 阶段拆分

P0 schema 落库 → P1 标准包格式 → P2 备份即同步（双通道）→ P3 增量合并。P4（服务端/CRDT）默认不做。
