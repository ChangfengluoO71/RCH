# 全云端远程扫描与封面索引 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 为 WebDAV、SFTP、百度网盘、115 和夸克实现首次全量、后续增量、可手动重扫的远程扫描，并让图片文件夹封面、Reader 和失效缓存清理共享同一套一致性边界。

**Architecture:** Rust `RemoteScanEngine` 持有 provider adapter、SQLite 清单、检查点、限速队列、封面依赖和 tombstone；Dart `RemoteScanCoordinator` 只负责触发、控制和状态 UI。Reader、ComicCover 和 LibraryStore 通过稳定逻辑书籍键复用扫描结果、前台优先 governor 和分层缓存。

**Tech Stack:** Rust、rusqlite、Tokio `spawn_blocking`、Flutter/Dart、Flutter Rust Bridge、现有 WebDAV/SFTP/Baidu/115/Quark clients、现有 Reader/page/cover caches。

**Spec:** `2026-09-14-remote-cloud-scan-design.md`

## Global Constraints

- 云端范围固定为 WebDAV、SFTP、百度网盘、115（扫码 Cookie/现有兼容模式）和夸克；SMB/NAS 仍按本地文件系统处理。
- 首次授权或首次打开根目录启动一次可恢复 full job；后续打开/授权启动增量 job；手动入口同时支持增量和全量重扫。
- M8 继续 Catalog-only/local-only；不调用线上智能刮削，不新增元数据 provider。
- 请求优先级固定为：前台当前页 > Reader 预取 > 扫描目录/封面；所有队列、批次、响应和内存缓冲有硬上限。
- Range 成功必须是 `206 + 与请求一致的 Content-Range`；后台封面 Range 不可用时使用占位符/提示，不自动整本下载。
- 阅读完成只清理 page/raw，保留封面、元数据、书源、凭据、标签、历史、完成状态和自定义封面参数。
- 认证失败、权限错误、截断列表、取消、网络失败和单项暂时 404 不得产生删除 tombstone。
- 日志、测试 fixture、截图和报告不得记录 Authorization、Cookie、token、密码或带凭据直链。
- 不升级 AGP、Gradle、Kotlin、NDK，不修改版本号，不覆盖已有 dirty 修改；允许任务审查所需的 task-local commits，但当前 evidence 边界下不提交 tag/release、不 push。

## File Map (冻结边界)

### Rust 新建

- `app/rust/src/remote_scan/mod.rs`：模块导出和 engine 入口。
- `app/rust/src/remote_scan/model.rs`：资产类型、能力、错误、扫描状态、任务键和 FRB DTO 的 Rust 模型。
- `app/rust/src/remote_scan/adapter.rs`：provider-neutral adapter trait、列表页和有限读取契约。
- `app/rust/src/remote_scan/persistence.rs`：扫描状态、目录清单、generation、依赖和检查点的 SQL 操作。
- `app/rust/src/remote_scan/engine.rs`：有界队列、优先级、背压、取消、重试和扫描数据流。
- `app/rust/src/api/remote_scan.rs`：Dart 可调用的 start/status/pause/resume/cancel/rescan API。
- `app/rust/src/document/remote_folder.rs`：远程图片文件夹 `Document`。

### Rust 修改

- `app/rust/src/lib.rs`、`app/rust/src/api/mod.rs`、`app/rust/src/document/mod.rs`：注册新模块。
- `app/rust/src/db/mod.rs`、`app/rust/src/api/db.rs`：additive schema migration、DTO 和查询。
- `app/rust/src/reader.rs`：新增最低优先级 scan lane，并保留前台槽位。
- `app/rust/src/api/source.rs`、`app/rust/src/source/mod.rs`：复用 provider session/ByteSource，接入 adapter 和图片文件夹打开。
- `app/rust/src/api/cache.rs`：按逻辑键及封面依赖清理 stale asset。
- `app/rust/src/source/webdav.rs`、`sftp.rs`、`baidu.rs`、`cloud115.rs`、`quark.rs`：只补 adapter 所需的结构化能力/错误映射，不复制下载逻辑。
- `app/rust/src/frb_generated.rs`：由 codegen 生成，禁止手改。

### Dart 新建

- `app/lib/store/remote_scan_models.dart`：状态、进度、资产类型和错误展示模型。
- `app/lib/store/remote_scan_coordinator.dart`：来源生命周期、去重、触发和控制。
- `app/lib/ui/remote_scan_status.dart`：来源页/全局进度 UI。

