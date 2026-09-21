# 统一云端索引查询、封面服务与任务队列实施计划

> **For agentic workers:** 使用 superpowers:executing-plans 逐任务执行；只有用户明确选择子代理执行时使用 superpowers:subagent-driven-development，模型上限沿用用户的 luna max 约束。以下复选框是实施检查点，本文件获准编写不表示生产代码已实现。

**Goal:** 首次打开任意云端根目录后，无需进入子目录即可看到逐步生成的漫画/文件夹封面；阅读优先、进度真实、缓存和清理一致。

**Architecture:** Rust SQLite 统一索引、任务、封面版本与删除证明。Dart 查询目录展示模型、提交可见需求、监听批量修订；浏览与扫描共用 Rust 封面服务。复用现有 provider 会话、Document 解码、governor 和 Reader BookKey，不重写 provider 或阅读系统。

**Tech Stack:** Rust、rusqlite/SQLite、现有同步 provider I/O 与 tokio blocking worker、flutter_rust_bridge 2.12.0、Flutter/Dart。

**Spec:** `../../09-14-remote-cloud-scan/research/2026-09-14-remote-cloud-scan-design.md`；本计划第 1 节列出对旧设计的明确修订；证据见 `.trellis/tasks/09-14-remote-scan-ui-verify/research/2026-09-17-cover-pipeline-review.md`。

## 1. 全局约束与设计修订

- 所有云端会话来源：WebDAV、SFTP、百度、115、夸克；SMB/NAS 和本地卡片维持既有路径。
- M8 Catalog-only/local-only，不引入线上刮削、后台常驻服务或新权限。
- 保留现有 Reader BookKey、阅读记录和元数据键；新增 assetId 是索引内的解析身份，不整体迁移历史 BookKey。
- 不修改版本号，不提交、不打 tag、不发布；保留执行开始时的 tracked/untracked/staged 改动。每任务验收后作为独立可审阅变更集，不自动提交。
- 后台不能自动整本下载；Range 不可用保留占位和解释。自定义封面整本下载继续显式确认；受限单图片读取只走既有明确允许策略。
- 阅读完成只清 page/raw/content，保留封面及关键用户状态；已验证远端删除才进入失效清理。
- 增量不得仅凭父目录指纹未变跳过所有后代：无可靠递归版本保证时按目录复查到期时间遍历。
- 分离 listing_complete 和 cover_summary。目录全量基线在完整 listing 事务发布后成立；封面未齐可继续独立重试，不伪称全部完成。
- 第一阶段保留当前整代清单发布/删除证明。流水线预览单独作为第 10 步，不能借实时展示放宽删除条件。
- 优先级：当前阅读页 > Reader 预取 > 可见封面 > 后台封面/目录发现；后台封面与目录发现有公平轮转，避免互相饿死。
- 开关在每次新 I/O 前检查，不仅在任务启动时读取。已执行的同步 I/O 不能承诺瞬间终止，完成结果提交前仍检查代际/取消。
- UI 文案全部中文；错误码用于内部，显示经映射的中文；日志不含 Cookie、Authorization、token 或私有直链。

## 2. 现有代码与文件责任

| 文件 | 当前行为 | 计划责任 |
|---|---|---|
| `app/rust/src/remote_scan/persistence.rs` | 清单 stage/publish、基线、依赖 | 保留权威发布；事务内接入新任务/展示投影 |
| `app/rust/src/remote_scan/engine.rs` | 遍历与 cover stage | 只发现/分类/排队；增量复查 |
| `app/rust/src/api/remote_scan.rs` | FRB、job、封面消费混合 | 保留控制入口，委派提取/队列；新增统计 |
| `app/rust/src/api/source.rs` | session adapter、各 provider 封面 | 暴露受控会话适配工厂；旧远程封面入口逐步委派 |
| `app/rust/src/source/cloud115.rs` | 取链、探测、Range、缓存 | 类型化错误、受控取链合并、协议验证 |
| `app/rust/src/reader.rs` | 全局优先级 governor | 阅读保护、可见/后台工作区分 |
| `app/rust/src/cache.rs` | path/page/size/crop 缓存 | 本地保留；新增云端版本缓存入口 |
| `app/rust/src/api/cache.rs` | 阅读清理/验证删除 | 接入新封面索引、依赖、任务取消 |
| `app/lib/ui/source_browser.dart` | 快照判断容器封面 | 云端改读展示 DTO，移除快照驱动的网络选择 |
| `app/lib/ui/comic_cover.dart` | UI 排队并调用 provider cover | 云端改读缓存/声明需求，本地保持原分支 |
| `app/lib/store/remote_scan_coordinator.dart` | 触发、控制、多处轮询 | 唯一来源级同步与控制协调器 |

