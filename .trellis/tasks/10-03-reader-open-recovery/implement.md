# 阅读器打开与失败恢复实施计划

> **For agentic workers:** 本计划按任务顺序逐项实现，并在每项结束时做静态检查。此轮不新增或运行自动化测试；不要提交代码，提交由 Trellis 收尾阶段统一处理。

**Goal:** 在阅读器中展示连接、打开和首屏阶段，提供可恢复的打开和单页失败处理，并收紧异步结果与页面控制器的生命周期。

**Architecture:** 路由先进入 ReaderPage，再由无 widget/context 的 store 层 `BookOpenCoordinator` 负责 session、远端图片目录判断、provider open API 和下载进度。ReaderPage 持有 UI 阶段与操作 generation，所有迟到结果按 generation 丢弃或关闭句柄；单页加载状态和 PhotoView 生命周期留在页面内。

**Tech Stack:** Flutter/Dart、Rust FRB source API、PhotoView。

**Spec:** `.trellis/tasks/10-03-reader-open-recovery/prd.md`、`.trellis/tasks/10-03-reader-open-recovery/design.md`

## Global Constraints

- 打开阶段至少区分连接书源、打开漫画/下载、加载首屏和阅读就绪；只在 provider 报告有效下载进度时显示百分比（`1.0` 是空闲标记）。
- 离开等待页不会承诺中断 provider 的阻塞式网络操作；晚到的成功 `BookInfo.handle` 必须调用 `close_book`。
- 每页最多 3 次自动请求（首次 + 2 次重试）；最终失败需用户显式点击重试，widget 重建不得重置次数。
- 控制器只在关联 PhotoView 子树卸载后释放，并始终保留当前页及相邻页。
- 日志沿用 `reader_diag.log` 的 key=value 格式，不记录路径、书名、URL 或凭据。
- 资料库路径保持逻辑身份；Quark/百度/115 文件操作必须先按 source id 和逻辑路径解析 provider 文件标识，不能依赖标识格式猜测。
- ReaderPage 的 AI 状态通知不得在 widget tree 锁定期间发出；延迟清理必须按 owner 身份校验。
- 不新增或运行测试；实现结束时只运行 Rust 编译检查、FRB 绑定生成和 Flutter 静态分析。
- 不在本计划执行期间创建提交；遵从 Trellis Phase 3.4 的一次性提交确认流程。

## 文件与边界

| 文件 | 责任 |
|---|---|
| `app/lib/store/book_open_coordinator.dart` | 新增 session/provider/open/进度协调，不依赖 widget/context |
| `app/lib/ui/opener.dart` | 保留阅读记录语义并立即 push ReaderPage |
| `app/lib/ui/reader_page.dart` | 阶段 UI、重试、页面加载状态、generation、PhotoView 控制器 |
| `app/rust/src/api/source.rs` | 修正 Quark stream/fallback 耗时并暴露受限诊断写入 API |
| `app/rust/src/api/library.rs`、`app/lib/store/library_catalog.dart` | 将资料库逻辑路径解析为当前 provider 文件标识 |
| `app/lib/store/book_open_coordinator.dart` | 在统一阅读打开边界解析并保留实际 provider 路径 |
| `app/lib/store/ai_upscale_manager.dart`、`app/lib/ui/book_detail_page.dart`、`app/lib/ui/cover_editor_page.dart`、`app/lib/ui/comic_cover.dart` | 让直接远程读取入口共用路径解析 |
| `app/lib/ui/reader_page.dart` | 以 provider 路径清理 raw 包；owner-scoped、post-frame 清理 AI 阅读状态 |
| `app/lib/src/rust/api/source.dart`、`app/lib/src/rust/frb_generated.dart` | 由 FRB 生成，不手改 |

## Review Focus

