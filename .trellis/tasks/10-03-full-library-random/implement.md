# 全库随机阅读实施计划

> **For agentic workers:** 本计划按任务顺序逐项实现，并在每项结束时做静态检查。此轮不新增或运行自动化测试；不要提交代码，提交由 Trellis 收尾阶段统一处理。

**Goal:** 让“随机一本”从本机配置书源的完整、已发布漫画索引中选书，并在首页与阅读结束入口共享最近选择历史。

**Architecture:** 以 Rust 的 `library_index` 查询作为随机池唯一来源。为本地目录补充漫画类型标记，由 Rust SQLite API 做范围过滤和随机批量读取，Dart 在数据库锁外校验本地路径并执行历史排除、循环和入口复用。

**Tech Stack:** Rust / rusqlite / flutter_rust_bridge、Flutter/Dart、SQLite。

**Spec:** `.trellis/tasks/10-03-full-library-random/prd.md`、`.trellis/tasks/10-03-full-library-random/design.md`

## Global Constraints

- 随机池覆盖配置中的 local、SMB 和云端书源；排除 `remote_only = 1`、已删除、普通目录、非漫画文件及远端未完整发布条目。
- 随机请求只读取已提交索引，不触发网络扫描；SQLite 锁内不访问本地文件系统。
- 目录漫画判断沿用 FolderBook 的图片扩展名和隐藏文件规则；只有归档容器和确认的图片目录可入池。
- `asset_kind` 用加法迁移；远程扫描字段和 staging/publish 语义必须保留。
- 所有入口共享最近 10 个规范漫画键；只有候选池都在历史中时才从最旧记录开始循环。
- 不新增或运行测试；实现结束时只运行 Rust 编译检查、FRB 绑定生成和 Flutter 静态分析。
- 不在本计划执行期间创建提交；遵从 Trellis Phase 3.4 的一次性提交确认流程。

## 文件与边界

| 文件 | 责任 |
|---|---|
| `app/rust/src/db/mod.rs` | 基础表迁移、索引行字段、查询与 upsert/sync 保留 |
| `app/rust/src/catalog_context.rs`、`app/rust/src/rchpkg/mod.rs` | 补齐 LibraryIndexRow 构造点 |
| `app/rust/src/api/db.rs`、`app/rust/src/api/library.rs` | 索引和随机候选 FRB DTO/API |
| `app/rust/src/remote_scan/persistence.rs`、`app/rust/src/sync/mod.rs` | 保留远端已发布元数据，并兼容旧同步载荷 |
| `app/lib/store/library_index_service.dart` | Dart FRB 索引 DTO 映射、本地目录分类与旧索引回填 |
| `app/lib/store/library_catalog.dart` | 随机候选查询封装 |
| `app/lib/store/library_store.dart` | 统一历史读写与候选访问 |
| `app/lib/store/random_read_selector.dart` | 分批取候选、路径校验、历史排除和小池循环 |
| `app/lib/ui/home_page.dart`、`app/lib/ui/reader_page.dart` | 首页和阅读结束入口接入同一选择器 |
| `app/lib/src/rust/api/*.dart`、`app/lib/src/rust/frb_generated.dart` | 由 FRB 生成，不手改 |
| `app/rust/src/frb_generated.rs` | 由 FRB 生成，不手改 |

## Review Focus

- 旧数据库尚无 `asset_kind` 时，迁移和下一次本地扫描补齐分类，不破坏远端字段；检查 Task 1 的迁移、读写路径。
- 含归档子项的容器目录、直接图片目录、普通目录分类互斥且与 FolderBook 规则一致；检查 Task 1 的分类分支。
- 远端归档的父清单未发布、远端图片目录自身未完成、幽灵来源和墓碑行均被排除；检查 Task 2 的 SQL 条件。
- 历史条目数大于候选池、路径失效、可用候选不足一个批次时，选择器可继续取样并从最旧历史开始循环；检查 Task 3 的选择循环。
- 搜索或标签过滤不改变候选池，结束弹窗排除当前漫画，两个入口写入同一历史；检查 Task 3 的调用点。

---

### Task 1: 索引分类与兼容持久化

**Files:**
- Modify: `app/rust/src/db/mod.rs`
- Modify: `app/rust/src/catalog_context.rs`
- Modify: `app/rust/src/rchpkg/mod.rs`
- Modify: `app/rust/src/api/db.rs`
- Modify: `app/rust/src/sync/mod.rs`
- Modify: `app/rust/src/remote_scan/persistence.rs`（仅在索引 upsert 需要显式保留远端值时）
- Modify: `app/lib/store/library_index_service.dart`
- Regenerate: `app/lib/src/rust/api/db.dart` 及 FRB 公共生成文件

**Interfaces:**
- `LibraryIndexRow.asset_kind: Option<String>` 与 `LibraryIndexDto.asset_kind: Option<String>` 对应。
- FRB 生成的 Dart `LibraryIndexDto.assetKind` 可空；旧同步 JSON 缺字段时按空值读取。
- 本地目录类型只写入 `ContainerDir`、`ImageFolder` 或 `PlainDir`；本地归档文件标记 `ArchiveFile`。这些值沿用 remote scan 已有的 asset kind 命名。