新增模块：`app/rust/src/remote_scan/catalog.rs`（展示查询/身份解析）、`cover_model.rs`（内部类型）、`cover_store.rs`（任务/缓存持久化）、`cover_service.rs`（去重/提取/发布）、`provider_budget.rs`（账号预算）；`app/rust/src/api/remote_cover.rs`（FRB）；`app/lib/store/remote_cover_repository.dart`（UI 统一入口）。在现有 `remote_scan/mod.rs`、`api/mod.rs` 注册，不先移动无关代码。

## 3. 固定数据与接口合同

### 3.1 身份

`assetId` 为库内 ID，优先使用既有 `library_index.id`；`sourceId + logicalPath` 保持旧 BookKey 兼容。provider 的 cid/fid/pickcode 单列为路由映射；临时 URL 仅内存。sourceFingerprint/root 改变使旧映射失效。任务执行时绑定当前 sessionEpoch，不跨会话复用临时取链结果。

封面任务唯一键：`(sourceId, assetId, contentRevision, selectionRevision, profile)`。selectionRevision 对应用户页码、裁剪、显式封面选择的确定性哈希；profile 只含尺寸/解码版本。容器目录引用代表漫画封面，不重复提取一次。

### 3.2 加法迁移

在 `cover_store::migrate` 建以下逻辑表，并由现有迁移入口在事务中调用；所有 schema 版本幂等校验：

| 表 | 主键/唯一键 | 必需字段 |
|---|---|---|
| `remote_asset_route` | source_id, asset_id | logical_path, parent_asset_id, provider_id, provider_file_id(nullable), source_fingerprint, generation, route_revision |
| `remote_directory_cover` | source_id, directory_asset_id | representative_asset_id(nullable), selection_reason, revision, completeness |
| `remote_cover_job` | job_key | source_id, asset_id, content_revision, selection_revision, profile, state, demand_kind, priority, attempt, next_attempt_at, lease_owner, lease_until, generation, session_epoch, error_code, updated_at |
| `remote_cover_blob` | blob_key | relative_path, format, byte_size, width, height, checksum, storage_version |
| `remote_cover_variant` | source_id, asset_id, content_revision, selection_revision, profile | blob_key, state, revision, updated_at |
| `remote_cover_ref` | owner_key, blob_key, role | source_id, asset_id, dependency_revision |
| `remote_view_revision` | source_id | revision, listing_generation, updated_at |

目录到期复查字段添加到 `remote_listing_state`：`last_checked_at`、`recheck_after`。建 job(state,next_attempt_at,priority)、route(source_id,parent_asset_id)、ref(source_id,asset_id) 索引。数据库不保存凭据/直链；认证代际是非秘密内部标识。

旧 `remote_cover_dependency`/tombstone 表继续作为清理兼容层；新 cover_ref 表补充多规格/共享引用，不冒然改旧主键。旧 stage 暂时作为入队输入，第 6 步接通后不再直接消费生成封面。

### 3.3 FRB 新接口（计划签名）

```rust
pub async fn remote_directory_view(
    source_id: String, logical_path: String, offset: u32, limit: u32,
) -> Result<RemoteDirectoryViewDto, String>;
pub async fn remote_cover_request(
    source_id: String, asset_id: String, consumer_id: String,
    selection: CoverSelectionDto, profile: CoverProfileDto,
) -> Result<RemoteCoverStateDto, String>;
pub async fn remote_cover_read(
    source_id: String, asset_id: String,
    selection: CoverSelectionDto, profile: CoverProfileDto,
) -> Result<Option<crate::api::book::PageImage>, String>;
pub fn remote_cover_release(consumer_id: String) -> Result<(), String>;
pub fn remote_cover_retry(source_id: String, asset_ids: Vec<String>)
    -> Result<(), String>;
pub fn remote_view_revision(source_id: String) -> Result<i64, String>;
```