- session 获取、图片目录 manifest、各 provider open 失败均在已 push 的 ReaderPage 内显示可重试错误；检查 Task 1 的状态转移和错误路径。
- 用户离开或重试后，旧回调不写入新状态；晚到 handle 关闭且新操作可继续；检查 Task 1 的 generation 所有权。
- 当前页第三次失败后出现单页重试操作，build/预取不会再次自动启动；检查 Task 2 的状态转移。
- 快速翻页与跳页时 PhotoView 子树仍在树内的控制器不被提前 dispose；检查 Task 3 的 post-frame identity/window 判定。
- Quark stream 失败回退中三个时间字段边界清楚，诊断字段不含用户内容；检查 Task 4 的计时点与参数白名单。
- 索引路径只有在 source fingerprint 与当前有效 route 匹配时才转换为 provider id；source browser 的原始 provider path 仍可直开。
- 阅读记录/元数据/manifest 保留逻辑路径，而 raw 缓存打开与清理使用同一个 provider path。
- ReaderPage 卸载后的 AI 状态清理只影响其 owner，并在 post-frame 通知。

---

### Task 1: 打开协调器与立即进入阅读器

**Files:**
- Create: `app/lib/store/book_open_coordinator.dart`
- Modify: `app/lib/ui/opener.dart`
- Modify: `app/lib/ui/reader_page.dart`

**Interfaces:**
- `enum BookOpenStage { connecting, openingBook, loadingFirstPage, ready, failed }`。
- `BookOpenCoordinator.open({required BookSource? source, required String path, required String title, required BookOpenStrategy strategy, required bool Function() isActive, required void Function(BookOpenStage stage) onStage, required void Function(double progress) onProgress, BigInt? existingSession}) -> Future<BookOpenResult>`。
- `BookOpenResult` 持有 `BookInfo` 与 `remoteImageFolder` 标记；session 只在 coordinator 内部使用，coordinator 不持有 BuildContext、State 或 Navigator。

- [x] 在新 coordinator 中集中迁移 session 获取和 provider 分派，覆盖本地、WebDAV、SFTP、百度、115、夸克及远端图片目录；保留策略与下载进度 API，AI 缓存绕过继续由 ReaderPage 控制。
- [x] 在 `opener.dart` 保留初始页恢复语义并立即 push ReaderPage；阅读记录在 ReaderPage 进入 openingBook 阶段时记录。
- [x] 在 ReaderPage 实现阶段状态展示与有效下载百分比；session、manifest、book open 失败保留错误和“重新打开”操作，显式重试启动新 generation。
- [x] 每次打开、重试和 dispose 更新 generation；回调只有在 generation 仍有效且 State mounted 时才写状态。若结果已过期但包含 BookInfo，立即调用 `close_book(handle: ... )` 并丢弃结果。
- [x] 将 remote image-folder 判断结果送入原有 remote use lease 生命周期；页面退出后停止进度轮询和 Dart 状态回调，不声称中断底层阻塞 I/O。

### Task 2: 有界单页加载与首屏状态

**Files:**
- Modify: `app/lib/ui/reader_page.dart`

**Interfaces:**
- 每个 page index 维护 loading/success/failure/attempt count、最后错误和所属 book handle/generation。
- 首屏成功只在当前初始页图片确实可显示时将阶段切换为 `ready`；首屏最终失败保留可见的单页重试入口。

- [x] 将 `_loading` 集合扩展为单页状态记录；页面请求失败后自动最多重试到 3 次，首次失败后等待 250 ms、第二次失败后等待 750 ms，等待期间检查页面 generation 与 handle。
- [x] 最终失败后让 build 复用失败状态并渲染包含原因/重试操作的错误卡片；预取失败保留状态，build 不会重新启动请求。
- [x] 显式重试只重置指定页状态；任何旧请求的成功或失败结果只有在 handle 和 generation 与发起时一致时才允许写回。
- [x] 首屏成功后切到阅读就绪；显式重试成功清除该页错误，不触发其他页的重试计数重置。

### Task 3: PhotoView 控制器延后释放

**Files:**
- Modify: `app/lib/ui/reader_page.dart`

- [x] 翻页时先更新当前页 ±1 保留窗口，再用 `addPostFrameCallback` 调度清理；回调执行时重新检查保留窗口和 map 中 controller identity。
- [x] 只释放仍在 map 中、且执行时仍离开当前窗口的 PhotoView 与 scale-state controller；ReaderPage dispose 统一释放剩余控制器。
- [x] 检查跳页、双页模式切换、条漫模式和 dispose 都使用同一生命周期规则，不留下重复 dispose 路径。