### Dart 修改

- `app/lib/store/folder_snapshot_store.dart`、`remote_listing.dart`、`library_index_service.dart`：快照 v2、size/mtime/asset kind 和 engine 结果消费。
- `app/lib/ui/source_browser.dart`、`source_tree.dart`、`comic_cover.dart`：卡片状态、缓存优先和图片文件夹入口。
- `app/lib/ui/reader_page.dart`、`app/lib/store/remote_cache_cleanup.dart`、`app/lib/ui/cache_manager.dart`：图片文件夹 Reader 及完成/失效清理。
- `app/lib/ui/home_page.dart`：`remoteBackgroundScanEnabled` 设置与手动入口。
- `app/lib/src/rust/api/remote_scan.dart`、其他 FRB Dart 绑定：由 codegen 生成。

### 测试和证据

- Rust 模块内 `#[cfg(test)]`：model、persistence、engine、remote folder、reader governor、cache cleanup。
- `app/test/remote_scan_manifest_test.dart`、`remote_scan_coordinator_test.dart`、`remote_scan_status_test.dart`、`remote_folder_reader_test.dart`、`remote_cover_dependency_test.dart`。
- `app/rust/tests/remote_scan_contract.rs`：跨模块 fake provider contract。
- `docs/reports/rch-remote-cloud-scan-<date>.md`：真实 provider/性能证据；不保存秘密或用户原文件。

---

### Task 1: 清单模型、Provider Adapter 与数据库迁移

**Files:**

- Create: `app/rust/src/remote_scan/model.rs`, `adapter.rs`, `persistence.rs`, `mod.rs`
- Modify: `app/rust/src/lib.rs`, `app/rust/src/db/mod.rs`, `app/rust/src/api/db.rs`
- Modify: `app/lib/store/folder_snapshot_store.dart`, `remote_listing.dart`, `library_index_service.dart`
- Test: `app/rust/src/remote_scan/model.rs` tests, `app/rust/tests/remote_scan_contract.rs`, `app/test/remote_scan_manifest_test.dart`

**Interfaces:**

- Produces `RemoteAssetKind::{ArchiveFile, ImageFile, ImageFolder, ContainerDir, PlainDir, Other}`.
- Produces `RemoteEntry { name, logical_path, is_dir, size: Option<u64>, mtime: Option<i64>, asset_kind }`.
- Produces `RemoteProviderAdapter` methods `list(path, cursor)`, `read_range(path, offset, length)`, `read_file_limited(path, max_bytes)`, `normalize_path(path)` and `capabilities(path, fingerprint)`.
- Produces `RemoteScanState { source_id, status, mode, generation, checkpoint, last_success_at, error_code }` and `RemoteCoverDependency { book_key, dependency_path, dependency_fingerprint, profile, status }`.

- [ ] **Step 1: Write classification and fingerprint tests**

  Add tests that assert direct image children classify a directory as `ImageFolder`, archive children classify only the parent as `ContainerDir`, hidden/system files are ignored, natural sort selects `01.jpg` before `2.jpg`, and identical normalized entries produce the same fingerprint.

- [ ] **Step 2: Run the focused Rust/Dart tests and verify they fail**

  Run `cd app/rust; cargo test remote_scan -- --test-threads=1` and `cd ../..; flutter test --no-pub test/remote_scan_manifest_test.dart`. Expected result: missing model/module or failing assertions, with no production fallback added.

- [ ] **Step 3: Add the typed model and adapter contract**

  Define the enums and structs above in `model.rs`; keep adapter methods synchronous so engine workers can call existing clients inside `spawn_blocking`. Map provider-specific failures to `RemoteScanError` instead of comparing error strings in Dart. Export the module from `lib.rs`.

- [ ] **Step 4: Add additive SQLite migration and transactional persistence**

  Extend `library_index` with `asset_kind`, `content_fingerprint`, `scan_generation`, and `listing_complete` using the repository’s existing schema migration pattern. Add `remote_scan_state`, `remote_listing_state`, and `remote_cover_dependency` with indexes on `(source_id, logical_path)` and `(book_key, dependency_path)`. Implement `upsert_complete_listing`, `load_checkpoint`, `mark_scan_status`, and `replace_verified_children`; never delete old rows when `complete=false`.

