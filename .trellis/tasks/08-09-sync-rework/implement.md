# 实施计划：Sync State + Three-Way Merge（ADR-024/025）

> 未获用户对最终规划摘要的明确批准前，不修改产品代码。

## Phase 0 — 架构收敛（本文档 + prd/design + ADR 已完成）

- [x] 阅读 README/SPEC/DECISION/TODO/LOG 与现有实现。
- [x] 废弃"rchpkg v2 作为日常同步协议"；rchpkg 降级为备份格式（ADR-025）。
- [x] 更新 prd.md / design.md / implement.md；新增 ADR-024（Sync State + 三方合并）、ADR-025（备份/同步分离）。
- [ ] 向用户输出可行性评估 + 差异分析，获确认后开工。

## Phase 1 — 身份与同步状态基础设施

已完成：source_fingerprint（派生写入/回填/唯一性）、library_index/source_snapshot 表与 CRUD、settings 白名单、设备名。
新增：
- [x] `sync_base` / `sync_meta` 表 + CRUD（`sync/base.rs`）；本地 key ↔ 同步稳定身份映射层（`sync/identity.rs`）。
- [x] 同步层 book_id = sha256(fingerprint + path)（复用 `library_index_id` 规则）统一收敛（`sync::identity::book_id`）。
- [x] fingerprint 规范化补强：URL query/fragment 剥离、Windows 盘符小写；迁移矩阵单测（大小写/斜杠/尾斜杠/query/空字段/重复源）。
- [x] 单测：base 增查改删/实体级清理/meta/库 ID 唯一；身份映射命中与未命中。

## Phase 2 — WebDAV Sync State 协议

- [x] `sync/state.rs`：manifest（schema/library_id/revision/files/history）+ 版本化文件命名 + 实体 hash + schema 校验 + 修剪策略；单测覆盖。
- [x] `sync/webdav.rs`：read_manifest / download_state / upload_state（复用 source::webdav 客户端）；版本化文件直接 PUT、manifest 先 `tmp` 后 MOVE（新增 `WebDavClient::move_file`）；CAS（expected revision 冲突拒绝）；修剪旧文件。
- [x] 单元测试：文件名/hash/历史裁剪/schema 拒绝/路径拼接/404 识别。
- [x] CAS 重试循环 + library_id 防误合并：`sync::sync_with_webdav`（≤3 次重试；本地事务应用；成功才推进 base）。
- [x] FRB `api/sync.rs`：sync_now / sync_status / syncSetLastError / syncClearLastError / syncLocalCounts + codegen + release 构建。
- [ ] 集成：双实例 push/pull、半包不被读、manifest 冲突重试、断点恢复（Phase 3 后）。

## Phase 3 — Three-Way Merge 引擎（Rust domain）

- [x] `sync/merge.rs`：SyncEntry + three_way + merge_batch → MergeResult（merged + plan）；策略：metas 字段级、tags/book_tags 并集+墓碑、records/sources/settings LWW+墓碑、library_index 单端胜/LWW+墓碑；平局墓碑胜防复活。
- [x] Sync Plan 可序列化（`SyncPlanItem` serde）；`sync_now` 返回 plan_json 供诊断。
- [x] 单测矩阵：local only / remote only / both same / 同字段双改 / 异字段双改 / 删除 vs 更新 / 删除 vs 删除 / tags 并集 / settings LWW / plan 决策。
- [x] 与 rchpkg 导入解耦：rchpkg 恢复走 force 覆盖（`import_package(force=true)`），不经三方合并（merge.rs 头注释 + 本项）。

## Phase 4 — 自动同步 Worker

- [x] Rust 编排：`snapshot.rs`（7 实体本地装载/身份映射/base 推进）、`apply.rs`（合并结果回写本地，凭据 COALESCE 保留）、`mod.rs`（plan_merge / sync_with_webdav CAS）。
- [x] Dart `SyncEngine`：本地变更防抖 2s、启动拉取、定时 60s、手动"立即同步"；自动开关持久化（sync_auto）；失败记录 last_error；SyncPanel 接入按钮/开关/状态。
- [x] 单测：初始推送、远端拉取应用、远端删除传播、base 推进/清理、合并矩阵（132 Rust + 54 Flutter 全绿）。
- [ ] 双实例实测：修改后无需手动操作另一设备自动获得变化（需真实 WebDAV）。
- [ ] 回前台/网络恢复触发（接入 AppLifecycleListener）。

## Phase 4.5 — 协议稳定性审查（2026-08-09）