复用已核实的 `crate::api::book::PageImage` 和 `crate::api::book::CropRect`，不复制像素 DTO。`read` 永不发网络；`request` 只提交/提升需求，不等待网络完成；`release` 释放当前卡片需求，不取消其他卡片/后台共享任务。retry 空列表表示该来源可重试失败项，不能重试 unsupported、用户暂停或登录失效任务。

`CoverSelectionDto { page:u32, crop:Option<CropRect>, explicit_asset_id:Option<String>, revision:String }`；`CoverProfileDto { width:u32, height:u32, decoder_version:u32 }`。后端验证 revision 与当前持久化用户选择一致，拒绝旧选择覆盖新选择。

`RemoteDirectoryViewDto { source_id, logical_path, revision:i64, listing_complete:bool, has_more:bool, entries:Vec<RemoteDirectoryEntryDto> }`。

`RemoteDirectoryEntryDto { asset_id, logical_path, provider_path:Option<String>, name, asset_kind, size:Option<u64>, representative_asset_id:Option<String>, cover:RemoteCoverStateDto }`。provider_path 为 cid/pickcode/普通路径而非 URL，只为既有 Reader 兼容。

`RemoteCoverStateDto { state, revision:i64, ready:bool, error_code:Option<String>, retry_at:Option<i64>, is_previous_revision:bool }`。有旧图和正在刷新可同时成立；不能用单一状态覆盖旧图可用性。

扩展 RemoteScanStatusDto：`listing_phase, directories_checked, discovered_books, discovery_complete, ready_books, active_books, pending_books, retry_books, blocked_books, unsupported_books, failed_books, view_revision`。字段默认兼容，processed/total 仅供旧客户端；新 UI 不使用其“成功”语义。同本多 profile 不重复计漫画数；参考当前选择的封面状态归类。

## 4. 实施顺序

依赖链：1 → 2 → 3 → 4 → 5 → 6 → 7 → 8 → 9 → 10 → 11。第 10 步可独立推迟；第 1–9 步构成必需功能闭环。

### 步骤 1：错误与构建证据先行

**修改：** `remote_scan/adapter.rs`、`api/source.rs`、`source/cloud115.rs`、`api/remote_scan.rs`。**测试：** 新增 `app/rust/tests/remote_cover_error_contract.rs`，复用 provider fake。

- [ ] 编写错误契约：115 downurl 返回 Unauthorized 时 capabilities 返回 Unauthorized；405 HTML 记录阶段与 HTTP 状态，不变成 RangeUnavailable；断网返回 TransientNetwork；200 的 Range 响应为不支持/被忽略；206 错范围或缺总长为 MalformedResponse。
- [ ] 运行 `cargo test --manifest-path app/rust/Cargo.toml --test remote_cover_error_contract -- --test-threads=1`，记录失败断言。
- [ ] 新增 `probe_checked(url) -> Result<RangeProbe, RemoteScanError>`，RangeProbe 包含 supported 和 total_size；旧 `(bool,u64)` 入口暂作 wrapper。去掉统一 adapter 的 `.unwrap_or(false)` 错误吞并。
- [ ] 以同一 typed error 贯通取链、探测、读取；保留脱敏 stage/status/attempt，不记录响应 HTML 全文或 URL。
- [ ] 记录构建时 Rust/Dart 修订与运行模块路径，不能只比较 exe 时间戳；不通过改应用版本实现。
- [ ] 复跑测试及既有 remote_scan_contract。产物：错误原因可信，尚不改变扫描调度。

### 步骤 2：加法 schema 和路由身份

**新增：** `cover_model.rs`、`cover_store.rs`。**修改：** `persistence.rs`、`mod.rs`、`api/source.rs`。**测试：** `app/rust/tests/remote_cover_store_contract.rs`。

