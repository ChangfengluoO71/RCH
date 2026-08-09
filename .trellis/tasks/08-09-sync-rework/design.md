# 设计：Sync State + Three-Way Merge（ADR-024/025）

## 1. 总体架构

```
                        RCH
                         │
                         ▼
                      SQLite（本机业务状态中心）
                         │
          ┌──────────────┼────────────────┐
          │              │                │
     Business Data   Sync Base       Source Snapshot
          │              │                │
          └──────────────┴────────────────┘
                         │
                         ▼
                  Sync Engine（Rust domain）
                    Pull / Push
                         │
                         ▼
                   Three-Way Merge
                         │
                    Sync Plan（可诊断）
                         │
                         ▼
        WebDAV Sync State（manifest + state/<rev>/ + devices/）
```

原则：SQLite 是本机状态中心；WebDAV 是同步存储层（普通文件，不是数据库）；Sync Base 是三方比较基线；身份稳定不依赖本机 ID；library_index 与 metadata 分离；语义合并优先于粗暴 LWW；自动同步无感但决策可诊断；rchpkg 是备份不是实时同步。

## 2. WebDAV Sync State 协议（ADR-024）

### 目录结构

```text
<sync_dir>/                      # 默认 RCH/sync
├── manifest.json                # 提交点：schema_version / library_id / revision / updated_at / files{entity: file, hash}
├── state/
│   ├── sources-<rev>.json
│   ├── metadata-<rev>.jsonl
│   ├── tags-<rev>.jsonl
│   ├── book_tags-<rev>.jsonl
│   ├── records-<rev>.jsonl
│   ├── library_index-<rev>.jsonl
│   └── settings-<rev>.json
└── devices/
    └── <device_id>.json         # device_id / device_name / last_seen_at
```

- **manifest 是唯一提交点**：只有 manifest 成功更新（tmp+MOVE）才算一次同步完成；读取端只信任 manifest 引用的文件。
- **版本化文件**：每次写新 revision 的实体文件（`-<rev>` 后缀），绝不覆盖在写文件；manifest 更新后旧版本文件可修剪（保留最近 N=3 版，异常恢复用）。
- **library_id**：随机 UUID，首次初始化写入 manifest；防止不同同步库误合并（本地 sync_state 也保存）。
- **schema_version**：同步协议版本独立于 rchpkg schema 版本；不识别的新版本安全拒绝（提示升级），不静默覆盖。
- **revision**：单调递增整数（manifest 写入时 `prev+1`）；CAS 冲突检测依据。

### 写协议（原子性 + CAS）

```text
Push(device):
  loop:
    remote = read manifest + state(revision R)
    local  = read local business data
    base   = read sync_base（上次成功时 R 的状态）
    plan, merged = three_way_merge(base, local, remote)
    if plan 无变化 → return
    写 state/<entity>-(R+1).*  （tmp → MOVE 到最终名）
    写 manifest(R+1, files=新文件引用)（tmp → MOVE）
    若 manifest 写入前 revision 已 != R → 重试（重新读远端，并入本轮结果）
    成功后：本地应用 merged → 推进 sync_base
```

### 读协议

```text
Pull(device):
  manifest → 检查 schema/library_id/revision
  revision 与本地 sync_base.revision 相同 → 无变化
  否则读取 manifest 引用的各实体文件 → three_way_merge(base, local, remote)
  → 本地应用 merged（事务）→ 推进 sync_base
```

## 3. Sync Base（SQLite）

```sql
CREATE TABLE sync_base (
    entity_type TEXT NOT NULL,      -- sources|metas|tags|book_tags|records|library_index|settings
    entity_key  TEXT NOT NULL,      -- 同步层稳定身份（fingerprint+path / tag id / ...）
    state_hash  TEXT NOT NULL,      -- 上次成功时远端条目 hash
    state_json  TEXT,               -- 字段级合并实体存完整 JSON（metas/tags/records/sources/settings）；library_index 仅 hash
    revision    INTEGER NOT NULL,   -- 推进时的远端 revision
    updated_at  INTEGER NOT NULL,
    PRIMARY KEY (entity_type, entity_key)
);
CREATE TABLE sync_meta (           -- 全局同步元数据
    key TEXT PRIMARY KEY,           -- library_id / last_revision / last_sync_at / last_error
    value TEXT NOT NULL
);
```

- 推进时机：本地应用 merged 且远端 manifest 写入成功后，同事务更新 sync_base + sync_meta。
- 失败语义：任何一步失败 → sync_base 不推进；下次重试重新比较。
- library_index 条目只存 state_hash（体积控制），合并策略为单端胜/LWW，不需要字段级 base。