- [x] 审查结论文档：`.trellis/tasks/08-09-sync-rework/phase45-review.md`。
- [x] P0-1 并发覆盖丢数据（TOCTOU）：upload 成功后回读 manifest 校验 revision/library_id，不匹配不推进 base 并重试。
- [x] P0-2 library_id 防误合并：远端库不一致直接拒绝。
- [x] P0-3 同步应用后 UI 不刷新：SyncEngine 成功后重载 LibraryStore（_syncing 防循环）。
- [x] P1-4 设备身份/名称进 manifest + devices/（见 Phase 4.6）。
- [ ] P1-5 远端墓碑清理；P1-6 锁外网络；P1-7 前台/网络恢复触发 + ETag 轮询。
- [ ] P2 已知限制记录在案（时钟偏差/字段删除复活/username 明文/首次同步本地为准）。

## Phase 4.6 — 同步参与者身份 + 可观测性（ADR-026/027）

- [x] manifest schema v3：`writer{device_id, device_name}`（revision 元数据，不参与合并）。
- [x] device_id 改 UUID v4（`db::get_or_create_device_id_on`），永久稳定；改名只改 device_name。
- [x] `sync_devices` 注册表（新表，不复用旧 devices）+ 远端 `devices/<id>.json` 读写/注册（`sync/actor.rs`）。
- [x] SyncPlanItem 增加 local_revision / remote_revision / winner / reason。
- [x] `sync_history` 表 + 每次同步记录（含失败）+ FRB `syncHistoryRecent`（`sync/history.rs`）。
- [x] `sync_now(session, dir, platform)`；`syncDevicesList` FRB；release 重建。
- [x] 单测：UUID 稳定性、设备注册表、历史记录；135 Rust + 54 Flutter 全绿。
- [ ] 设置页"同步历史/参与者"展示（随 Phase 6 UI 一起做）。

## Phase 5 — library_index 集成与压测

## Phase 6 — 设备/书源/漫画三级 UI（按 6.0→6.5 顺序）

### Phase 6.0 — 数据语义层（2026-08-09）

- [x] 同步进来的书源一律标记远端（remote_only=true + origin_device_id=写入设备，apply 层 writer 透传）——UI 不再靠旧 remoteOnly 猜测。
- [x] `api/library.rs`：SourceAvailabilityDto（has_local_source/has_local_resource/has_credentials/device/is_remote/offline_index_count/can_browse_offline/requires_network/status）、SourceTreeNodeDto（设备→书源树）、BookSearchDto。
- [x] 三状态由 Rust 输出：read=🟢 / needs_network=🟡 / index_only=⚪（⚪=仅索引，不是不可用）。
- [x] `db_search_books` 分页 SQL（query/标签/含远端过滤），不整载 library_index。
- [x] 单测：远端书源标记（Rust 143 全绿）。

### Phase 6.1 — 目录树数据模型（2026-08-09）

- [x] 搜索状态细化：db_search_books 按凭据/本地资源计算 status。
- [x] 采纳翻转：LibraryStore.updateSource 保存时把远端书源转为逻辑本地源（remote_only=false + origin_device_id=null）。
- [x] Dart `LibraryCatalogStore`（loadTree/searchBooks/status 映射）；main 启动加载 + 同步成功后刷新。
- [x] 验证：143 Rust / 57 Flutter / analyze / release 重建。

- [x] Phase 6.2：三级树 UI（[source_tree.dart](C:/Users/cfl/Desktop/RCH/app/lib/ui/source_tree.dart)）——设备/书源节点 ExpansionTile 惰性构建；漫画列表分页 100/页滚动加载更多（不整载 library_index）；本机设备默认展开；书源节点显示 🟢🟡⚪ + 离线索引数 + 远端标记；点击按 status 走 openBook 或只读详情页；源操作（编辑/详情/删除）经回调接回原对话框（编辑即采纳为本地源）。
- [x] Rust `db_source_books`（按书源分页查询）+ codegen + release 重建。
- [x] 移除旧"隐藏 remote_only"的扁平书源列表与 `_sourceTile`。
- [x] 验证：analyze 无问题；Flutter 57 全绿。
- [x] Phase 6.3：SourceBrowser 离线优先（[source_browser.dart](C:/Users/cfl/Desktop/RCH/app/lib/ui/source_browser.dart)）——
  本地有文件 → 在线真目录；幽灵/远端 → 只读离线索引；云端本机源有索引 → 离线优先（横幅"离线索引浏览（不连服务器）"）；无索引 → 回退在线 + "生成离线索引"按钮。
  离线浏览走 `dbSourceDirEntries`（parent_id = book_id(fp,dir)，无会话、无 list 服务器）；下钻/返回/刷新离线感知；
  打开漫画仍走 BookDetailPage → 开始阅读时按本机文件/凭据判断（index_only 只读横幅）。