- [ ] **Step 5: Preserve metadata in Dart snapshots and index inputs**

  Upgrade `FolderSnapshotEntry` JSON to v2 with nullable `size`, `mtime`, `assetKind`, and `fingerprint`. Update `listRemoteDirFor` to retain `DirEntry.size/mtime`; update `LibraryIndexService` to write the normalized fields and distinguish image-folder directories from plain containers. Read v1 snapshots without using them as deletion evidence.

- [ ] **Step 6: Run focused tests and migration idempotency checks**

  Run `cd app/rust; cargo test remote_scan -- --test-threads=1`, `cargo test db:: -- --test-threads=1`, and `cd ../..; flutter test --no-pub test/remote_scan_manifest_test.dart test/folder_snapshot_store_test.dart`. Expected result: PASS, with a second migration invocation leaving schema and row counts unchanged.

- [ ] **Step 7: Record a checkpoint diff**

  Run `git diff --check` and inspect `git diff --stat -- app/rust/src/remote_scan app/rust/src/db app/lib/store`. Do not stage unrelated existing changes; task-local commits may contain only this task's files, while tag/release/push remain forbidden.

### Task 2: 共享调度、扫描生命周期与封面批处理

**Files:**

- Create: `app/rust/src/remote_scan/engine.rs`, `app/rust/src/api/remote_scan.rs`
- Modify: `app/rust/src/reader.rs`, `app/rust/src/api/mod.rs`, `app/rust/src/api/source.rs`, provider source files
- Modify: `app/lib/store/remote_scan_models.dart`, `remote_scan_coordinator.dart`, `app/lib/main.dart`
- Test: engine tests, `app/rust/tests/remote_scan_contract.rs`, `app/test/remote_scan_coordinator_test.dart`

**Interfaces:**

- Produces Rust FRB calls:

  ```rust
  pub async fn remote_scan_start(source_type: String, source_id: String, session: u64, root_path: String, mode: String) -> Result<RemoteScanJobDto, String>;
  pub fn remote_scan_status(source_id: String) -> Option<RemoteScanStatusDto>;
  pub fn remote_scan_pause(source_id: String) -> Result<(), String>;
  pub fn remote_scan_resume(source_id: String) -> Result<(), String>;
  pub fn remote_scan_cancel(source_id: String) -> Result<(), String>;
  ```

- `RemoteScanCoordinator.ensureForSession(source, session)` returns a deduplicated `Future<RemoteScanStatus>`; `rescan(source, mode)` accepts `incremental` or `full`.
- `RequestPriority::Scan` is lower than `Cover` and cannot consume the reserved foreground slot.

- [ ] **Step 1: Write governor and engine state-machine tests**

  Test that a foreground permit starts while scan work is queued, scan queue capacity rejects the next item, duplicate task keys coalesce, cancellation prevents commit, 429 honors a bounded retry schedule, and a complete directory creates cover tasks without collecting the full tree in memory.

- [ ] **Step 2: Run governor/engine tests and verify failure**

  Run `cd app/rust; cargo test reader::tests:: -- --test-threads=1; cargo test remote_scan -- --test-threads=1`. Expected result: missing `Scan` priority/engine symbols or failing new assertions.

- [ ] **Step 3: Add the scan priority and bounded worker**

  Extend `RequestPriority` and queue indexes in `reader.rs` with `Scan` below `Cover`. Implement a worker that consumes one `RemoteEntryBatch` at a time, uses a cancellation token checked before DB commit, applies provider rate gates, and stores a checkpoint after each complete directory. Keep retry policy explicit for `rateLimited` and `transient` only.

- [ ] **Step 4: Implement provider adapter factories**

  Add one adapter constructor per existing provider/session path in `source.rs`. Reuse `webdav::WebDavClient`, `sftp::SftpClient`, `baidu::BaiduClient`, `cloud115::{Cloud115Client, Cloud115WebClient}`, and `quark::QuarkClient`; map their native entries into `RemoteEntry` without logging credentials or full URLs.

- [ ] **Step 5: Add full/incremental engine transitions**

  On `never_started`, enqueue root full scan; on `complete`, compare root and enqueue only changed/new/uncompleted directories. Commit missing children only after all list pages have `complete=true`; retain the previous generation on partial/error/cancel. Make repeated start calls join the existing source job.

- [ ] **Step 6: Expose FRB control/status APIs and Dart coordinator**

  Implement the exact API signatures above, regenerate bindings with `cd app; .\codegen.ps1`, and make `RemoteScanCoordinator` listen for successful `remoteSessionFor`/root-open events. Debounce repeated triggers, persist the last observed status, and resume jobs after app restart.