### Task 4: 阅读诊断时间

**Files:**
- Modify: `app/rust/src/api/source.rs`
- Modify: `app/lib/ui/reader_page.dart`
- Regenerate: `app/lib/src/rust/api/source.dart` 及 FRB 公共生成文件

**Interfaces:**
- 新增受限 FRB 函数 `log_reader_timing(stage: String, source_type: String, result: String, elapsed_ms: i64)`；Rust 仅接受 `connect/book_open/first_page`、已知来源类别和 `success/error`，自行格式化并写入现有诊断文件，不接受任意日志行或用户路径。
- 事件格式沿用 `reader_diag.log` 的 key=value；毫秒非负。

- [x] 在 Quark stream fallback 进入下载前冻结 stream attempt 用时，独立计时下载，并将 total 计时结束点移到完整 open 结果返回处。
- [x] 实现受限诊断写入 FRB API，在 ReaderPage/coordinator 分别记录连接、整体 open 与首屏结果；记录来源类别、阶段、结果和毫秒，不传入书名、完整路径、URL 或凭据。
- [x] 以 `app/codegen.ps1` 生成并编译绑定；运行脚本前确认 RCH 未运行。核对旧 Quark 事件字段含义不变。

### Task 5: 静态验证与差异复核

**Files:**
- No additional source files.

- [x] 在 `app` 目录运行 `.\\codegen.ps1` 生成 Rust API 绑定并重建 release 库；先确认 RCH 已关闭。
- [x] 在 `app` 目录运行 `flutter analyze --no-pub`，预期退出码为 0。
- [x] 在 `app/rust` 目录运行 `$env:RUSTFLAGS="-D warnings"; cargo check --locked --all-targets`，预期退出码为 0。
- [x] 按 Review Focus 复核所有状态转移、generation 检查、句柄释放、单页失败卡片和计时边界；本任务不执行测试命令。

### Task 6: 远程资料库路径解析与失败页清理

**Files:**
- Modify: `app/rust/src/api/library.rs`
- Modify: `app/lib/store/library_catalog.dart`
- Modify: `app/lib/store/book_open_coordinator.dart`
- Modify: `app/lib/store/ai_upscale_manager.dart`
- Modify: `app/lib/ui/book_detail_page.dart`
- Modify: `app/lib/ui/cover_editor_page.dart`
- Modify: `app/lib/ui/comic_cover.dart`
- Modify: `app/lib/ui/reader_page.dart`
- Update: `.trellis/spec/backend/source-refresh-cleanup.md`
- Update: `.trellis/spec/frontend/state-management.md`
- Regenerate: Flutter Rust bridge bindings through `app/codegen.ps1`

**Interfaces:**
- `db_resolve_remote_provider_path(source_id: String, logical_path: String) -> Result<Option<String>, String>` returns only a current provider route for an indexed logical path; no route for an unindexed browser path means the caller may keep its original provider identifier.
- `LibraryCatalogStore.providerPathFor(source, path) -> Future<String>` centralizes route lookup and preserves the supplied path when it is already provider-facing.
- `BookOpenResult.providerPath` records the actual provider path used for the open, while ReaderPage continues to key reading state by `widget.path`.
- `AiUpscaleManager.setReadingBook` / `clearReadingBook` use an owner token so an old ReaderPage cannot clear a newer reader registration.

- [x] Add the current-preview and source-fingerprint/live-index guarded route lookup to `api/library.rs`, and expose it through the catalog store.
- [x] Resolve provider paths once in `BookOpenCoordinator`; preserve logical paths for manifest checks, records, and leases; return provider path with `BookOpenResult`.
- [x] Reuse the resolver in direct remote reads for cover fallback/editing and AI upscale; pass the resolved path to raw-cache cleanup.
- [x] Defer AI reading-state cleanup to post-frame and scope registration/cleanup by owner identity.
- [x] Regenerate FRB bindings, run `flutter analyze --no-pub` and `RUSTFLAGS="-D warnings" cargo check --locked --all-targets`, then review callsite coverage; do not run tests.