- [x] Rust：`dbSourceIndexCount`（模式判定）+ `dbSourceDirEntries`（离线目录分页）；codegen + release 重建。
- [x] 验证：analyze 无问题；143 Rust / 57 Flutter 全绿。
- [x] Phase 6.4：跨设备搜索/筛选 UI（[global_search.dart](C:/Users/cfl/Desktop/RCH/app/lib/ui/global_search.dart)）——
  全局搜索改走 Rust 分页 SQL（dbSearchBooks），结果行展示 🟢🟡⚪ + 设备 / 书源 / path / 标签；
  滚动加载更多；点击按 status 分流（可读 → openBook，index_only → 只读详情）；home_page 全局模式接入 + 计数联动过滤条。
- [x] Rust 搜索词扩展到书源名 + 设备名（sync_devices EXISTS + 本机 device_id）；结果返回标签（GROUP_CONCAT）。
- [x] 验证：143 Rust / 57 Flutter / analyze 无问题 / release 重建。
- [x] Phase 6.5：SyncPanel 收敛（[sync_panel.dart](C:/Users/cfl/Desktop/RCH/app/lib/ui/sync_panel.dart) 重写）——
  只保留 WebDAV 地址/账号/密码/远程目录 + 测试连接 + 设备名称（`sync_device_name` 持久化、随同步传播）+ 自动同步开关（60s 间隔说明）+ 最后同步（syncStatus）+ 状态 + 立即同步 + 参与设备（syncDevicesList）+ 同步历史（syncHistoryRecent：拉/推/合/冲突/错误）。
  旧 Push/Pull/Export/Import/Archive/模式下拉从面板移除（正式删除在 Phase 7）。
- [x] SyncManager 增加 deviceName 读写（load/save）。
- [x] 验证：analyze 无问题；Flutter 57 全绿。

### Phase 5.0 — 数据模型定稿（2026-08-09）

- [x] `library_index` 表新增 `hash` 列（新库 DDL + 老库 ensure_columns 迁移，幂等）。
- [x] 语义定稿：`entry_type ∈ {dir(文件夹级), file(漫画/book级)}`；**不进入图片/页级**。
- [x] `hash` = 条目元数据哈希 `sha256(path|name|type|size|mtime)`，用于增量检测与"同 path 不同 metadata → LWW"判定；**不是漫画内容哈希**（ADR-020 约束）。
- [x] Row / DTO / FRB / snapshot / apply 全链路携带 hash。
- [x] Dart 扫描器（本地 + 云端）生成 entryHashOf。
- [x] 单测：老库迁移补列、hash 往返（Rust 136 全绿）；entryHashOf 稳定性与扫描携带（Flutter 全绿）。

### Phase 5.1 — 本地扫描器（2026-08-09）

- [x] 增量扫描：目录 mtime 未变的子树跳过遍历、旧条目原样保留（`previous` + 非 force）。
- [x] 目录条目记录 modifiedAt/hash；修复嵌套文件 parentId 错指 bug。
- [x] `refreshSourceIndex` 本地源自动加载上一次索引走增量；root_hash 未变不写库。
- [x] 云端源增量复用 FolderSnapshotStore + 250ms 节流。
- [x] 单测：增量捕获新增、子树保留、force 全量（Flutter 57 全绿）。

### Phase 5.2 — 同步集成与软删策略（2026-08-09）

- [x] library_index 合并策略：同 path（同 book_id）不同 metadata → LWW（既有 lww）；删除保持 `deleted=true` 墓碑。
- [x] `apply_library_index` 墓碑 → 软删（UPDATE deleted=1），不立即 DELETE；live 条目 upsert 显式 `deleted=0`（复活路径）。
- [x] 整源替换 `replace_library_index_for_source_on`：不在新集合的旧条目软删（文件消失 → 墓碑可传播）。
- [x] 修复 snapshot 装载 library_index 的 hash/updated_at 列序错位（行被静默丢弃）。
- [x] 单测：软删差异、墓碑保留可导出、plan_merge 含 library_index、apply 墓碑软删（Rust 139 全绿）。

### Phase 5.3 — 压测与性能优化（2026-08-09）