- [ ] **Step 7: Add cover batch path and cache-only card consumption**

  After classification, enqueue one cover task per `archive_file` and `image_folder`; use the existing safe partial cover functions and write `remote_cover_dependency`. Keep `ComicCover` network-free for generated entries by first reading local cover cache; a visible fallback submits the same task key.

- [ ] **Step 8: Run focused scheduler/coordinator tests**

  Run `cd app/rust; cargo test remote_scan reader:: -- --test-threads=1`, then `cd ../..; flutter test --no-pub test/remote_scan_coordinator_test.dart test/comic_cover_scheduler_test.dart`. Expected result: PASS with assertions for one trigger, priority ordering, dedupe, pause/resume, cancellation, and no secret-bearing logs.

- [ ] **Step 9: Regenerate and inspect bindings**

  Run `cd app; .\codegen.ps1`; inspect only generated `app/lib/src/rust/api/remote_scan.dart` and related generated diffs. Run `git diff --check`; do not hand-edit generated files.

### Task 3: 远程图片文件夹 Document 与 Reader 接入

**Files:**

- Create: `app/rust/src/document/remote_folder.rs`
- Modify: `app/rust/src/document/mod.rs`, `app/rust/src/api/source.rs`, `app/rust/src/api/book.rs`, `app/rust/src/reader.rs`
- Modify: `app/lib/ui/source_browser.dart`, `app/lib/ui/reader_page.dart`, `app/lib/store/remote_cache_cleanup.dart`
- Test: `app/rust/src/document/remote_folder.rs` tests, `app/test/remote_folder_reader_test.dart`

**Interfaces:**

- Produces `RemoteFolderBook::open(entries: Vec<RemoteImageEntry>, reader: Arc<dyn RemoteFolderReader>, title: String) -> Result<Self>` implementing `Document`.
- `RemoteImageEntry { logical_path, name, size: Option<u64>, mtime: Option<i64>, fingerprint }` is sorted once at open and has a stable page count.
- Provider opening returns the same `BookInfo { handle, title, page_count }` shape used by archive Reader sessions.

- [ ] **Step 1: Write remote-folder Document tests**

  Test page count, natural ordering, hidden-file exclusion, cover candidate selection, out-of-range page errors, cancellation before read, and a page body larger than `max_page_bytes` returning a typed limit error.

- [ ] **Step 2: Run the new Document tests and verify failure**

  Run `cd app/rust; cargo test remote_folder -- --test-threads=1`. Expected result: missing module/type errors.

- [ ] **Step 3: Implement the bounded per-image reader**

  Add `RemoteFolderBook` that calls the adapter for one image at a time. Use random/Range reads when available; otherwise enforce `max_page_bytes` while consuming the single image response. Never create an archive raw-cache path for a folder.

- [ ] **Step 4: Wire folder opening into source APIs**

  In `open_*_book` provider paths, branch on indexed `image_folder` before archive extension handling, pass the committed child manifest and session adapter, and preserve the existing logical cache namespace. Keep archive `stream/download/auto` branches unchanged.

- [ ] **Step 5: Connect SourceBrowser and Reader UI**

  Let a remote `image_folder` card open through the normal reader route; do not reconstruct a credential-less `remoteOnly` source. Reuse current-page/next-page loading and record the folder logical path as the read key.

- [ ] **Step 6: Add completion cleanup coverage**

  Extend `RemoteBookUseRegistry` and `purgeRemoteBookContentCache` to delete folder page cache only, preserve the generated cover, and leave metadata/tags/history/custom cover parameters untouched.

- [ ] **Step 7: Run targeted Reader tests**

  Run `cd app/rust; cargo test remote_folder reader:: -- --test-threads=1`, then `cd ../..; flutter test --no-pub test/remote_folder_reader_test.dart test/remote_cache_cleanup_test.dart`. Expected result: PASS with no whole-folder raw download and preserved cover after completion.

### Task 4: 封面依赖、tombstone 与失效缓存清理

**Files:**

- Modify: `app/rust/src/remote_scan/persistence.rs`, `app/rust/src/db/mod.rs`, `app/rust/src/api/cache.rs`, `app/rust/src/db/mod.rs`
- Modify: `app/lib/store/library_store.dart`, `app/lib/store/remote_cache_cleanup.dart`, `app/lib/ui/cache_manager.dart`
- Test: Rust cache/db tests, `app/test/remote_cover_dependency_test.dart`, `app/test/library_store_cache_target_test.dart`