## 4. 稳定身份

- `source_fingerprint = sha256(type + "://" + normalized_endpoint + "/" + root_identifier)`（已实现，`db::compute_source_fingerprint`）。
- **同步层 book_id** = `sha256(source_fingerprint + "|" + normalized_path)`（复用 `library_index_id` 规则）。
- 远端状态所有实体 key 使用同步层稳定身份；本地 SQLite key 保持 `type|source_id|path`，由 Sync Engine 映射层转换：
  `remote_key ↔ (fingerprint → 本地 source_id) → 本地 key`。映射基于 `book_sources.fingerprint`，重命名/重建源不影响。
- 标签身份 = 标签名（现有 Tag id 规范）或独立 tag id——保持现状（id 为名字），远端以 name 为 key。

## 5. 三方合并策略（ADR-024）

每个实体：`local == base && remote != base → remote wins`；反之 local wins；双端都改 → 按实体策略：

| 实体 | 策略 |
| --- | --- |
| sources | LWW（updated_at，等值 tie-break by key）；同 fingerprint 视为同一源（名称/备注可合并） |
| book_metas | **字段级**：逐字段 local/remote 相对 base 三态；仅同字段双改才冲突（确定性 LWW） |
| tags | 新增并集；删除 = 墓碑（revision 标记），删除优先于旧新增 |
| book_tags | 关系级 add/remove + 墓碑；避免"删 vs 改"幽灵关系 |
| read_records | LWW（last_read_at/updated_at）+ 墓碑；等值确定性 tie-break |
| library_index | 单端变 → 接受；双端变 → 确定性 LWW（updated_at，等值 by path）；删除墓碑；源删除级联 |
| settings | 白名单 + LWW per key |

### Sync Plan（可诊断）

```json
{ "entity": "metas", "key": "fp|/books/a.cbz",
  "base": {"read": false, "tags": []},
  "local": {"read": true, "tags": ["收藏"]},
  "remote": {"read": false, "tags": ["神作"]},
  "decisions": [{"field": "read", "winner": "local"}, {"field": "tags", "action": "merge"}],
  "result": {"read": true, "tags": ["收藏", "神作"]} }
```

Plan 对象通过 FRB 暴露（debug/诊断页或日志），不参与正常 UI 流程。

## 6. Rust 模块布局（新增 `app/rust/src/sync/`）

```text
sync/
├── mod.rs          # 对外编排：sync_pull / sync_push / sync_now / status / plan
├── identity.rs     # fingerprint + book_id + 路径规范化（收敛 db 内现有实现）
├── base.rs         # sync_base / sync_meta CRUD
├── state.rs        # 远端状态模型 + manifest + 序列化（JSON/JSONL）+ hash/revision
├── merge.rs        # 三方合并引擎（实体策略 + SyncPlan）
├── webdav.rs       # 状态文件传输（复用 source::webdav 客户端；tmp+MOVE；CAS 循环）
└── worker.rs       # 变更检测/防抖由 Dart 侧驱动，Rust 提供原子 sync 操作
```

## 7. Dart 侧

- `SyncEngine`（替代/重写 `SyncManager` 同步部分）：生命周期钩子（启动 pull、前台 pull、网络恢复 pull、退出兜底）、变更防抖（1~3s）→ `sync_now`、定时轮询（30~60s，ETag/revision 判断）、手动同步按钮。
- 变更检测：本地写入路径（repository/store）通知 `SyncEngine.markDirty(entityType, key)`；防抖合并为一批。
- UI：设置页 SyncPanel 保留 WebDAV 配置 + 测试连接 + 手动"立即同步"+ 自动开关 + 设备名/ID；同步状态/时间/错误展示。
- 书源列表分区：普通书源区（本机 + fingerprint 命中同源）+ 设备折叠区（未命中）；SourceBrowser 离线索引模式（library_index 数据源）＋三状态标识。

## 8. rchpkg 降级为备份（ADR-025）

- 保留：rchpkg v2 布局、导出/导入/恢复、v1 兼容、可选加密凭据、JSONL。
- 删除：rchpkg 作为同步传输的路径（sync_manager 的 push/pull/归档/WebDAV 包上传）、cursor 增量导出（备份一律全量快照）。
- UI 入口：设置页"备份/恢复"（导出 .rchpkg / 导入 .rchpkg），与"同步"完全分离；删除 rchbundle。

## 9. 删除清单（旧同步实现）