### Task 7: Windows PDFium runtime provisioning

**Files:**
- Create: `app/windows/prepare_pdfium.ps1`
- Modify: `app/windows/CMakeLists.txt`
- Modify: `.github/workflows/release.yml`
- Modify: `docs/development/setup.md`
- Modify: `app/.gitignore`

**Interfaces:**
- `prepare_pdfium.ps1` downloads the upstream Windows x64 archive and stages `pdfium.dll` plus its license under the ignored `app/windows/pdfium/win-x64/` cache.
- Windows CMake configure invokes the script if either staged runtime file is missing, then installs both next to `RCH.exe` in Debug, Profile, and Release. Configuration fails with the script output if provisioning fails, instead of producing an app that fails only when a PDF is opened.
- The release workflow relies on the same CMake provisioning path and no longer copies a second, independently downloaded DLL.

- [x] Implement an idempotent PowerShell provisioner that extracts and validates `bin/pdfium.dll` and `LICENSE` from the upstream archive.
- [x] Make Windows CMake provision and install PDFium for every build profile; keep the downloaded files out of git.
- [x] Remove the duplicate release download/copy step and document automatic first-build setup plus manual recovery.
- [x] Rebuild and launch Windows Debug; verify the built executable directory contains `pdfium.dll` and `pdfium-LICENSE.txt`. Do not run automated tests.

### Task 8: Quark authentication recovery and route error accuracy

**Files:**
- Modify: `app/lib/store/quark_session.dart`
- Modify: `app/lib/ui/reader_page.dart`
- Modify: `app/rust/src/api/library.rs`
- Update: `.trellis/spec/backend/quark-source.md`
- Update: `.trellis/spec/backend/source-refresh-cleanup.md`
- Update: `docs/development/setup.md` (replace stale PDFium manual-copy FAQ)

**Interfaces:**
- ReaderPage recognizes only Quark authentication-expiry responses, opens the existing QR login dialog, persists the returned Cookie, clears the cached session, and reopens once. Cancellation keeps the original failure visible; non-auth errors (404 and route/index errors) never trigger the scanner.
- The remote provider-path resolver treats only indexed `ArchiveFile` items as requiring a persisted provider route; directory and unclassified entries do not produce a misleading missing-book-mapping error.

- [x] Add a narrow Quark auth-expiry classifier and one-shot QR recovery for book-open and page-read failures, with manual retry allowing another scan.
- [x] Restrict the Rust missing-route guard to readable archive files and keep provider paths for other callers intact.
- [x] Update Quark/PDFium troubleshooting docs and source-auth contract.
- [x] Run static analysis and Windows Debug build; do not add or run automated tests. Keep the desktop app available after rebuilding.

### Task 9: Apply route fix to the loaded Rust runtime and make refresh actionable

**Files:**
- Modify: `app/lib/ui/reader_page.dart`
- Modify: `app/lib/ui/source_browser.dart`
- Modify: `app/lib/ui/home_page.dart`
- Rebuild: `app/rust/target/release/rust_lib_app.dll` via `app/codegen.ps1`

**Interfaces:**
- A missing Quark route error offers an in-place full source scan with progress feedback, then retries opening. Authentication expiry during refresh still offers QR login.
- Both the source-list refresh and the current-directory refresh provide visible success/failure feedback; the source-list action is clearly a local projection reload.
- Windows Debug must load the rebuilt release Rust DLL configured by FRB, not only the separate Cargokit Debug plugin artifact.

- [x] Add an actionable Quark index refresh/retry flow to the Reader error card, including scan status/error feedback.
- [x] Add visible feedback and accurate tooltips to source-list and source-browser refresh actions.
- [x] Stop RCH, regenerate bindings/build the FRB release DLL with `app/codegen.ps1`, rebuild and relaunch Windows Debug, then confirm loaded DLL timestamps are current.

### Task 10: Bound webtoon reads and stabilize distant page jumps

**Files:**
- Modify: `app/lib/ui/reader_page.dart`
- Modify: `app/lib/ui/webtoon_navigation.dart`
- Update: `.trellis/spec/frontend/state-management.md`