- [ ] 在旧数据库 fixture 上连续调用 migrate 两次，验证旧 BookKey、历史、用户封面参数、baseline 和 tombstone 完全保留。
- [ ] 建第 3.2 节表/索引，复用 library_index.id；不把所有 provider 强行归为 pickcode。
- [ ] 在清单发布事务内写 route、父子关联与 view_revision；旧 generation/session 写入拒绝。
- [ ] 浏览初始 listing 与后台 listing 走同一路由注册，重启后从 route 恢复适配器映射；凭据从既有 session/vault 路径获取。
- [ ] 测试同一路径在两个来源不同 asset/cache 身份、source 改根后旧 route 不生效、迁移中断后重跑安全。
- [ ] 运行 `cargo test --manifest-path app/rust/Cargo.toml --test remote_cover_store_contract -- --test-threads=1`。产物：权威身份映射，不改 Reader 键。

### 步骤 3：目录展示查询，先解除浏览快照依赖

**新增：** `catalog.rs`、`api/remote_cover.rs`。**修改：** `api/mod.rs`、`remote_scan/mod.rs`。**测试：** `app/rust/tests/remote_directory_view_contract.rs`。

- [ ] 构造“根目录 → 容器 → a.cbz”和“根目录 → 图片文件夹 → 1.jpg/2.jpg”，不创建 FolderSnapshot；查询必须分别返回容器代表漫画与图片文件夹首页关联。
- [ ] 实现 remote_directory_view，按 source/parent 查询，limit 最大 200；自然排序稳定，分页有确定性次级 assetId 排序。
- [ ] 代表漫画选择顺序：用户显式选择 > 当前仍有效的既定代表 > 自然序首个可用漫画；图片文件夹使用现有页序。只有子目录时按稳定层级规则求代表，检测循环并限制一次计算的节点预算，未完成记 pending。
- [ ] 代表封面状态跟随被引用资产；容器本身不增加 discovered_books。
- [ ] 原子更新 remote_directory_cover 与 revision；父目录变更向上批量传播，不对每个条目触发一次 UI 回调。
- [ ] 测试未知目录与空目录区别、分页截断保持旧投影、选中子漫画删除后的重选、未点击目录也可查询结果。
- [ ] 运行对应 test。产物：UI 能一次查询完整展示信息，不发 provider 请求。

### 步骤 4：本地封面读取与版本缓存

**新增：** `cover_service.rs`。**修改：** `cover_store.rs`、`cache.rs`、`api/remote_cover.rs`。**测试：** `app/rust/tests/remote_cover_cache_contract.rs`。

- [ ] 创建缓存后关闭网络，remote_cover_read 必须返回图片，fake provider 调用数为 0；sourceA 的 /a.cbz 不能被 sourceB 命中。
- [ ] 实现 read：当前 variant → 可安全复用的既有源图派生 → 有身份依据的旧 cache alias；无结果返回 None，禁止隐式取链。
- [ ] 原子文件写入使用同目录临时文件、长度/解码校验、sync、rename；文件发布成功后提交 blob/variant/ref/revision 事务。
- [ ] 源图与派生规格分开；只保留封面相关图像，限制压缩字节和解码像素；高画质不能把低清放大后标记满足。自定义页码/裁剪与默认预览分开键。
- [ ] 旧 path-only 缓存仅在可证明归属时导入；跨来源歧义时保留文件但不错误展示，不全库清空重建。
- [ ] 测试更改画质可从源图生成且不取链，改页码不会命中旧页，崩溃遗留临时文件不导致 ready，文件被手工清掉后状态可恢复。
- [ ] 运行对应 test。产物：网络禁用仍可离线显示，身份/版本可解释。

### 步骤 5：持久任务状态机与去重

**修改：** `cover_model.rs`、`cover_store.rs`、`cover_service.rs`、`api/remote_cover.rs`。**测试：** `app/rust/tests/remote_cover_queue_contract.rs`。

状态：pending → running → ready；暂时错误 → retry_wait；登录/开关 → blocked；不支持 → unsupported；重试预算耗尽 → failed；身份失效/删除 → cancelled。暂停与取消不同：暂停保留需求，取消终止该调用方需求。

