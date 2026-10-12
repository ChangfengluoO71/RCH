# 后台任务与前台响应优化实施计划

> **For agentic workers:** 本计划由当前执行者 inline 逐项实施；遵循 Trellis 子任务顺序。阶段 3 isolate 未获批准。

**Goal:** 降低扫描、同步和批量操作对前台的影响，并让云端扫码/Cookie 等待过程可见，同时保留目录索引和自动化正确性。

**Architecture:** 先用 Profile/Release 采集帧时间、阶段耗时和数据库等待。之后在 Dart 索引/自动化层实现 single-flight 与源级 pending dirty；通过 Rust SQLite 批量 API 处理标签及索引变更；最后增加轻量前后台调度与云端连接 loading，不实现通用任务框架或 isolate。

**Tech Stack:** Flutter/Dart、Rust、SQLite、flutter_rust_bridge、Trellis。

**Spec:** `docs/superpowers/specs/2026-10-12-background-work-performance-design.md`。

## Global Constraints

- SQLite 使用唯一全局连接；事务中只做 SQL，文件/网络 I/O 在事务外。
- generated Rust bridge files 由 codegen 生成，禁止手动修改。
- BatchTagTarget 必须跨 UI/store/index 保留 `file`/`dir` 类型和 parent path。
- Flutter UI 不在 build 内做 I/O，不用 Timer/Future.delayed 轮询推进状态。
- 日志不记录 Cookie、token、完整路径、书名或账号。
- 不修改 H1 EH Daily / 115 自动化调度及持久化契约；isolate 暂不批准。
- 保留本工作区已有未提交改动，只增量适配，不重置或覆盖。

## Review Focus

1. dirty generation 在扫描、同步、持久化期间变更时仍补跑，且协调器自己的通知不会形成循环。
2. 远端 revision/no-op、远端目录应用变化、本地外部文件变化和 snapshot freshness 分开判定；unknown 保守处理。
3. 强制扫描排在 incremental 之后仍会执行，失败不会发布部分索引。
4. 批量标签的元数据和关系都持久化；超大批次不能形成长时间 mutex lock，部分失败不伪报成功。
5. 持续前台操作时后台最终得到有界服务；扫码/连接旧请求不会清除新请求的 loading 状态。

---

### Task 1: P0 性能基线

**Files:**
- Inspect: `app/lib/store/library_index_service.dart`
- Inspect: `app/lib/store/automation_coordinator.dart`
- Inspect: `app/lib/store/library_store.dart`
- Inspect: `app/lib/ui/home_page.dart`
- Inspect: `app/lib/ui/source_browser.dart`

- [ ] 用 Flutter Profile/Release 构建采集 Windows 扫描、同步、批量标签和云端连接路径的现有时间线/阶段耗时。
- [ ] 记录 `flutter devices` 结果：本轮无 Android 设备，因此移动端基线标记 pending，不用 Windows 数据代替。
- [ ] 若现有时间线不能区分实际瓶颈，先记录具体缺口，再决定最小诊断点；不得输出凭据或完整路径。

### Task 2: P1-A/P1-B 索引与自动化正确性

**Files:**
- Modify: `app/lib/store/library_index_service.dart`
- Modify: `app/lib/store/automation_coordinator.dart`
- Modify: `app/lib/store/sync_engine.dart`
- Modify: relevant Rust sync-result API only if existing result is insufficient
- Generate: `app/lib/src/rust/**` through FRB codegen if Rust API changes

- [ ] 给每个 source ID 增加 single-flight；normal 等待同源 normal/force，force 排在 normal 后执行。
- [ ] 保持旧索引直到完整扫描和提交成功；索引请求结果可被等待者共享。
- [ ] 替换宽泛 Store listener 为相关 dirty 信号；周期开始 capture generation，运行期间变化放入 pending，finally 后重核并补跑。
- [ ] 为同步新增结构化 outcome，表达 status、应用到本地的变更域/失效源、unknown 和错误类别；现有 UI 文案保持兼容。
- [ ] 本地源 freshness 独立于远端 revision；unknown invalidation 保守刷新。
- [ ] Profile 若证实 `previous` 子树筛选成本高，先建立按子树查询的数据结构；没有证据不做 isolate。

### Task 3: P1-C 批量标签

**Files:**
- Modify: `app/lib/store/library_store.dart`
- Modify: `app/lib/repository/tag_repository.dart`
- Modify: `app/lib/store/library_index_service.dart`
- Modify: `app/rust/src/api/db.rs` and required DB SQL helpers
- Generate: `app/lib/src/rust/**` through FRB codegen
- Modify: `app/lib/ui/batch_tagging.dart`, `app/lib/ui/source_browser.dart`

- [ ] 设计不可变 batch plan，保留去重后的目标路径/类型/名称/父路径。
- [ ] 将 index upsert 按锁预算拆为幂等有界事务。
- [ ] 批量更新 `BookMeta.tags` 与 Tag/BookTag 内存关系，仅合并通知。
- [ ] 持久化本次变更的元数据、Tag 和关系，避免全量 load/diff 后逐行 FFI；保存结果与本操作关联。
- [ ] 操作返回阶段级 `processed/total/succeeded/skipped/failed`；UI 展示阶段进度和最终摘要，不按目标逐项刷新。
- [ ] 维持选中文件/目录、author/genre/series、已读、去重和索引失败处理语义。

### Task 4: P2 前后台任务协调

**Files:**
- Modify: `app/lib/store/automation_coordinator.dart`
- Modify: `app/lib/store/library_index_service.dart`
- Create or extend: small foreground-work coordination abstraction only if existing state cannot express leases
- Modify: targeted foreground scan/sync/batch entry points

- [ ] 用户主动操作、当前页面必要查询、已提交数据持久化和一致性恢复不排到 routine maintenance 后面。
- [ ] 常规 refresh/scrape/cleanup 可按 source/type 合并，前台 busy 时 defer。
- [ ] 按最大等待期限 aging；aging 后每次只做有界工作片段，片段边界优先处理必要前台请求。
- [ ] 应用 pause/resume 复用现有 lifecycle；不加入 UI timer polling。
- [ ] 仅在 Profile/DB 指标证明 500ms 轮询造成锁压力时调低后台非可见轮询频率。
- [ ] EH Daily 与 115 automation 完全留在独立通道。

### Task 5: 云端扫码与 Cookie 连接 loading

**Files:**
- Modify: `app/lib/ui/home_page.dart`
- Modify: `app/lib/ui/cloud115_qr_scan.dart` and/or `app/lib/ui/quark_qr_scan.dart` only after inspecting existing state contract
- Modify: `app/lib/ui/source_browser.dart` if reconnect entry lacks feedback

- [ ] 夸克/115 QR scan、Cookie validate/connect 和首次目录加载显示即时 spinner 与真实阶段文案。
- [ ] 使用请求 ID/token 避免旧 Future 清理新操作状态；进行中禁用重复提交。
- [ ] success/cancel/timeout/error 都收尾；失败保留可读错误与 retry。
- [ ] loading 由异步状态推进，不用 UI polling，不显示伪百分比。

### Task 6: 集成复核

- [ ] 对照设计核查所有接口边界、transaction time、dirty replay、foreground fairness 和登录 UX。
- [ ] Android 真机 Profile 与代表性数据批次仍需补采；当前无设备。
- [ ] 按当前会话约束不运行测试/分析；最终报告明确列出未验证项。
- [ ] 不提交、不推送。