- [x] 在 `db/mod.rs` 基础 `library_index` schema 与迁移中添加可空 `asset_kind`，并扩展 `LibraryIndexRow` 的 create/load/upsert/sync 路径；缺字段的旧同步载荷使用默认空值，合并时不能用空值覆盖已有远端 asset_kind。
- [x] 更新所有现存 `LibraryIndexRow` 构造点（含内部 fixture）以提供字段默认值；不新增用例，也不运行测试。
- [x] 在 `api/db.rs` 读写 DTO 和生成的 Dart `LibraryIndexDto` 中贯通 `asset_kind`，再按仓库流程生成 FRB 绑定。
- [x] 在 `library_index_service.dart` 的 `scanLocalSource` 中，索引归档文件类型；枚举目录直接子项时按 remote scan 的同一分类优先级标为 `ContainerDir`、`ImageFolder` 或 `PlainDir`，图片名沿用 FolderBook 的扩展名与隐藏文件规则。
- [x] 在本地扫描增量复用前检查子树是否含缺失 asset kind 的旧行；未分类的子树必须重新枚举并回填。将 asset kind 纳入 `rootHashOf`，确保分类变化会触发持久化。
- [x] 对通用 upsert 使用空值保留语义（例如 SQL `COALESCE`），使旧扫描/同步载荷不会清空远端扫描写入的 `asset_kind`。
- [x] 核对远端扫描发布仍由原有远端持久化逻辑写入 `asset_kind/listing_complete`，本地扫描与同步 upsert 不会清空这些列。

### Task 2: Rust 全库候选 API 与 Dart catalog 封装

**Files:**
- Modify: `app/rust/src/api/library.rs`
- Modify: `app/lib/store/library_catalog.dart`
- Regenerate: `app/lib/src/rust/api/library.dart` 及 FRB 公共生成文件

**Interfaces:**
- Rust `RandomBookCandidateDto`: `book_key`、`source_id`、`source_type`、`path`、`title`。
- Rust API: `db_random_library_candidates(excluded_book_keys: Vec<String>, unavailable_candidate_keys: Vec<String>, limit: i64) -> Result<Vec<RandomBookCandidateDto>, String>`；limit 在 API 内限制为最多 32。
- Catalog 封装将数据库 DTO 映射为带解析后 `BookSource` 的 Dart 随机候选。

- [x] 在 `api/library.rs` 构造与 Dart/Rust `bookKeyOf` 等价的 SQL 规范漫画键（斜杠、Windows 盘符、归档别名一致），并用该键去重与比较排除历史；local/SMB 归档按既有漫画扩展名识别，远端归档按已发布的 `ArchiveFile` 标记识别，图片目录按 `ImageFolder` 识别；过滤 deleted/ghost/普通目录，并校验远端归档父清单与图片目录自身均已完整发布。
- [x] 给候选 SQL 加入历史键与本次失效键排除，执行 `ORDER BY RANDOM()` 并限制返回最多 32 条；不在 SQLite 锁内做文件系统检查。
- [x] 在 `library_catalog.dart` 暴露候选批量读取方法，并映射 source/path/title/key；按 `app/codegen.ps1` 生成绑定。运行脚本前需关闭正在运行的 RCH，因为脚本会重建 release 库。

### Task 3: 统一选择器与两个入口

**Files:**
- Modify: `app/lib/store/random_read_selector.dart`
- Modify: `app/lib/store/library_store.dart`
- Modify: `app/lib/ui/home_page.dart`
- Modify: `app/lib/ui/reader_page.dart`

**Interfaces:**
- 选择器输入为异步候选批量加载回调、最近漫画键、历史写入回调，以及可选的当前漫画排除项；不再接收 `ReadRecord` 列表作为候选池。
- 选择结果包含 `BookSource`、path 和 title；稳定 key 使用现有 `bookKeyOf(source.type, source.id, path)`。
- `LibraryStore.recordRandomSelection` 保留唯一 key 并将历史上限固定为 10。

- [x] 改造 `RandomReadSelector.pick`：每次从 catalog 拉取最多 32 项，在 SQLite 锁外并发检查本地文件/目录；已配置且非 ghost 的远端来源不做本地路径检查，实际连接在打开时建立。
- [x] 对每批可用候选等概率抽取一个 key；候选经过规范 key 去重后再进入抽取，避免重复索引行改变概率。
- [x] 选择时优先排除最近 10 个 key；批次为空或仅有无效路径时将失效 key 加入本次排除并继续取样；候选只落入历史时逐项放开本候选池中最旧的历史 key，直到选中或确认空池。
- [x] 在选中后、开始打开漫画前立即调用 `recordRandomSelection`，避免失败条目被连续抽中；空池返回明确结果。
- [x] 更新首页随机按钮：移除搜索/标签过滤列表参数，随机池直接走 catalog；保留按钮 busy 状态和无候选提示。
- [x] 更新阅读结束随机：复用相同 selector，并通过 source + path 排除当前漫画。
- [x] 确认 `library_store.dart` 的已有随机历史设置兼容旧值，读取历史不改变阅读进度。

### Task 4: 静态验证与差异复核

**Files:**
- No additional source files.

- [x] 在 `app` 目录运行 `.\\codegen.ps1` 生成 Rust API 绑定并重建 release 库；先确认 RCH 已关闭。
- [x] 在 `app` 目录运行 `flutter analyze --no-pub`，预期退出码为 0。
- [x] 在 `app/rust` 目录运行 `$env:RUSTFLAGS="-D warnings"; cargo check --locked --all-targets`，预期退出码为 0。
- [x] 按 Review Focus 逐项复核 SQL 条件、分类分支和选择循环；本任务不执行测试命令。

### Follow-up: 阅读器顶栏随机入口

- [x] 在所有阅读模式与平台的顶栏显示“随机阅读一本”按钮，并保留挑选中禁用状态。
- [x] 将末页随机入口改为复用阅读器统一动作；统一处理当前漫画排除、最近历史、无候选提示和选择异常。
- [x] 运行 Flutter 静态分析并重新构建 Windows Debug 应用；不新增或运行自动化测试。