- [ ] 写并发测试：后台与 20 个可见消费者请求同键，数据库一个 job，provider 提取一次，释放一个 consumer 不影响其他需求。
- [ ] 原子 upsert job；相同键只提升优先级/加入 consumer；重试不新建漫画计数。活跃进程 consumer 保存在受限内存表，重启不恢复已经消失的可见消费者。
- [ ] 通过 SQLite 短事务 claim 租约；网络和等待全部在事务外；发布需匹配 lease_owner、内容/选择版本及当前 source 身份。
- [ ] 执行进程启动时回收旧 owner 的 running 租约，重新绑定有效 session；已 ready 且文件有效不再次下载。
- [ ] retry_wait 存 next_attempt_at，定时唤醒不占 worker 槽；最多 3 次自动重试，采用可注入时钟和随机源，遵守 Retry-After。手动 retry 重置预算但仍经过账号冷却和总开关。
- [ ] 限制可见需求集合为当前 viewport 加预加载窗口（最多 200 项），worker 默认全局 2 个后台槽且仍服从 governor；磁盘待办分页读取，内存候选最多 64。
- [ ] 测试重启恢复、租约失效、同键失败复用、取消共享需求、旧 session 不能发布、队列不随全库数量常驻内存。
- [ ] 运行对应 test。产物：任务可恢复、可取消、不会重复提取。

### 步骤 6：统一请求预算与接管扫描封面

**新增：** `provider_budget.rs`。**修改：** `reader.rs`、`source/cloud115.rs`、`api/source.rs`、`api/remote_scan.rs`、`cover_service.rs`。**测试：** `app/rust/tests/remote_cover_scheduler_contract.rs`，既有 reader/governor 测试。

- [ ] 使用 fake 慢请求验证阅读先于排队后台任务获得下一次请求机会；同账号多个根目录共享预算，不同账号不会误共享认证结果。
- [ ] 账号 key 使用 provider + 非秘密账号身份；无法获知账号时按 provider 保守聚合限流，取链缓存仍以 source/session 隔离，不能以 Cookie 原文作持久键。
- [ ] 115 改按 account/credentialEpoch/pickcode 合并取链结果（成功和失败均共享）；保留实际请求速率约束，不因移除全局锁增加无限并发。
- [ ] 避免 worker 拿着全局 I/O 槽等待账号冷却：队列先检查可运行时间，尝试联合获取预算，失败释放并重新排队；确定锁顺序，不持有数据库锁进入网络。
- [ ] 保留 current page > prefetch > visible cover > background；后台目录/封面公平轮转；记录队列等待与 I/O 延迟，说明同步在途请求不可抢占。
- [ ] 将 fetch_remote_cover_image、cover_source_info、cache publish 从 api/remote_scan.rs 委派给服务。发布 listing 同事务生成 job，替换 consume_staged_covers 的直接下载循环。
- [ ] 目录完整 baseline 独立发布，封面失败继续留队；旧 RemoteScanStatus.status 在两阶段完全结束前不显示无条件 complete。旧客户端兼容终态含 errorCode。
- [ ] 设置读取通过更新通知与每次 I/O 边界复核；关闭 remoteCoverFetchEnabled 后不启动下一次封面 I/O，已有图和待办保留。
- [ ] 运行 scheduler、queue、error、remote_scan_contract；测量取链/Range 调用数而非只测 UI 状态。产物：后台与可见任务共享同一网络链路。

### 步骤 7：Flutter 接入统一 Repository 和真实进度

**新增：** `app/lib/store/remote_cover_repository.dart`。**修改：** `source_browser.dart`、`comic_cover.dart`、`remote_scan_status.dart`、`remote_scan_models.dart`、`remote_scan_coordinator.dart`、各生成文件。**测试：** 新增 `app/test/remote_cover_repository_test.dart`，扩展 `remote_scan_ui_regression_test.dart`、`remote_scan_status_test.dart`。

