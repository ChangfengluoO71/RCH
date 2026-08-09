# 同步系统重构：Sync State + Three-Way Merge（ADR-024/025）

## Goal

让 RCH 拥有可靠、可诊断、可扩展、WebDAV 后端无关、支持多设备自动同步的**本地优先数据同步系统**：

- 体验接近 Obsidian + Remotely Save：本机修改 → 防抖 → 自动同步；另一设备启动/回前台/定时/网络恢复 → 自动检查 → 三方合并 → UI 更新。手动按钮只做强制同步/恢复/调试。
- **不复制 SQLite 数据库文件**；**rchpkg 不再是日常同步协议**（降级为完整备份/迁移/离线恢复格式）。
- 日常同步 = WebDAV 上的**状态文件**（manifest + state + devices）+ 本机 **Sync Base** + **三方比较/语义合并**。
- 凭据默认不上传；可选开启时独立加密、按 source_fingerprint 绑定。

## 关键决策（ADR-024/025）

| 项目 | 决策 |
| --- | --- |
| SQLite 直接复制 | ❌ 禁止（一致性/并发合并/带宽均不可行） |
| rchpkg | ✅ 仅作备份/迁移/离线恢复格式；**不是日常同步协议** |
| 日常同步 | WebDAV 状态文件（manifest 作提交点，版本化文件，tmp+MOVE 原子写） |
| 同步模型 | 三方合并（Base + Local + Remote → Sync Plan → Merged） |
| Sync Base | ✅ 每实体 hash/JSON，成功推进、失败不回滚 |
| 身份 | source_fingerprint（已落地）+ 同步层稳定 book_id（fingerprint+规范化路径）；本地 SQLite key 不变，由映射层转换 |
| 合并策略 | 字段级（metas）/ 并集+墓碑（tags、book_tags）/ LWW+墓碑（records、sources、settings）/ 单端胜+墓碑（library_index） |
| 冲突 | 不引入 CRDT/Vector Clock/Event Sourcing；LWW 仅局部 |
| 凭据 | 默认不上传；可选开启 → 独立加密文件，按 fingerprint 绑定 |
| 近实时 | 本地变更防抖 1~3s；定时 pull 30~60s；启动/回前台/网络恢复触发 |
| 并发 | Push 采用 CAS 循环（manifest revision 冲突则重拉重并）；同写窗口最后写者胜（已知限制） |
| UI | 设备 → 书源 → 漫画三级树；远端书源不再隐藏；离线索引浏览；同源 fingerprint 命中合并为普通书源 |
| rchbundle | ❌ 删除 |

## 背景与确认事实（代码证据）

- 已落地（本任务前几阶段）：fingerprint 派生写入 + 存量回填（`db/mod.rs`）；`library_index`/`source_snapshot` 表与 CRUD；rchpkg v2 备份布局（manifest + sources/ + library_index/(entries.jsonl+snapshots.json) + metadata/ + records/ + tags/ + settings/ + tombstones.json）；settings 白名单；设备名 `sync_device_name`；FRB 批量索引 API；Dart `LibraryIndexService`（本地扫描 + 云端 BFS + root_hash）。
- 现状同步路径仍是"rchpkg 包导出 → WebDAV PUT / 本地文件"（`sync_manager.dart`），需要**废弃为备份语义**。
- 现状导入为整包 merge（LWW + 墓碑，`rchpkg::merge_package`），无 Sync Base、无字段级合并、无 Sync Plan。
- WebDAV 传输 API（upload/download/make_dir）已存在（`api/source.rs`），可复用于状态文件传输。
- FolderSnapshotStore（`folder_snapshot_store.dart`）可复用于云端目录增量检测。
- 本地实体 key：`type|source_id|path`（`bookKeyOf`）；导入时按 fingerprint 重映射 source id 并重写 key 前缀（`rchpkg::merge_package`）——同步层稳定身份可基于此扩展。

## Requirements