1. `sync_manager.dart`：`_pushFolder/_pullFolder/_pushWebdav/_pullWebdav/_cleanFolderArchives/_cleanWebdavArchives`、`SyncMode.folder/webdav`、WebDAV 包上传路径、归档逻辑。
2. `rchpkg` 中同步专用增量游标路径（`cursor_export` 不再用于日常同步；备份导出为全量）。
3. rchbundle：`home_page.dart:333/335`、`SourceBundleDto`、`encrypt_source_bundle/decrypt_source_bundle` 的同步用途（备份可保留加密凭据）。
4. 依赖单一 `cursor_export` 的同步模型；`load_source_credentials` 仅保留备份导出用。
5. UI 隐藏 remote-only 的逻辑（`home_page.dart:341` 的过滤）。

## 10. 数据迁移路径

1. fingerprint 回填（已完成，幂等）。
2. `sync_base`/`sync_meta` 新表：`init_tables` 建表；首次同步前为空（base 不存在 = 全量 local-only）。
3. 首次同步：本地全量作为 merged 推远端，写入 base=revision 1（不做三方合并，等价"初始化"）。
4. 老设备升级：无 base → 与远端第一次交互按"初始化"处理；已有远端状态的设备按三方合并接管。
5. rchpkg 备份不受影响（独立格式版本）。

## 11. 风险与缓解

- **并发写**：CAS 循环 + manifest 提交点；极端双写窗口最后写者胜（记录在案）。
- **多文件一致性**：版本化文件 + manifest 引用；修剪保留 N 版。
- **大 library_index**：JSONL 流式；base 仅存 hash；单实体增量上传。
- **115/WebDAV 限流**：目录枚举已有节流；状态同步走 WebDAV（低频小文件），定时轮询用 ETag。
- **身份规范化漂移**：fingerprint 单测覆盖（大小写/尾斜杠/query/userinfo），规范化规则沉淀 spec。
- **字段级合并复杂度**：先 metas/tags/records，再 sources/library_index；每实体独立测试矩阵。

## 12. ADR-028 修复设计（2026-08-09，双设备实测触发数据清空后）

> 实测（坚果云 WebDAV，Windows + MuMu Android）：远端 state 在“有内容 ↔ 空文件”间
> 反复横跳，30 分钟内产生 22 个 revision；最终全部实体被墓碑清空。根因三类：
> Base 语义退化为 change log、未变化实体被写成空文件、apply 静默跳过导致“快照缺 key”
> 被误判为“本地删除”。本 ADR 修复这三类根因，并做身份/序列化稳定性加固。

### 12.1 Base = 上次提交成功后的全量镜像（修正语义）

- `advance_base` 的输入必须是**完整提交状态**（merged 变化 ∪ base 中未变化 key），
  不能只收变化条目；未变化 key 必须保留，禁止按 merged 差集 prune。
- base 中保留墓碑条目（deleted=true），保证“从未存在”与“已删除”可区分，
  这是三方合并不再伪造删除的前提。
- 墓碑 GC 后续与 library_index 墓碑 GC 对齐（>30 天清理 base 墓碑行），本轮不做。

### 12.2 manifest 全量引用协议（禁止空文件）

- `manifest.files` = 每实体的**最新全量文件引用**（版本化 blob，类似 git）：
  本轮变化的实体写新文件并更新引用；未变化的实体**沿用旧文件引用**，
  不写任何文件、不出现空文件。
- 拉取端：manifest 未引用的实体 = 本轮未提交，**沿用 base**（绝不能理解为“远端为空”）。
- 实体真正清空 = 文件内含墓碑条目（deleted=true），不是空文件。
- 拉取端防御：**被引用但为空的 state 文件 = 旧版残留/损坏，直接报错**，
  禁止按“合法空状态”合并（否则旧版留下的空文件仍会触发全量墓碑灾难）。
- 修剪保护：`prune_targets` 不得删除当前 manifest.files 仍引用的文件。
- 清理历史遗留 `manifest.json.tmp`。

### 12.3 apply 禁止静默跳过（sync_pending_apply）

- 新增 `sync_pending_apply(entity_type, entity_key, reason, payload, created_at, updated_at)`。
- metas/records/library_index/book_tags 的 live 条目 resolve 失败时写入 pending
  （payload = 完整 SyncEntry JSON），绝不 `continue`。
- 本地快照加载时把 pending 条目视为“存在”（live），使 merge 不产生伪墓碑。
- 新源加入/更新后可解析时，`reapply_pending` 落真实表并清除 pending。
- 墓碑条目先清除对应 pending 行，再执行删除。
- UI“等待绑定”展示为 P1，本轮只保证数据不丢失、不误删。