**Contracts:**
- Intermediate children built to resolve a distant `ListView` offset do not start page reads outside the active `webtoonPageLoadRadius`.
- Placeholder extents use the same estimate as jump-offset calculation but are excluded from measured image-height averages.
- A programmatic target remains active through the first layout and is cancelled by a newer target or user drag.

- [x] Bound page reads to the viewport/jump target window in the webtoon builder.
- [x] Measure only real image extents and use the model estimate for unloaded placeholder heights.
- [x] Complete instant jump intents after first-frame layout; ignore programmatic scroll-end notifications.
- [x] Run Flutter static analysis, review the diff, and rebuild/relaunch Windows Debug; do not add or run automated tests.

#### Task 10 follow-up: Reproduced jump and page-index failure

**Files:**
- Modify: `app/lib/ui/reader_page.dart`
- Modify: `app/lib/ui/webtoon_navigation.dart`
- Modify: `app/lib/ui/book_detail_page.dart`
- Update: `.trellis/spec/frontend/state-management.md`

- [x] Keep ordinary ReaderPage reads at three in flight, allow one focused jump target to enter the Rust priority queue beyond that budget, and discard queued reads outside the active window.
- [x] Freeze the unknown webtoon extent estimate once navigation starts so a new image measurement cannot resize all distant placeholders.
- [x] Defer webtoon jumps when the ScrollController has not attached yet; reconcile the page mapping after image measurement and anchor correction.
- [x] Change E-Hentai tag hiding from unlinking source tags to a reversible visibility toggle that keeps its control available.
- [x] Run `flutter analyze --no-pub`; do not add or run automated tests. Rebuild/relaunch Windows Debug and leave it available for user verification.

#### Task 10 follow-up: Prioritize jump targets and reveal page bytes progressively

**Files:**
- Modify: `app/lib/ui/reader_page.dart`
- Modify: `app/rust/src/reader.rs`
- Modify: `app/rust/src/api/book.rs`
- Modify: `app/rust/src/api/source.rs`
- Regenerate: `app/lib/src/rust/api/book.dart` and FRB bindings through `app/codegen.ps1`
- Update: `.trellis/tasks/10-03-reader-open-recovery/prd.md`
- Update: `.trellis/tasks/10-03-reader-open-recovery/design.md`
- Update: `.trellis/spec/frontend/state-management.md`
- Update: `.trellis/spec/backend/logging-guidelines.md`

**Contracts:**
- Current visible pages use the foreground API; bounded offscreen pages use `book_page_prefetch` and the existing `Prefetch` request priority.
- Rust queued prefetch work must not claim the page inflight before it receives a governor permit. Same-page work already in synchronous I/O remains shared and non-cancellable.
- If the visible target is already being fetched by a Dart prefetch request, a newer foreground generation supersedes its response and can enter the priority queue.
- Keep the normal Dart page-read budget at three, with at most one urgent visible target using the fourth slot; the Rust governor remains the remote concurrency boundary.
- The original page bytes appear immediately. AI cache lookup is best-effort and can replace the image after a cache hit.
- The toolbar random-reading action uses `Icons.casino_outlined`, matching the home page.
- Visible-page loading indicators and page timing logs exclude offscreen prefetch; logs do not include book identity or page number.

- [x] Split Rust foreground and prefetch page APIs and route neighbors through `RequestPriority::Prefetch`.
- [x] Make queued low-priority requests relinquish page ownership until they receive governor capacity; retain deduplication for active reads.
- [x] Prioritize the current spread/jump target, promote in-flight Dart prefetch when it becomes visible, and stop retrying failed reads outside the active window.
- [x] Display original bytes before optional AI-cache lookup; scope spinner to the visible page/spread and match the home-page dice icon.
- [x] Add allowlisted, identity-free foreground page timing events to `reader_diag.log`.
- [x] Regenerate FRB and build Release Rust DLL via `app/codegen.ps1`; run `RUSTFLAGS=-D warnings cargo check --locked --all-targets` and `flutter analyze --no-pub`, rebuild and relaunch Windows Debug; do not add or run automated tests.
- [x] Record the visible-page priority and logging contracts in the frontend/backend specs.
