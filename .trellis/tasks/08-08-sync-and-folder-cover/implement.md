# Implement

## M1 同步完善（先做，可独立验证）

1. Rust：`MergeStats` 增加 `incremental` / `device_id`（rchpkg/mod.rs merge 时从 manifest 带出）；补单测。
2. FRB 重新生成（flutter_rust_bridge_codegen）后更新 Dart 绑定；若工具链不便，先手工同步生成文件。
3. `sync_manager.dart`：
   - `pushNow({bool full})` / `_pushFolder` / `_pushWebdav` 透传 `incremental`；
   - `_pushWebdav`：写 `devices/{id}/latest.rchpkg` + `meta.json`，保持根 `latest.rchpkg`；
   - `_pullWebdav`：列 `devices/` → 逐设备 meta + 包合并 + 首次接触增量提示；
   - 自动同步 Timer（interval、启动延迟 60s、先拉后推、busy 跳过、dispose 取消）。
4. `sync_panel.dart`：全量推送按钮、自动同步间隔下拉、已同步设备列表。
5. 验证：`cargo test --lib`、`flutter analyze`、本机+模拟第二设备互推互拉。

## M2 文件夹封面（独立验证）

1. 快照缓存（Dart）：远程 `_list` 成功后写入 `_dirSnapshots[sourceType|sourceId|path] = entries`（原始条目，不新增请求）。
2. `FolderSnapshotStore`（folder_snapshot_store.dart）：全局内存 + 磁盘 JSON（缓存根目录），main 启动加载、`_LifecycleFlush` 兜底落盘。
3. `source_browser.dart`：`_detectComicFolders` 改为纯本地判定（快照 → 自然序第一个漫画扩展名文件；缺失时查 `LibraryStore.records` 该目录下最小路径）；记录 `_folderFirstFile`；无本地数据 → “未缓存”占位卡片（点击下钻），不发起任何 `*List`；监听 `LibraryStore` 重跑远程判定（下载/记录变化后 未缓存 → 封面）。
4. `_ComicFolderCoverCard` 远程模式：封面 = `ComicCover(source, path: firstComicFile, force: true)`（未下载自动“未缓存”）；firstComicFile 为空时 cover 区直接渲染同一“未缓存”占位。
5. `ComicCover`：cacheKey（封面页/裁切/画质）变化时 didUpdateWidget 重载 → 自定义封面即时生效。
6. 本地路径：容器文件夹（含 .cbz/.zip 等）检测为本地目录读取并显示首个漫画包封面，点击下钻；回归 cover.jpg 优先、无封面用首页、无漫画文件无封面。
7. 验证：`flutter analyze`、`flutter test`；手动验证本地 + WebDAV + 百度/夸克/115/SFTP 目录封面与“未缓存”；断网打开网盘目录确认封面检测 0 新增请求（仅用户浏览列表请求）；重启后下载过的文件夹封面仍显示。

## 验证命令

```bash
cd app/rust && cargo test --lib
cd app && flutter analyze
```

## 提交与归档

- 每里程碑一个提交（`feat(sync): ...` / `feat(ui): ...`），完成后按 Trellis 流程归档任务。
