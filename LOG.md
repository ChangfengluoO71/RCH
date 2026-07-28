

## 2026-07-28|第 32 轮:SQLite 数据层迁移（Phase 1 + 2）

**本轮目的**
将应用状态持久化从 library.json 全量读写迁移到 SQLite（rusqlite）为主存储 + library.json 备份，按 SPEC ADR-013 实施数据层升级。

**背景**
- library.json 全量重写性能差，数据量大时卡顿
- 无增量更新能力，改一个字就要序列化全库
- 无并发安全保障
- SPEC 明确要求 SQLite 管理状态，M9 里程碑已规划

**修改内容**

### 1. Rust 侧 — 完整 SQLite schema + CRUD 层

`app/rust/src/db/mod.rs`（重写，约 900 行）：
- 新增 5 张应用数据表：`book_sources`, `read_records`, `book_metas`, `tags`, `book_tags`
- 新增 `app_settings`（key-value）、`schema_version`（迁移版本标记）
- 保留原有 `cache_index` + `source_capability` 表不变
- 标签 ID 生成策略：tag_id = tag_name.trim().to_lowercase()（标签名即主键，零碰撞）
- 23 个 CRUD 方法：完整的书源/记录/元数据/标签/设置增删改查
- `migrate_from_library_json()` 从 library.json 全量导入 SQLite（幂等）
- `is_migrated()` 检查迁移是否完成

`app/rust/src/api/db.rs`（新文件，约 300 行）：
- FRB 桥接层，6 个 DTO + 23 个 pub fn
- 与 `api/book.rs`（阅读会话）、`api/cache.rs`（缓存管理）并列

`app/rust/src/api/mod.rs`：
- 注册 `pub mod db;`

### 2. Dart 侧 — SQLite-first 加载 + 双写

`app/lib/main.dart`：
- 启动时检查 `dataIsMigrated()` → 未迁移则调 `dataMigrateFromJson()` 全量导入
- 迁移失败不阻塞启动，回退到 JSON

`app/lib/store/library_store.dart`（重写 load + save）：
- `load()` 优先从 SQLite 加载（`_loadFromSqlite()`），失败则 fallback JSON
- `_save()` 双写：先写 SQLite → 再写 library.json 备份
- `recordRead()` 高频读写优化：直接 `dbUpsertRecord()`，不做全量同步
- `_saveRecordToSqlite()` + `_saveJsonBackup()` 单条增量写入
- 新增 `filePath()` 公开方法供迁移使用
- 新增 `_tryParseJson()` 辅助解析 settings key-value

`app/lib/repository/tag_repository.dart`：
- 新增 `loadFromSqlite()` — 从 SQLite 加载 tags + book_tags
- 新增 `saveToSqlite()` — 增量同步标签到 SQLite
- `_tagId()` 改为 `name.trim().toLowerCase()`，与 Rust 侧 `tag_id()` 一致
- `_normalizeTagIds()` 向后兼容旧 hash ID → 新 name ID 合并

`app/lib/src/rust/api/db.dart`：
- FRB codegen 自动生成，23 个 Dart API + 6 个 DTO 类

### 3. Rust FRB codegen

- 运行 `flutter_rust_bridge_codegen generate`
- `app/rust/src/frb_generated.rs` 和 `app/lib/src/rust/frb_generated*.dart` 自动更新

**决策原因**

1. **标签 ID 用名称小写作为主键**：
   - DJB2 哈希跨语言实现不一致导致"标签管理点击标签不显示漫画"的 bug
   - 改用标签名即 ID，彻底消除跨语言 Hash 一致性问题
   - "已读""朝凪"这样中英文混合的标签直接用原名小写做主键，可读性更好
   - 个人本地书库标签量不会大到需要 Hash 节省空间

2. **双写（SQLite + JSON）而非纯 SQLite**：
   - JSON 作为备份，万一 SQLite 损坏仍可恢复
   - JSON 保持向后兼容旧版本

3. **`recordRead()` 直接写 SQLite**：
   - 阅读进度更新频率最高，全量同步太重
   - 单条 upsert 到 SQLite + 异步写 JSON 备份

**影响范围**

| 层 | 文件 | 变更类型 |
|---|---|---|
| Rust | `db/mod.rs` | 重写（+800行） |
| Rust | `api/db.rs` | 新文件 |
| Rust | `api/mod.rs` | +1行 |
| Rust | `frb_generated.rs` | codegen 更新 |
| Dart | `main.dart` | +10行 |
| Dart | `store/library_store.dart` | 重写 load/save（+200行） |
| Dart | `repository/tag_repository.dart` | +100行 |
| Dart | `src/rust/api/db.dart` | codegen 生成 |
| Dart | `src/rust/frb_generated*.dart` | codegen 更新 |

**是否完成**
✅ Phase 1（Rust 数据层）和 Phase 2（Dart 加载/双写）已完成。
- `cargo check` ✅ 0 errors
- `flutter_rust_bridge_codegen generate` ✅
- `flutter analyze` ✅ 0 errors
- `flutter run -d windows` ✅ 启动成功，SQLite 迁移生效
- database.db 7 张表全部有数据

