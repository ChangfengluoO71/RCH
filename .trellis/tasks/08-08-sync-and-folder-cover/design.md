# Design

## A. 同步完善

### A1 全量推送
- 现有 FRB 接口 `rchpkg_export(path, incremental)` 已支持 `incremental=false`（api/package.rs:34），全量无需改 Rust，只需 Dart 侧传参。
- `SyncManager.pushNow({bool full = false})`，`_pushFolder` / `_pushWebdav` 透传 `incremental: !full`。
- 同步面板在「推送」旁加「全量推送」按钮（WebDAV 与目录模式都提供）。

### A2 云端按设备保留包
- 推送（`_pushWebdav`）：
  1. 逐级 MKCOL：根目录、`devices/`、`devices/{deviceId}/`；
  2. `rchpkgExport` 后 PUT `devices/{deviceId}/latest.rchpkg`；
  3. PUT `devices/{deviceId}/meta.json`：`{"deviceId","deviceName","lastPushAt"}`（deviceId/deviceName 从 `get_or_create_device_id` / `default_device_name` 获取——需要新增轻量 FRB 或复用 manifest 信息；优先复用 `SyncExportInfo.deviceId` + Dart 侧补 name）；
  4. 兼容：仍 PUT 根 `latest.rchpkg`。
- 拉取（`_pullWebdav`）：
  1. `webdavList(devices/)` 列设备目录；
  2. 对每个设备 GET `meta.json`（解析失败跳过）→ `dbRegisterDevice` 刷新 devices 表；
  3. 对每个设备 GET `latest.rchpkg` → `rchpkgImport(force:false)` 合并；
  4. 根 `latest.rchpkg` 若存在且不属于任一设备包，也合并（兜底兼容旧版本）。
- 归档清理：`_cleanWebdavArchives` 只清理 `archive/` 下 `.rchpkg`，`devices/` 不受影响。

### A3 增量缺失提示
- Dart 侧判断“首次接触”：拉取每个设备包前查询 `dbListDevices()` 是否已含该 deviceId；若首次且随后合并的包 `manifest.incremental == true`，收集提示文案。
- 需要把包的 incremental/deviceId 信息暴露给 Dart：`SyncImportStats` 增加 `incremental` / `deviceId` 字段（Rust merge 结果带出 manifest 字段），或在 Dart 侧下载后先解包 manifest。倾向改 `MergeStats` 增加 `incremental`、`device_id`（rchpkg/mod.rs），FRB 重新生成后 Dart 使用。

### A4 自动同步
- `sync_interval_minutes`（已有 key，0=关闭）：同步面板下拉选择。
- `SyncManager` 内 `Timer`：启动时若 interval>0 且配置完整，60s 后执行一次，之后每 interval 执行；流程 = `pullNow()` → `pushNow()`（先拉后推，避免覆盖他人）；`busy` 时跳过本次。
- 生命周期：`dispose` 取消 Timer；Android 后台可能被杀（文档注明，应用前台/运行时有效）。

### A5 设备列表
- 同步面板新增「已同步设备」区：`dbListDevices()`（id → name → last_seen_at）+ 云端 `devices/` 目录发现的设备（meta.json），显示名称、短 ID（前 8 位）、最近同步时间。
- 设备唯一 ID 已存在（`dev_{timestamp}_{pid}`，db/mod.rs:1348），无需新机制。

## B. 文件夹封面

### 现状
- 本地已实现：`is_comic_folder` / `folder_cover_path`（api/book.rs:172/178）+ `source_browser._comicFolderCard` / `_ComicFolderCoverCard`；
- `ComicCover` 对远程书源未下载时显示“未缓存”占位（comic_cover.dart `_placeholder`），下载后显示封面——B3 基本已满足，需回归确认；
- `ComicCover._load()`（comic_cover.dart）对远程书源先查 `*HasRawCache`（本地 raw 文件存在性），无缓存直接抛错 → “未缓存”占位；有缓存才调 `*Cover` 从本地 raw 缓存生成封面。Dart 侧已保证封面生成不触网（Rust 侧 `*Cover` 内的流式/下载分支不会被走到）。
- 缺：`_detectComicFolders` 仅 `isLocalFs` 生效，网盘子目录不检测、无封面；master 上远程目录列表无持久缓存（上次备份分支的 60s 列表缓存未合入）。

