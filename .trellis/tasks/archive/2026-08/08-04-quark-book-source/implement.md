# 夸克网盘书源 — 实施计划

## 前置条件（用户侧）

- [ ] 提供夸克网盘登录后的 Cookie（浏览器 F12 从 `pan.quark.cn` 任意请求复制），网盘中备 1 个 CBZ 测试文件用于联调。

## 实施步骤（按序执行）

### 步骤 0：API 冒烟（独立最小验证，关隘 A）

- 用真实 Cookie 验证：`GET /config` → `GET /file/sort`（根目录 `0`）→ `POST /file/download` → 直链 `Range: bytes=0-0` 是否 206。
- 产出 `research/quark-api-contract.md`：实际 JSON 样例、错误码 / message 映射、`__puus` 续期行为、Range 结论、download 响应是否含 `file_name` / `size`。
- 关隘：能列目录 + 取直链才进入实现；否则先修正契约理解。

### 步骤 1：Rust 客户端模块

- 新增 `app/rust/src/source/quark.rs`（`QuarkClient` + `QuarkFile` + 单测），`source/mod.rs` 注册模块。
- 验证：`cd app/rust; cargo test`。

状态：✅ 已完成（`cargo test --lib` 55 项全绿，无警告）。

### 步骤 2：DB 迁移 + 模型

- `db/mod.rs`：幂等 `ALTER TABLE book_sources ADD COLUMN cookie TEXT`；`BookSourceRow` + `load_all_sources` / `upsert_source` 补字段。
- `flutter_rust_bridge_codegen generate` 再生成；`api/db.rs` 与 `frb_generated.*` 同步。
- 验证：`cargo test` + `flutter analyze`。

状态：✅ 已完成（`book_sources` 幂等加 `cookie` 列；FRB 2.12.0 已重新生成）。

### 步骤 3：Rust API 层

- `api/source.rs`：`QUARK_SESSIONS` / `QUARK_DOWNLOADS` + `quark_connect / disconnect / list / open_quark_book / download_progress / has_raw_cache / cover`；三态打开镜像 115，`open_document` 传真实文件名。
- FRB regen + Dart wrapper（默认 `strategy='auto'`）。
- 验证：`cargo test` + `flutter analyze`。

状态：✅ 已完成（`quark_connect / list / open_quark_book / download_progress / has_raw_cache / cover` 全链路可编译）。

### 步骤 4：Dart 模型与会话

- `models.dart`：`cookie` 字段 + `isQuark` / `needsSession` / `capabilityDisplay` + JSON。
- `book_repository.dart`：`updateSource` / `loadFromSqlite` / `saveToSqlite` 映射。
- `store/quark_session.dart`：`quarkSessionFor` / `clearQuarkSession`。
- 验证：`flutter analyze` + `flutter test`。

状态：✅ 已完成（含 cookie 回写 DB 与会话失效）。

### 步骤 5：UI 与分发

- `home_page.dart`：DropdownMenu 第 7 项 + 表单 + `_submitQuark` + 编辑分支 + `_sourceTypeLabel`。
- 分发点：`source_browser.dart` / `book_detail_page.dart` / `ai_upscale_manager.dart` / `comic_cover.dart`。
- `test/add_source_dialog_test.dart`：6 → 7 类型，quark 表单断言。
- 验证：`flutter analyze` + `flutter test`。

状态：✅ 已完成（分发点：source_browser / book_detail_page / ai_upscale_manager / comic_cover / reader_page / opener 全部接入；analyze 0 issues，flutter test 11 项全绿）。

### 步骤 6：全量验证

- `cd app/rust; cargo test; cargo build --release`；`cd app; flutter analyze; flutter test`。
- 清理逻辑回归：`removeSourceWithCleanup` 对 `quark|{id}|` 前缀正确；`purgeStale` 不误删其他类型。

状态：✅ 已完成（`cargo build --release` 通过；`removeSourceWithCleanup` 按 `type|id|` 前缀天然覆盖 quark）。

### 步骤 7：用户联调（关隘 D，需真实 Cookie）

- 粘贴 Cookie → 连通性测试 → 浏览 → 三策略打开 CBZ → 封面 → 重启凭据保持 → 删除书源清理。
- 手动 `cd app; flutter build windows --release` 冒烟。

联调发现（2026-08-04）：打开 PDF 报「无法加载 pdfium 动态库」。

- **根因**：非夸克问题。PDF 解析（pdfium-render 动态加载）依赖 `pdfium.dll`，但项目构建产物 / 安装包 / CI 均未捆绑该 dll。
- **修复**：`document/pdf.rs` 增加 exe 同目录查找并把缺 dll 从 panic 改为中文报错；已下载 `pdfium.dll`（bblanchon/pdfium-binaries v7988）放入 `build/windows/x64/runner/Debug/`；`.github/workflows/release.yml` 构建后自动下载并捆绑进安装包；SETUP.md 补充本地构建说明。单测 `pdfium_dll_loads_when_present` 覆盖 dll 可加载。
- **用户操作**：重启应用（`flutter run` 重新构建后生效）。历史安装包（v0.3.x）不含 pdfium.dll，需等下一次发布或手动放置。

联调发现（2026-08-04）：打开 EPUB 报「EPUB 中找不到图片: resources/P00001.jpg」。

- **根因**：非夸克问题。`document/epub.rs` 把 HTML 内 `<img src>` 按 **OPF 目录**解析，而正确语义是相对 **HTML 文件所在目录**。实测 EPUB（ChainLP 生成，`content/index_P00001.xhtml` + `content/resources/P00001.jpg`）因此算出 `resources/P00001.jpg`，实际为 `content/resources/P00001.jpg`，索引失败即报错。
- **修复**：`epub.rs` 改为按 HTML 目录解析 img src，并对收集到的图片路径去重；新增回归单测 `open_epub_with_html_subdir_images`（复刻实测结构）。`cargo test` 57 项全绿；`cargo build --release` 通过。
- **用户操作**：重启应用（`flutter run` 重新构建后生效；原 EPUB 已在 raw/ 缓存，重开即复用）。

### 步骤 8：收尾

- `trellis-check` 全量检查（spec 合规 / lint / 测试 / 跨层一致性）。
- `trellis-update-spec`：夸克 API 契约要点 + 认证流程写入 `.trellis/spec/backend/`。
- 按 Phase 3.4 批量提交（先工作提交，再归档 / 日志提交）。

## 验证命令速查

```powershell
cd app/rust; cargo test; cargo build --release
cd app; flutter analyze; flutter test
```

## 回滚点

- 步骤 0 失败：不进实现，回 research 修正契约。
- 步骤 1~3 失败：只动 Rust 层，删模块 / API 即可，Dart 未动。
- 步骤 4~5 失败：删 Dart 分支，DB 列保留无害。
- 任意步骤发现 prd 缺陷：回 Phase 1 修订 prd/design 再继续。

## 评审关隘

- 步骤 0 冒烟结果（真实 API 响应 / Range 结论）→ 关隘 A
- 步骤 3 完成：Rust API 全链路可编译 + 单测 → 关隘 B
- 步骤 5 完成：UI + 测试全绿 → 关隘 C
- 步骤 7 用户联调通过 → 关隘 D（之后才提交）