**Interfaces:**

- Produces `purge_verified_remote_asset(source_id, logical_path, dependency_paths)` which is callable only with a complete-listing proof.
- Consumes `remote_cover_dependency` and existing `purge_stale_book_cache`/`purge_remote_book_content_cache` semantics.

- [ ] **Step 1: Write deletion safety tests**

  Test complete listing deletion removes a folder cover, cover aliases, child page/raw and dependency cache; partial listing, 403, auth expiry, cancellation, network error and one-file 404 leave all old data; unrelated source/path remains untouched.

- [ ] **Step 2: Run deletion tests and verify failure**

  Run `cd app/rust; cargo test cache:: db:: -- --test-threads=1`, then `cd ../..; flutter test --no-pub test/remote_cover_dependency_test.dart test/library_store_cache_target_test.dart`. Expected result: new assertions fail until proof-aware cleanup exists.

- [ ] **Step 3: Add proof-carrying tombstone persistence**

  Have `replace_verified_children` require `listing_complete=true`, source session validity and the current generation. Store tombstones only for missing children of that verified directory; keep old rows for all other outcomes.

- [ ] **Step 4: Expand Rust cleanup by logical key and dependency**

  Update cache cleanup to delete folder logical cover keys, raw-path aliases, dependent image paths and page/raw namespaces together. Preserve custom cover metadata and stable cover when the book remains live.

- [ ] **Step 5: Integrate LibraryStore stale cleanup**

  Make remote alignment consume engine tombstones and call the proof-aware Rust API; remove any path-prefix cleanup that can run after a failed refresh. Keep existing `alignFailed` warning behavior.

- [ ] **Step 6: Verify reading cleanup remains separate**

  Assert `purge_remote_book_content_cache` never deletes cover for a live book, while `purge_verified_remote_asset` does delete cover only after a verified remote deletion. Run the focused Rust/Dart commands again and inspect cache paths in a temporary test root.

### Task 5: Dart 触发 UI、设置和跨 provider 验证

**Files:**

- Create: `app/lib/store/remote_scan_models.dart`, `app/lib/store/remote_scan_coordinator.dart`, `app/lib/ui/remote_scan_status.dart`
- Modify: `app/lib/ui/home_page.dart`, `app/lib/ui/source_browser.dart`, `app/lib/ui/source_tree.dart`, `app/lib/ui/comic_cover.dart`
- Test: `app/test/remote_scan_status_test.dart`, `app/test/remote_scan_coordinator_test.dart`, widget tests for source browser/home settings

**Interfaces:**

- `RemoteScanCoordinator` exposes `ensureForSession`, `pause`, `resume`, `cancel`, `rescanIncremental`, `rescanFull`, and a `ValueListenable<RemoteScanViewState>` per source.
- `RemoteScanViewState` carries `status`, `mode`, `processed`, `total`, `lastSuccess`, `errorCode`, and `coverFetchPaused`.

- [ ] **Step 1: Write UI and setting tests**

  Assert first auth/root open calls `remote_scan_start(..., "full")` once, a later open calls `"incremental"`, manual actions select the requested mode, duplicate taps join one job, and `remoteBackgroundScanEnabled=false` suppresses automatic starts without removing existing covers.

- [ ] **Step 2: Run Flutter tests and verify failure**

  Run `flutter test --no-pub test/remote_scan_coordinator_test.dart test/remote_scan_status_test.dart test/app_settings_parse_test.dart`. Expected result: missing coordinator/state/settings symbols.

- [ ] **Step 3: Add persisted setting with backward-compatible default**

  Add `remoteBackgroundScanEnabled` to the existing settings parse/serialize flow with default `true`; retain `remoteCoverFetchEnabled` as the total remote-cover network gate. Explicit `false` must survive restart and migration.

- [ ] **Step 4: Build source and global status surfaces**

  Add a non-blocking status banner/panel with processed counts, pause/resume, retry, incremental rescan and full rescan. Show Range-unavailable and degraded errors without exposing URLs or credentials. Keep card rendering cache-first.

- [ ] **Step 5: Connect session/root lifecycle hooks**

  Call `ensureForSession` after successful `remoteSessionFor` and after the first root listing; reuse the root listing as the first manifest page to avoid a duplicate request. Resume persisted jobs when the app starts.

- [ ] **Step 6: Add kill switch and compatibility behavior**

  When the kill switch is off, stop automatic queue submission and preserve the old visible-card on-demand path. Turning off either setting must not delete existing cover cache, metadata, history or custom cover parameters.