**遗留问题**
- Phase 3（纯 SQLite 单写 + 删 JSON 非 settings 部分）待用户确认后实施
- Phase 4（FTS5 搜索、性能优化）后续
- 构建过程中多次遇到 CMake INSTALL 步骤失败，根因是 `dart.exe` / `conhost.exe` 残留进程占用文件，需 `taskkill` 后重新构建
- `cmake_install.cmake` 中 `native_assets/windows` 目录为空时会失败，需先确保 Flutter 构建完整

## 2026-07-28|第 33 轮:标签 ID 重构 + 标签管理 Bug 修复 + 已读标签

**本轮目的**
1. 修复标签管理界面点击标签只显示已读漫画的 Bug
2. 标签 ID 从 DJB2 哈希改为标签名（与 Rust 统一，消除跨语言 Hash 不一致）
3. 添加"已读"元数据标签功能

**问题分析（先分析后动手）**

Bug 根因：
1. `recordsByTag()` 第 567-574 行的元数据标签分支，当漫画有 author/genre/series 标签但**没有阅读记录**时，`records[m.key]` 返回 null，被直接跳过，未合成 ReadRecord，导致该漫画不出现在标签详情页。
2. `_tagId()` 从 `hashCode→base36` 改为 DJB2 后，SQLite 和 library.json 中迁入的是旧 ID，而查找时用新算法重算 ID → 匹配不到 → 查不到关联漫画。

**修改内容**

1. `app/lib/store/library_store.dart` — `recordsByTag()`：
   - 元数据标签分支补上未读漫画的合成 ReadRecord（之前只有 `if existing != null` 才加，缺少 else 分支）

2. `app/lib/repository/tag_repository.dart` — `_tagId()`：
   - 从 DJB2 hash 改为 `name.trim().toLowerCase()`，与 Rust `tag_id()` 完全一致
   - 新增 `_normalizeTagIds()` 向后兼容归一化

3. `app/lib/store/library_store.dart` — `recordRead()`：
   - 每本打开过的漫画自动打"已读"元数据标签

4. `app/rust/src/db/mod.rs` — `tag_id()`：
   - 从 DJB2 hash 改为 `name.trim().to_lowercase()`

**决策原因**

- **标签名即 ID**：跨语言 Hash 一致性问题是根本问题。Dart `hashCode` → `base36`、Rust `DefaultHasher` → DJB2 → hex，版本间不同算法产生不同 ID，累计迁移成本太高。直接用标签名小写做主键，零碰撞、可读、彻底消除跨语言不一致。
- **"已读"标签不设为独立字段**：复用现有 Tag 体系，自动打标签而非新增 schema 字段，保持模型简单。用户将来可以手动移除"已读"标签。

**影响范围**
- Dart: `library_store.dart`、`tag_repository.dart`
- Rust: `db/mod.rs` `tag_id()`

**是否完成**
✅ flutter analyze 通过，待用户启动验证。

**遗留问题**
- 已读/未读按钮目前通过 TagRepository + saveToDisk 持久化，后续可优化为单独 API 减少 SQLite 写入延迟
- 已读标签显示为红色元数据图标（与 author/genre/series 同级）
- 数据库已有旧 hash ID 标签需经一次启动归一化后生效

---

## 2026-07-28|第 34 轮：已读元数据标签完善 + 应用安装程序 + 使用文档

**本轮目的**
1. 完善已读元数据标签：详情页按钮手动切换、批量操作支持
2. 构建 Release 安装程序并推送 GitHub
3. 编写用户使用文档，解释标签体系和已读功能

**修改内容**

### 已读标签完善
1. `app/lib/ui/book_detail_page.dart`：详情页加已读/未读切换按钮，红色图标
2. `app/lib/store/library_store.dart` — `batchTag()`："已读" 作为元数据标签直接走 `TagRepository.link()`，不走 `BookMeta.tags`
3. `app/lib/store/library_store.dart` — `metaFields`：增加 '已读'
4. 公开 `saveToDisk()` 供外部直接操作 TagRepository 后持久化

### 应用安装程序
- `flutter build windows` → `build/windows/x64/runner/Release/RCH.exe` (92KB)
- 发布目录含 `flutter_windows.dll` (20MB) + `rust_lib_app.dll` (8.4MB) + `data/`

### README 更新
- 漫画详情增加已读标记说明
- 标签管理增加已读标签说明
- 批量标签管理增加批量标注已读说明

### 使用文档（docs/user-guide.md）
- 完整用户操作指南：书源、阅读、标签体系、跨书源搜索
- 已读标签：首次打开漫画自动标记，也可在详情页手动切换，支持批量操作
- 标签体系说明：元数据标签（红色）vs 普通标签（黄色）

**决策原因**
- "已读" 设为元数据标签而非 schema 字段：复用 Tag 体系保持模型简单
- 批量标注已读：`batchTag` 开头加守卫，标签名是"已读"时直接走 TagRepository

**影响范围**
- Dart: `book_detail_page.dart`、`library_store.dart`
- 文档: `README.md`
- 构建产物: `build/windows/x64/runner/Release/`

**是否完成**
✅ 已完成。代码已提交推送到 GitHub (`f1148c1`)。
- `flutter analyze` 0 error、0 warning
- `flutter build windows` 成功
- 使用文档编写完毕

**遗留问题**
- 使用文档暂不上传，后续用户确认后补充到仓库