- [x] 性能修复：`advance_base` 剪枝 O(n²)→O(n)；apply 预载 `LocalSources`（去掉逐条目全表 SQL）；合并去掉双重克隆；决策枚举替代深比较。
- [x] 诊断解耦：SyncPlan 可选且封顶（PLAN_CAP=500）；同步路径只做轻量计数（pull/push/merge/deleted 进 SyncOutcomeDto）。
- [x] JSONL：解析逐行直入 HashMap（无 `Vec<Entry>` 中间态）；序列化逐行追加。
- [x] 基准：`pipeline_scale_5k`（常跑，debug 预算 3s）+ `bench_pipeline_100k`（ignored）。
- [x] **release 实测 10 万条目本地管线 = 1214ms**（snapshot 183 / merge 755 / serialize 111 / parse 164），远低于 5s 目标；1 万条目约 150ms。
- [ ] 真实数据量内存 RSS 验证（<300MB）与 WebDAV 端到端同步时长（随 Phase 5.5 实机压测）。
### Phase 5.5 — 锁外网络 / 墓碑 GC / 断网恢复（2026-08-09）

- [x] 锁外网络（P1-6）：`sync_with_webdav_global` 生产入口——网络阶段（download/upload/verify/devices）**不持有 DB 锁**，DB 阶段（合并/应用/推进 base）短锁；测试路径保留 `sync_with_webdav(conn,…)`。
- [x] 墓碑 GC（P1-5）：`gc_library_index_tombstones` 清理软删超 30 天的 library_index 行，`finalize_sync` 成功后执行（删除传播期充足）。
- [x] 断网恢复：Dart `SyncEngine` 失败后 30s 自动重试（最多 5 次，成功重置）；错误写入 sync_history/sync_meta。
- [x] 阶段化拆分：`prepare_sync`（DB）/ `push_sync`（网络+CAS 校验）/ `finalize_sync`（DB+GC）/ `record_outcome_history`。
- [x] 单测：prepare 变化检测与 next_rev、GC 只清过期软删（Rust 142 全绿；Flutter 57）。
- [ ] 真实 WebDAV 端到端压测 + 10 万条目 RSS <300MB 验证（需实机；作为双端验收项）。
- [ ] 设备 → 书源 → library_index 树；墓碑；源删除级联。
- [ ] 压测 1 万 / 5 万 / 10 万条：JSONL 读写内存、包体积、同步耗时；115/WebDAV 限流与断网恢复。

## Phase 6 — UI

- [ ] 书源列表分区：普通书源区（本机 + fingerprint 命中同源）+ 设备折叠区（🟡 仅索引）；remote-only 不再隐藏。
- [ ] SourceBrowser 在线/离线索引双模式 + 三状态（🟢🟡⚪）；阅读按可读性启用。
- [ ] SyncPanel：WebDAV 配置 + 测试连接 + 立即同步 + 自动开关 + 设备名/ID + 同步状态；备份/恢复入口独立。

## Phase 7 — rchpkg 备份化（ADR-025）

- [x] 删除 rchbundle：home_page 按钮/方法/导入 + Rust `SourceBundleDto` / `source_bundle_encrypt/decrypt`（FRB codegen 已清）。
- [x] SyncManager 重写：删除 SyncMode / mode / dir / pushNow / pullNow / cleanArchives / 归档路径；保留 WebDAV 配置、设备名、跨设备搜索、export/restore 备份。
- [x] 备份独立入口：[backup_panel.dart](C:/Users/cfl/Desktop/RCH/app/lib/ui/backup_panel.dart)（导出/导入 .rchpkg，可选加密凭据）接入设置页，与同步分离。
- [x] 同步侧不再调用 rchpkg 增量导出（cursor_export 不再用于日常同步；rchpkg 机制保留给备份）。
- [x] 验证：143 Rust / 57 Flutter / analyze 无问题 / release 重建。

## Phase 8 — 全量回归

- [ ] `cargo test --lib` / `dart analyze` / `flutter test` 全绿；FRB codegen + release 双端构建。
- [ ] 双设备 20 项手测（spec §30）：新增/删除源与漫画、元数据、标签、进度、双端同时改、断网修改、网络恢复、WebDAV 不可用/半途失败、状态损坏、旧 rchpkg 导入、凭据同步（可选）、fingerprint 迁移、大 catalog、限流、崩溃恢复。
- [ ] 更新 LOG.md / LOG-INDEX.md / TODO.md / README；验收记录。

## Phase 9 — ADR-028 修复（2026-08-09，数据清空事故回归修复）