- [ ] Rust API 完成后在 app 目录执行 `flutter_rust_bridge_codegen generate`，同步生成的 Rust/Dart 文件，禁止手改生成绑定。
- [ ] Repository 暴露 directoryView、readCover、requestCover、releaseConsumer；UI 用 sourceId/assetId 稳定 key，保留本地 ComicCover 分支。
- [ ] 根目录云端卡片从 DTO 获取 representativeAssetId，移除 _detectRemoteFolderKind 对浏览快照作为权威的依赖；打开 Reader 仍走既有 provider 路由和 BookKey。
- [ ] 可见卡片先 read，再 request；离屏 release，不直接 cloud115CoverFor/quarkCover。旧 provider cover API 作为兼容 wrapper 委派同服务，禁止同时保留两个抓取 worker。
- [ ] RemoteScanCoordinator 为每来源唯一 500ms revision 轮询者；revision 未变不重建卡片，变更时批量刷新可见目录，后台无观察者降低/停止 UI 轮询。此版本不新增 FRB 事件总线。
- [ ] 新统计按“本”去重：发现中只显示动态已发现数；发现完成后显示总数和可用/处理中/等待/待重试/暂停/不支持/失败，容器不计一本。
- [ ] 只有 running 转圈；排队/冷却显示中文原因。有旧图时保持旧图，并显示刷新状态。对批量状态只做一次 setState，取消重叠检测，避免 AXTree 放大。
- [ ] Widget 测试：没有快照、未进入子目录，root 可逐步显示封面；重建 20 次不新增请求；网络关闭但磁盘有图可显示；多尺寸同资产不重复计数；退出页面不停止其他消费者。
- [ ] 在 app 运行 `flutter test --no-pub test/remote_cover_repository_test.dart test/remote_scan_ui_regression_test.dart test/remote_scan_status_test.dart`。产物：用户可见闭环。

### 步骤 8：清理、版本变更和迟到写入

**修改：** `api/cache.rs`、`cover_store.rs`、`cover_service.rs`、`persistence.rs`、`app/lib/ui/cache_manager.dart`。**测试：** 新增 `app/rust/tests/remote_cover_cleanup_contract.rs`，扩展 `app/test/remote_cache_cleanup_test.dart`、`remote_cover_dependency_test.dart`。

- [ ] 用 fake blocked worker 制造“开始获取 → 远端完整删除证明 → 清理 → worker 返回”，验证不能重建已删除封面。
- [ ] 清理事务先使任务/变体失效，移除 owner refs；无引用 blob 进入受控回收，目录重新选代表；父目录不可永久引用已经删除的版本。
- [ ] 阅读完成接口保持只清内容，不触碰 cover_blob、variant、ref 和选择参数。
- [ ] 新旧 alias 清理均绑定 source 身份/现有证明；旧 path-only 有跨来源歧义时不能顺带删其他来源有效缓存。
- [ ] 同一路径内容替换只失效对应 revision，新图成功后替换旧图；改根/删除来源使旧 owner 失效。
- [ ] 缓存管理统计包含新封面目录，显式清封面使 ready 降为可重建，不删除用户选择参数；禁止清理后所有条目同时无界重取。
- [ ] 执行 Rust cleanup contract 与 Dart cleanup/dependency 测试。产物：阅读清理和失效清理与新服务一致。

### 步骤 9：增量扫描与失败补偿

**修改：** `engine.rs`、`persistence.rs`、`adapter.rs`、`cover_store.rs`。**测试：** 扩展 `app/rust/tests/remote_scan_contract.rs`。

- [ ] 测试“父目录字段全不变，深层新增漫画”；到期复查必须发现。没有递归版本证明的 provider 不允许无限跳过。
- [ ] 为每目录保存复查时间，使用可注入时钟；默认自动重复打开 15 分钟内复用元数据，超过期限重列举到期目录。手动增量跳过 TTL、复查目录元数据，但只提取变更/缺失封面；全量重新验证全部目录与删除证明。
- [ ] 失败封面队列独立唤醒，不等待目录变化；metadata unchanged 且封面有效则不产生新网络提取。
- [ ] pagination 完成、source/root/session 验证和旧 generation 保留沿用原 contract；时间戳未知不等于未变化。
- [ ] 测试相同库二次扫描封面请求为 0、失败重试无需全量、目录分页失败不产生删除。
- [ ] 运行 remote_scan_contract 与 queue/cleanup。产物：增量能收敛，封面失败不会永久停留占位。

### 步骤 10：首屏提前生成（后置优化，可独立延期）

**修改：** `engine.rs`、`persistence.rs`、`catalog.rs`、`cover_service.rs`。**测试：** 新增 `app/rust/tests/remote_cover_preview_contract.rs`。

