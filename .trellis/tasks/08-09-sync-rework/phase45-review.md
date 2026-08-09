# Phase 4.5 协议稳定性审查（2026-08-09）

> 范围：Phase 1–4 已实现的同步骨架（sync/ 模块 + FRB + Dart SyncEngine）逐环审查。
> 目的：进入 Phase 5（library_index 大规模实现）前消除正确性/稳定性阻塞项。

## 结论摘要

同步协议整体成立：三方合并 + Sync Base + 版本化状态文件 + manifest 提交点 + CAS 的骨架正确，
单元测试覆盖了主要合并矩阵。审查发现 **3 个必须修复项（已修复）**、**5 个应在 Phase 5/6 前置处理项**、
**4 个记录在案的已知限制**。

## P0 — 必须修复（本轮已修复）

### 1. 并发覆盖导致单端改动丢失（TOCTOU）✅ 已修复
- **问题**：`upload_state` 的 CAS 是"读 manifest 后写"，检查与 MOVE 之间存在窗口。
  双端几乎同时 push 时，后写者可能覆盖先写者的 manifest，而**先写者随后 advance_base 成功**；
  下一轮三方比较中"先写者"会因 base 与远端不一致被远端胜出，把本地独有改动洗掉。
- **修复**：`sync_with_webdav` 在 `upload_state` 成功后**回读 manifest 校验 revision/library_id**；
  不匹配则不推进 base、进入重试循环重新合并（本地改动保留在 merged 中，重并后推回）。
- **验证**：逻辑推演双写场景收敛（A 写 rev2、B 写 rev3 → A 校验失败重并 → rev4=并集）；
  双实例实测列入 Phase 8。

### 2. library_id 防误合并缺失 ✅ 已修复
- **问题**：`read_manifest` 只校验 schema，未校验远端 `library_id`；两个不同同步库指向同一
  WebDAV 目录会被静默合并，数据相互污染。
- **修复**：`sync_with_webdav` 读取远端后校验 `remote.library_id == 本地 library_id`，不一致直接拒绝。

### 3. 同步应用后 UI 不刷新 ✅ 已修复
- **问题**：合并结果写入 Rust SQLite，但 Dart 内存态（LibraryStore）不感知，用户看不到拉取结果。
- **修复**：`SyncEngine.syncNow` 成功后调用 `LibraryStore.instance.load()` 重载；
  `_syncing` 标志保证重载触发的 notify 不会造成防抖循环。

## P1 — Phase 5/6 前置处理（建议本轮之后、大规模 library_index 之前安排）

### 4. 设备身份与名称未进同步协议
- **问题**：`state.rs` 声明了 `devices/` 目录，但 manifest 不含 device_id/device_name，
  `devices/<id>.json` 从未读写；Phase 6 设备分组无数据来源。
- **建议**：manifest 增加 `device_id/device_name`（写者身份，LWW/确定性合并）；
  push 时写 `devices/<device_id>.json`（last_seen_at）；拉取端注册设备并更新显示名。

### 5. 远端墓碑无限累积
- **问题**：删除传播产生的墓碑条目会残留在远端状态文件（merged 为空时不再重写文件），
  长期累积使状态文件单调膨胀。
- **建议**：维护"墓碑仅保留 N 版"，在无变化检测中加入墓碑清理（重写远端文件去掉已传播墓碑）
  或改用"远端只存活条目 + 墓碑仅存在于 base"模型。

### 6. 同步期间持有 DB 锁做网络 IO
- **问题**：`sync_with_webdav` 在 FRB 调用全程持有 `Mutex<Connection>`，上传/下载网络期间
  阻塞其他 DB 操作（大状态文件时 UI 卡顿）。
- **建议**：重构为"锁内快照 + 锁外网络 + 锁内应用/推进"，或对 sync 使用独立连接。

### 7. 自动触发缺前台/网络恢复；定时轮询全量拉取
- **问题**：当前仅启动 + 定时 60s + 变更防抖；回前台/网络恢复未接；轮询每次全量 GET 状态文件。
- **建议**：接入 `AppLifecycleListener`（resume → 同步）；轮询先 HEAD/GET manifest 比对 revision
  （或 ETag）再决定是否下载全量。

### 8. 墓碑在远端传播后的清理时机
- **问题**：删除传播依赖"base 存在"的缺席推断；首次同步（无 base）时远端缺席 ≠ 删除，
  语义正确但依赖 base 先建立。若用户删除发生在首次同步前，不会传播删除（可接受）。
- **建议**：文档注明"删除同步以至少一次成功同步为前提"；Phase 8 手测覆盖。

## P2 — 已知限制（记录在案，暂不处理）

- **时钟偏差**：LWW 依赖各端 `updated_at`，快时钟设备在同字段冲突时占优；仅等值平局做确定性
  tie-break（墓碑优先防复活）。
- **字段删除双端复活**：metas 某字段被两端同时删除时按 LWW 可能复活（边缘场景，可接受）。
- **源账号用户名明文进状态文件**：sources 数据含 `username`（非密码）；同步基础设施密码
  （`sync_webdav_*`）已由 settings 白名单排除。
- **首次同步以本地为准**：远端为空且本地已有 base 时按本地全量重推（远端被清空场景可自愈）。

## 下一步建议

1. 设备身份（P1-4）先做——Phase 6 设备分组依赖它，且不依赖 library_index。
2. 墓碑清理（P1-5）与锁外网络（P1-6）随 Phase 5 压测一起做（压测会暴露锁与体积问题）。
3. 然后进入 Phase 5 library_index 同步与压测。