- [ ] **Step 7: Run widget and regression tests**

  Run `flutter test --no-pub test/remote_scan_status_test.dart test/remote_scan_coordinator_test.dart test/comic_cover_scheduler_test.dart test/app_settings_parse_test.dart test/remote_cache_cleanup_test.dart` and `flutter analyze --no-pub`. Expected result: PASS with status transitions and settings compatibility.

### Task 6: FRB regeneration, provider contract integration and release evidence

**Files:**

- Modify generated bindings through `app/codegen.ps1` only.
- Create: `app/rust/tests/remote_scan_contract.rs`, `rch-remote-cloud-scan-2026-09-14.md`
- Modify: `.trellis/spec/backend/remote-cover-update-contracts.md`, `.trellis/spec/backend/source-refresh-cleanup.md`, historical-task note in `08-02-cover-loading-perf` only by adding a superseding reference (do not rewrite history).

**Interfaces:**

- Consumes all previous task APIs and fake adapter fixtures.
- Produces reproducible commands, provider capability matrix, pending evidence labels and final release recommendation.

- [ ] **Step 1: Add fake provider matrix**

  Implement one fake adapter fixture for WebDAV, SFTP, Baidu, 115 and Quark behavior, covering complete/partial pagination, missing mtime, 206, 200, 416, malformed Content-Range, 429, 403, auth expiry, nested image folders and verified deletion.

- [ ] **Step 2: Run the matrix before live checks**

  Run `cd app/rust; cargo test --test remote_scan_contract -- --test-threads=1` and the corresponding Flutter fake coordinator/widget suites. Expected result: all provider contract outcomes are typed and no secret-bearing log line is emitted.

- [ ] **Step 3: Regenerate FRB and run project gates**

  Run exactly:

  ```text
  cargo fmt --all -- --check
  cargo clippy --workspace --all-targets --all-features -- -D warnings
  cargo test --workspace --locked -j 1 -- --test-threads=1
  flutter analyze --no-pub
  flutter test --no-pub
  dart run tool/startup_contract_check.dart
  flutter build apk --debug
  flutter build windows
  git diff --check
  ```

  Classify fmt/Clippy output as new, existing dirty, or historical; fix only new findings.

- [ ] **Step 4: Run real provider evidence without secrets**

  Use existing authorized sessions or process-only environment input. Record provider, path hash, status, range, bytes, wall time, throughput, RSS, retry/recovery and SHA-256; never record credentials, full URL or user originals. Missing provider/device/sample evidence is `PENDING`, not PASS.

- [ ] **Step 5: Run cache/read/long-strip smoke**

  Verify reading completion retains covers and metadata, verified remote deletion removes stale cover, 50+ page image folders read through the emulator without foreground starvation, and 20/100/300 MiB fixture measurements stay within the project memory threshold.

- [ ] **Step 6: Update stable specs and task context**

  Add the finalized adapter, tombstone and cache-retention invariants to `.trellis/spec/backend/remote-cover-update-contracts.md` and `.trellis/spec/backend/source-refresh-cleanup.md`; add the design/plan paths to the parent task context manifests after the task enters `in_progress`.

- [ ] **Step 7: Produce the evidence report and final Git boundary check**

  Write the report sections `Executive Status`, `Environment`, `Provider Matrix`, `Scan Evidence`, `Reader/Cache Evidence`, `Automated Verification`, `Pending External Validation`, `Git Boundary`, and `Final Recommendation`. Run `git status --short` and `git diff --check`; do not create a tag, release or push. Task-local commits remain limited to the reviewed task files.

## Plan Self-Review

- Spec coverage: Sections 1–5 map to Tasks 1–5; provider/error/security and release evidence map to Task 6; M8 and SMB exclusions are repeated in the global constraints and Task 6.
- Placeholder scan: every step names concrete files, interfaces, tests and commands; no unbounded future-work wording is used.
- Type consistency: `RemoteAssetKind`, `RemoteEntry`, `RemoteScanState`, `RemoteCoverDependency`, `RemoteProviderAdapter`, `RemoteFolderBook`, `RequestPriority::Scan`, `remote_scan_start`, and `RemoteScanCoordinator` are defined once and reused by later tasks.
- Safety review: complete-listing proof is required before tombstone cleanup; live reading cleanup remains cover-preserving; Range failure never selects a whole-book background download path.
