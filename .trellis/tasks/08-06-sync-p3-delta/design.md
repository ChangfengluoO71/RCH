# P3 设计 — 增量同步（LWW + 墓碑合并 + 幽灵书源）

## 1. 合并语义

- **拉取 = merge（LWW）**：逐行比较 `updated_at`，新者胜；同值保留本地；行不存在则插入。
- **恢复/导入 = force**：包覆盖本地（书源凭据仍 COALESCE 保留本地值）。
- **墓碑机制（本阶段决策）**：新增 `sync_tombstones(entity, key, updated_at)` 表；本地删除路径硬删除的同时写墓碑；导出把墓碑并入 `tombstones.json`；合并应用墓碑 = 硬删除本地行。实体行 `deleted` 列保留为 P0 兼容，P3 不依赖它做传播。
- 标签重命名 = 删旧 + 建新（rename_tag 硬删除 + 墓碑）。

## 2. 跨设备书标识匹配

- 源匹配：包内书源按 `fingerprint` 与本地 `book_sources.fingerprint` 匹配：
  - 命中 → key 前缀重写（`type|pkgSourceId|path` → `type|localSourceId|path`），metas / records / book_tags 按重写后 key 合并——同 fingerprint + 同路径 = 同一本书。
  - 未命中且类型为 local/smb → 创建**幽灵书源**（`remote_only=1`，`origin_device_id=包 manifest.device_id`，无凭据），保留原 key（元数据键天然一致）。
  - 未命中且类型为远程 → 正常创建（待填凭据）。
- stable_id 双轨：两边行 stable_id 均非空且相等时也视为同一本（内容 hash 计算留待后续阶段补齐）。

## 3. 设备注册与名称

- 导入时用 manifest 的 `device_id` / `device_name` 写入 `devices` 表。
- Dart 通过新 FRB `db_list_devices` 反查幽灵书源的来源设备名。

## 4. 幽灵书源 UI

- `BookSource` 模型/DTO 增加 `remoteOnly` / `originDeviceId`。
- 详情页：不可阅读（占位提示）、封面占位、元数据（标签/简介/感想/标题等）可编辑并随同步回传；隐藏自定义封面入口（需要源文件）。
- 全局设置开关 `sync_cross_device_search`（默认开）：`globalSearch()` 关时过滤幽灵书源，开时包含并标注"仅元数据 · 来自设备X"，点击进入详情编辑而非阅读。

## 5. FRB/API 变更

- `rchpkg_import(path, force)`：force=false 走 merge，true 走恢复。
- `BookSourceDto` 增 `remote_only` / `origin_device_id`；新增 `db_list_devices`。

## 6. 导出变更

- `tombstones.json` = 实体 deleted=1 行 + `sync_tombstones` 增量行。