> 设计见 design.md §12。双设备实测触发后按 P0 顺序实施，每步 `cargo test --lib` 全绿。

### P0-1 manifest 全量引用（禁空文件）
- [x] `Manifest::push`：files = 每实体最新全量引用；未变化实体沿用旧引用，不写文件。
- [x] `build_remote_files`/`prepare_sync`：仅变化实体进 files 与 manifest.files。
- [x] `upload_state`：只上传变化文件；修剪排除当前 manifest 仍引用的文件。
- [x] `plan_merge`：manifest 未引用实体 = 沿用 base（不产生伪墓碑）。
- [x] 拉取端防御：被引用但为空的 state 文件直接报错（旧版残留），禁止当合法空状态。
- [x] 单测：局部变化不覆盖未变化实体文件；空实体文件不再出现；剪枝保护；远端全量可重建。

### P0-2 base 全量镜像
- [x] `advance_base`：incoming = merged ∪ base 未变化 key；保留墓碑；不再按差集 prune。
- [x] 单测：连续同步后 base 仍含未变化 key；墓碑保留；旧 prune 测试改写。

### P0-3 apply 禁止静默跳过
- [x] `sync_pending_apply` 建表 + CRUD（db/base.rs）。
- [x] apply_metas/records/library_index/book_tags：resolve 失败写 pending；墓碑清 pending。
- [x] snapshot 加载 pending 为 live；`reapply_pending`（新源可解析后落真实表）。
- [x] 单测：不可解析条目不产生墓碑；加源后可重新落库；240 行跨设备场景回归。

### P0-4 身份与序列化稳定
- [x] `normalize_index_path`（Rust db + Dart LibraryIndexService 一致），book_id/parent_id 统一。
- [x] sources 负载移除 remoteOnly/originDeviceId；apply 不覆盖本机标志（新源 origin=writer）。
- [x] 单测：两端路径形式不同 book_id 一致；连续同步无变化 0 推送。

### P1 UI/数据修复
- [x] `LibraryStore.load(force)` + SyncEngine 成功后 force reload；DTO 增加 path，兜底用真实 path。
- [x] `refreshSourceIndex` rootHash 短路增加 live 行数比对。
- [x] 数据修复：Windows 本地重置（备份 + 清 sync_base/sync_meta，见 sync-repair-reset.ps1）+ 远端 RCH同步 目录清空。
- [x] `prepare_sync` 远端无 manifest = 本地全量重推（自愈，防空 manifest）。
- [x] 轻量轮询 `sync_remote_revision` + SyncEngine 退避让位（消灭每分钟盲重试/503 循环）。
- [x] Android/MuMu 端：新 release APK 已构建安装（含全部修复），旧协议污染源已清除。
- [x] release DLL 重建（含自愈/轻量轮询改动，cargo build --release 完成；FRB 绑定已 codegen）。
- [x] 远端二次清理：删除 MuMu 旧协议写的 rev 1-16 全部 state/devices/manifest（95 文件），目录已空。
- [x] Windows 重开验证：自愈全量重推成功（远端 rev 14+），60s 轮询仅 revision 检查；
      MuMu 加入同步（409 根因=目录名隐藏控制字符，已 sanitize 防御），双端正常，提交归档。

### Phase 10 — 离线索引自动化（ADR-029）

- [x] Rust `ensure_index_entry_on`（父链补全、幂等、零网络）+ 单测。
- [x] FRB `dbEnsureIndexEntry` / `dbEnsureIndexEntries`（IndexEntryInput）。
- [x] Dart 触及即补：`LibraryStore.recordRead` / `batchTag` 接线。
- [x] 浏览即索引：`SourceBrowser._list` 写入当前目录快照索引。
- [x] 生成离线索引本地化：默认走 FolderSnapshotStore 快照（零网络）；
      "全量重建索引（联网）"移至菜单高级选项。
- [x] SyncEngine 同步前对云端源执行 `buildIndexFromSnapshots`。
- [x] 层级修正：显式 parent_path + 快照反查父目录 + 父链保留已有层级/目录名。
- [x] 层级修正二：父链只创建缺失条目（回归测试 parent_chain_never_resets_existing_hierarchy）。
- [x] SourceBrowser 常驻"退出"按钮（主页进入/离线浏览均可一键退出）。
- [x] release DLL 重建（ADR-029 + 层级修正，已完成）。

## 验证命令

```bash
cd app/rust && cargo test --lib
cd app && dart analyze lib
cd app && flutter test
cd app && .\codegen.ps1   # FRB 变更后
```