- [ ] 在整代尚未遍历完成时，已完成分页的目录可建立 source/session/generation 限定的预览投影，并排队封面。
- [ ] 查询明确区分 preview 与 authoritative，优先保留已提交结果；预览只增加/更新展示，绝不作为删除、全量 baseline 或路径消失证据。
- [ ] 预览 source info 从对应 stage 读取，不读取上一代 library_index 冒充新文件。取消/失败撤销预览引用，保留上一完整 generation。
- [ ] 全代成功发布后将相同内容 revision 的结果提升为正式引用，不能再提取一次；孤立预览 blob 通过引用回收。
- [ ] 验证一个目录封面出现时另外目录仍在扫描，随后模拟失败，旧内容/删除证明保持；观察到 viewport 目录需求时通过队列提升发现优先级。
- [ ] 执行 preview、cleanup 和全量 remote_scan_contract。产物：首屏更早显示且不牺牲删除安全。

### 步骤 11：交叉验证与启用门禁

**文档：** 在现有子任务补充执行证据，更新 backend netdisk/source-refresh-cleanup 及 frontend 相关契约。

- [ ] fake provider 覆盖五类来源；真实账号优先 115、夸克，其余缺证据明确 pending，不使用 fake 冒充真实通过。
- [ ] 隔离 fixtures 验证根目录不点击子目录、图片文件夹、压缩包容器、多个来源同路径、深层变化与至少 200 项列表。
- [ ] 固定样本比较扫描关闭/开启：首张封面时间、全库完成时间、每本取链次数/Range 次数、缓存命中、峰值 RSS、阅读页 P50/P95。建议验收目标为扫描开启后阅读 P95 增量不超过 max(基线20%,200ms)，这是项目目标而非 provider 保证；不达标就减后台预算并复测。
- [ ] 验证 Windows AXTree 不持续刷屏、长列表重建有界；Android 进入后台/进程重启后队列恢复，系统挂起期间不承诺持续运行。
- [ ] 在 app/rust 执行 cargo fmt --all -- --check、cargo clippy --workspace --all-targets --all-features -- -D warnings、cargo test --workspace --locked -j 1 -- --test-threads=1。
- [ ] 在 app 执行 flutter analyze --no-pub、flutter test --no-pub、dart run tool/startup_contract_check.dart、flutter build windows、flutter build apk --debug。
- [ ] 仓库根执行 git diff --check；fmt/Clippy 分类本轮新增/既有 dirty/历史无关，只修本轮新增。
- [ ] 对照原规格逐条核验：统一身份/缓存任务（2–7）、阅读优先（6）、中文状态（7）、删除/设置保护（6–8）、增量（9）、实时首屏（10）、五 provider 证据（11）。

## 5. 回滚与启用边界

1. 步骤 1–6 在新服务未接 UI 前独立测试；切换时每个来源只有一个封面执行者，旧入口委派或停用，不双跑。
2. 加法表可保留；开关回退只停止新派发，保留 DB/封面/用户配置，不清库。运行时回退仍遵守网络总开关和限流。
3. 不承诺任意旧二进制都理解新缓存：优先回滚服务启用路径；版本降级仅对已验证可读取该 schema 的构建执行。活跃任务停派并失效租约后再切换。
4. 第 10 步预览单独控制；回退只隐藏预览、恢复已发布投影，不能回滚已确认的用户设置或删除其他来源缓存。
5. 基础闭环先以“根目录不点击能看到封面、计数可信、阅读不被压住”为验收；全部回归和真实 provider 证据完成前仍为 YELLOW / DO NOT RELEASE。

## 6. 执行检查示例

以下行为断言是各测试夹具应实现的实际目标（fake clock/provider 由对应测试文件局部定义，网络使用隔离本地服务）：

```text
Given root contains container A/a.cbz and image-folder B/1.jpg
And FolderSnapshotStore is empty
When full listing publishes and cover workers succeed
Then remote_directory_view(root).A.representative_asset_id == asset(A/a.cbz)
And root.B.cover.ready == true
And UI never needs to open A or B to display their covers

Given background + visible request use identical job_key
When visible consumer is released while background demand remains
Then extraction_count == 1 and background result can become ready

Given a current cover exists on disk and remoteCoverFetchEnabled == false
When remote_cover_read is called after process restart
Then pixels are returned and provider_call_count == 0

Given a leased worker holds source/session/generation/content revision V1
When verified deletion invalidates its asset before publish
Then publish is rejected and no ready cache reference is recreated
```

规划完成状态：只新增本计划；实施复选框全部未完成。下一执行入口为步骤 1，随后逐步验收，不直接跳到 UI 动画或增大并发。