### 12.4 身份与序列化稳定性

- `book_id` 的 path 输入统一规范化（`\`→`/`、盘符小写、去尾斜杠，根 `/` 保留），
  Rust `db::library_index_id` 与 Dart `LibraryIndexService.libraryIndexId` 必须一致。
- `remoteOnly/originDeviceId` 不再进入 sources 同步负载；apply 不覆盖本机标志：
  - 新 fingerprint → 插入为 remote_only=true + origin=writer（manifest 透传）；
  - 已存在（含本机源）→ 只更新配置字段，remote_only/origin 保持不变。
  消除设备间标志翻转导致的 LWW 重推抖动。

### 12.6 初始化/自愈与轻量轮询

- **远端无 manifest = 初始化/远端被清空自愈**：`prepare_sync` 在 remote=None 时
  以**本地全量**作为提交状态，而不是三方 diff——否则 base 已建立时 diff 为空集，
  会推一个空 manifest 导致远端数据永久丢失。
- **轻量轮询**：定时轮询先只读远端 manifest revision（`sync_remote_revision`），
  与本机 last_revision 一致则跳过全量同步；失败退避期间轮询/防抖让位给退避定时器，
  杜绝“每 60 秒全量下载 + 盲重试 → 坚果云 503”的循环。

### 12.5 UI 与数据修复

- `LibraryStore.load({bool force = false})`，同步成功后 `load(force: true)` +
  `loadTree()` + notify；`SourceAvailabilityDto` 增加 `path`，兜底不再用 `/`。
- `refreshSourceIndex` 的 rootHash 短路增加 live 行数比对，墓碑软删后可重扫恢复。
- 数据修复：远端 rev22 已全空、本机 base 停在 rev21，下一次成功同步会清空剩余数据；
  先关自动同步、备份 DB，代码修复后重置同步（换 library_id / 清远端目录）全量重推。

### 12.7 离线索引自动化（ADR-029，2026-08-09）

> 用户反馈：手动"生成离线索引"（全量爬云端树）复杂化；缓存/已读/标签过的书
> （含未读但被批量标签的）应自动出现在离线索引，且生成索引不应发网络请求。

- **触及即补**：读（recordRead）、缓存、打标签（batchTag）成功时，自动补写该漫画的
  `library_index` file 条目 + 父目录链（`ensure_index_entry_on`，幂等 upsert，零网络）。
  - 未读但被标签的书同样覆盖（标签即"有用信息"信号）。
  - 补的条目随同步传播，其他设备也能看到"被触及的书"的路径条目。
- **浏览即索引**：在线浏览云端源目录成功时，把当前目录直接子项写入索引
  （复用同一次列表响应，不新增请求）；用户浏览到哪里，离线索引积累到哪里。
- **生成离线索引本地化**：默认按钮只从本地浏览快照（FolderSnapshotStore）构建，
  零网络；全量爬云端树改为菜单里的高级选项"全量重建索引（联网）"。
- **同步前**：SyncEngine 对云端源执行 `buildIndexFromSnapshots`（零网络），
  把本地浏览快照补的索引推给其他设备。
- 实现：`db::ensure_index_entry_on`（Rust，父链补到书源根路径为止，根目录不入库）+ FRB
  `dbEnsureIndexEntry` / `dbEnsureIndexEntries`；Dart 接线点：
  `LibraryStore.recordRead` / `batchTag` / `SourceBrowser._list` / `SyncEngine.syncNow`。
- **层级修正（实测补充）**：夸克/115 的 path 是**扁平 fid**（无层级前缀），
  从条目 path 推导父子会把子目录文件错误挂到根下。修复：
  `ensure_index_entry_on` 增加显式 `parent_path`（浏览即索引传当前浏览目录；
  触及即补通过 `FolderSnapshotStore.parentDirOf` 反查父目录）；
  父链条目用保留 name/path 的 upsert（不覆盖浏览写入的中文目录名）。
  已有错误层级数据可通过"生成离线索引（本地快照）"一键重写修正。
- **层级修正二（多级目录拍平）**：浏览深层目录时，父链补全对扁平路径推导不出
  target 的真实上级，会默认挂根——若允许覆盖，会把"浏览 B 时写入的 B.parent=A"
  错误重置为根，多级目录被逐层拍平（顶层无漫画的纯目录链最容易暴露）。
  修复：父链 upsert 改为**只创建缺失条目**（已存在条目保留 parent/name/path，
  仅复活软删与更新时间）；正确层级始终由"条目本身"写入，父链只兜底。
  新增回归测试 `parent_chain_never_resets_existing_hierarchy`。