### 设计（全本地，0 网盘请求）
1. 目录快照缓存（Dart 侧，进程内 Map + 可选持久化）：远程任意 `*List` 成功返回时，把该路径原始条目（name / is_dir）写入快照，key = `sourceType|sourceId|path`。**复用用户浏览时的同一次请求，不新增任何请求**。
2. 封面判定（纯本地读，`_detectComicFolders` 改造）：
   - 优先读快照：子目录有快照 → 按自然序找第一个漫画扩展名文件（与 `_list` 过滤同一扩展名集合）；有 → 漫画文件夹，记录 `_comicDirFirstFile[path]`；无漫画文件 → 普通文件夹；
   - 快照缺失 → 查 `LibraryStore.records`：该目录下按自然序最小路径的记录（用户已打开/下载过其中书籍）；
   - 两者皆无 → 视为“未缓存”（与漫画文件一致），显示占位；不发起任何请求。
3. 渲染：`_ComicFolderCoverCard` 远程模式 → `ComicCover(source, path: firstComicFile, force: true)`：
   - raw 缓存存在 → 本地生成封面（现有 hasRawCache 前置检查保证）；
   - 不存在 → “未缓存”占位（现有 `_placeholder`）。
   - firstComicFile 为空（本地无数据）→ cover 区直接渲染同一“未缓存”占位，点击下钻打开目录。
4. 下载后自动刷新：`_ComicFolderCoverCard` 监听 `LibraryStore`（ChangeNotifier）；检测到首文件 raw 缓存落盘后重建 cover future（清失败 future / 重入 `_CoverLoadQueue`），实现 未缓存 → 封面，无需手动刷新。
5. 本地文件夹：`is_comic_folder`（图片文件夹）+ `folder_cover_path`（cover.jpg）+ 首页兜底维持现状；本地容器文件夹（含漫画包）的检测扩展为本地目录读取（isLocalFs 纯本地，无网络），封面 = 首个漫画包封面。
6. 容器文件夹（内含 .cbz/.zip 等漫画包）默认也显示首文件封面，点击仍下钻（与图片文件夹点击进详情的语义区分）。
7. 性能：仅海报模式检测；判定为内存查询（无 IO 竞争）；复用 `_CoverLoadQueue`（并发 4）；页面 dispose 取消。

### 边界
- 本地无数据的远程子目录：显示“未缓存”占位（与漫画文件一致），点击下钻；用户打开该目录后快照落库（同一次浏览请求），返回后变为封面或普通文件夹卡片；下载其中书籍后经 records 显示封面；
- 零请求保证：封面判定路径不含任何 `*List` / `downurl` / `probe` / 下载调用；用断网打开网盘目录验证（目录列表本身是用户浏览请求，允许）；
- 快照持久化：`FolderSnapshotStore`（folder_snapshot_store.dart）全局内存 + 缓存根目录 `folder_snapshots.json`（防抖落盘、退出时 flush），跨页面/跨重启保留；判定顺序 = 快照 → 阅读记录（层级路径来源兜底）→ 未缓存。扁平路径来源（115/夸克 pickcode/fid）重启后必须依赖快照，阅读记录无法反推文件夹归属；
- SMB 沿用 `isLocalFs` 路径（本地文件系统语义一致）；
- 远程“文件夹式漫画”（目录内直接放散图）不在本任务范围：远程列表过滤已剔除散图，且远程散图目录无法作为单本书打开；
- 不新增 Rust 接口：快照与判定均在 Dart 组合现有 list 结果 + records 完成。