- **R1 三方合并引擎**：独立 Rust domain 层；输入 Base/Local/Remote，输出 Sync Plan 与 Merged State；实体覆盖 sources / book_metas / tags / book_tags / read_records / library_index / settings；每种实体有明确合并策略与测试（local only / remote only / both same / 同字段双改 / 异字段双改 / 删除 vs 更新 / 删除 vs 删除）。
- **R2 Sync Base**：SQLite `sync_base` 表（entity_type + entity_key + state_hash + state_json），与业务状态分离；只有同步成功才推进；失败不推进。
- **R3 WebDAV Sync State 协议**：`manifest.json`（schema_version / library_id / revision / updated_at / 各实体文件引用）+ `state/<entity>-<rev>.jsonl|json` + `devices/<id>.json`；版本化文件 + manifest 为提交点；tmp+MOVE 原子写；旧版本可修剪（保留最近 N 版）。
- **R4 CAS 并发控制**：Push 先读远端 revision，本地合并后写新 revision，manifest 冲突（revision 已变）→ 重拉重并重写，直至成功；绝不拿旧状态覆盖远端。
- **R5 稳定身份**：source_fingerprint（已有）；同步层 book_id = `hash(source_fingerprint + normalized_path)`；远端状态以稳定身份为 key；本地 SQLite key 通过映射层转换（不迁移本地表主键）。
- **R6 自动同步 Worker**：本地变更防抖 1~3s → pull-merge-push；启动/回前台/网络恢复/定时（30~60s，优先 ETag/revision 判断）触发 pull；失败自动重试；手动同步按钮保留。
- **R7 凭据**：默认不进入同步状态；可选"同步凭据"开关 + 口令 → 独立加密文件（AES-GCM，按 fingerprint 绑定）；同步基础设施密码（`sync_webdav_*`）永不明文进状态。
- **R8 settings 白名单**：仅用户级非敏感设置进同步（现有 `is_syncable_setting` 复用）。
- **R9 UI**：设备 → 书源 → 漫画三级树；普通书源区（本机 + fingerprint 命中的云端同源）+ 设备折叠区（未命中源，🟡 仅索引）；SourceBrowser 在线/离线索引双模式；三状态 🟢🟡⚪；同步状态/最后同步时间/失败提示展示。
- **R10 备份（rchpkg）**：导出/导入/恢复保留，支持可选加密凭据；与日常同步完全分离；v1 旧包可导入。
- **R11 删除/降级**：删除 rchbundle；删除"rchpkg 作为同步协议"路径（sync_manager push/pull、归档、cursor 增量）；remote-only 隐藏逻辑移除。

## Acceptance Criteria

- [ ] AC-R1：合并引擎 7 类实体全量单测（含场景 A/B/C/D）；Sync Plan 可在 debug/诊断中查看。
- [ ] AC-R2：同步成功后 Base 推进；下载/合并/上传任一失败 Base 不推进；断点重试幂等。
- [ ] AC-R3：两个 RCH 实例可通过 WebDAV 状态文件 push/pull；半包（tmp 未 MOVE / manifest 未更新）不被读取；manifest schema 不识别时安全拒绝。
- [ ] AC-R4：并发 push 场景（双端同时修改）不丢数据；revision 冲突自动重拉重并（压力测试）。
- [ ] AC-R5：同源在两台设备（不同 source_id）同步后身份一致；新增/重命名源后映射稳定。
- [ ] AC-R6：B 修改信息后 1~3s 内自动同步；A 启动/回前台自动拉取；断网恢复自动重试；无手动操作完成同步。
- [ ] AC-R7：默认同步状态不含任何凭据；开启凭据同步后远端仅存在加密文件，口令错误无法解密。
- [ ] AC-R8：状态文件不含 `sync_webdav_*`、cacheDir、token/cookie 明文。
- [ ] AC-R9：设备 B 看到设备 A 折叠组（🟡 可浏览编辑），同源合并为普通书源（🟢）；断网可浏览全部索引。
- [ ] AC-R10：rchpkg 备份导出/导入/恢复可用（含可选加密凭据）；v1 包可导入；日常同步不生成 rchpkg。
- [ ] AC-R11：仓库中无 rchbundle、无 sync_manager push/pull/归档代码；remote-only 不再被 UI 隐藏。
- [ ] 回归：`cargo test --lib`、`dart analyze`、`flutter test` 全绿；双端（Windows/Android）手测 20 项场景（spec §30）。

## Out of Scope

- 漫画文件跨设备传输；CRDT / Vector Clock / 完整 Event Sourcing；服务端账号系统。
- 多写者同时 PUT 的强一致（CAS 已缩窗；强一致需未来"每设备状态文件 + 汇总"）。
- 本地 SQLite 主键迁移（同步层映射实现稳定身份，本地 key 保持 `type|source_id|path`）。

