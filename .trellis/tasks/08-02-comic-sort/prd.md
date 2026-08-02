# 漫画排序：按字母 / 按加入时间
## Goal

书源浏览（SourceBrowser）的漫画列表支持两种排序：**按字母**（名称自然排序，数字感知）与**按加入时间**（本地文件修改时间作为"加入时间"代理，最新在前）。

## Background（已确认）
- 当前目录列表来自 Rust `list_dir`：目录在前 + 自然名称排序（[local.rs](C:/Users/cfl/Desktop/RCH/app/rust/src/source/local.rs:54)）。
- `DirEntry` 仅有 name/path/is_dir/size，无时间字段；WebDAV 列表未解析修改时间。
- FRB 2.12.0 + `flutter_rust_bridge_codegen` 可用，`DirEntry` 增加 mtime 字段后需重新生成绑定（[flutter_rust_bridge.yaml](C:/Users/cfl/Desktop/RCH/app/flutter_rust_bridge.yaml)）。

## Requirements

- **R1** Rust `DirEntry` 增加 `mtime`：本地取文件修改时间（unix 秒）；WebDAV 无数据时为 0。
- **R2** SourceBrowser 工具栏提供排序选择：按字母（默认）/ 按加入时间；会话内状态即可，不持久化。
- **R3** 按加入时间 = mtime 降序（最新在前）；mtime=0（WebDAV/无时间数据）的条目排最后，同值按名称自然序兜底。两种排序都保持"目录在前"的分组。
- **R4** 排序作用于当前目录的漫画/文件夹列表，海报墙与列表视图共用同一排序。

## Acceptance Criteria

- [x] 海报墙/列表视图可切换两种排序（工具栏排序菜单，两种视图共用 `_filtered`）。
- [x] 本地目录切到"按加入时间"后，最新修改的漫画排在最前（mtime 降序）。
- [x] WebDAV 目录切换排序不崩溃（mtime=0 视为最旧，按名称自然序兜底）。
- [x] `cargo test --lib` 通过（34 passed）；`flutter analyze` 0 issues；FRB 重新生成仅新增 mtime 字段。

## Out of Scope

- 排序选择持久化（本次为会话内状态）。
- 最近阅读 / 最多阅读 / 标签结果等视图的排序。
- 应用侧独立维护"首次入库时间"（本次以文件修改时间代理"加入时间"）。

## Verification（2026-08-02）

- Rust：`DirEntry`/`Entry` 增加 `mtime`（本地 `metadata.modified()`，WebDAV 为 0）；`cargo test --lib` 34 passed。
- FRB：`flutter_rust_bridge_codegen generate` 重新生成，diff 仅 mtime 相关（frb_generated.rs/dart + api/book.dart），无无关改动。
- Flutter：`source_browser.dart` 增加排序状态与工具栏菜单；`_filtered` 末尾统一排序（目录在前；按字母=自然序，按加入时间=mtime 降序）。`flutter analyze` 0 issues，`flutter test` 8/8 通过。
