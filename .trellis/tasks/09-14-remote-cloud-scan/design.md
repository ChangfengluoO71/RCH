# 全云端远程扫描与封面索引：技术设计

> 本文是 Trellis 任务 `09-14-remote-cloud-scan` 的技术设计摘要。完整、可审阅的设计规范位于 [`docs/superpowers/specs/2026-09-14-remote-cloud-scan-design.md`](../../../docs/superpowers/specs/2026-09-14-remote-cloud-scan-design.md)。

## 1. 目标与边界

本任务把 WebDAV、SFTP、百度网盘、115 和夸克统一纳入“首次授权/首次打开后全量、后续增量、用户可手动重扫”的远程媒体扫描。SMB/NAS 保持本地文件系统生命周期，只复用图片文件夹分类；M8 继续 Catalog-only/local-only。

Rust 是远程 I/O、扫描状态、检查点、限速、封面依赖和缓存清理的一致性边界；Dart 是触发、控制和 UI 边界。应用级后台 worker 不新增 Android 常驻服务，进程被挂起或终止时从检查点恢复。

## 2. 组件与数据流

```text
Dart RemoteScanCoordinator
  -> Rust RemoteScanEngine
     -> RemoteProviderAdapter (WebDAV/SFTP/Baidu/115/Quark)
     -> manifest/index + cover dependency + progress
  -> ComicCover (cache-only read) / SourceBrowser status UI

Reader foreground -> shared request governor -> provider byte source
Reader completion -> page/raw cleanup (cover retained)
Verified tombstone -> stale asset cleanup (cover + aliases + dependencies)
```

可见卡片只读已生成的 cover cache；未完成扫描时的按需请求以相同任务键合并，不直接另起请求。

## 3. Provider adapter

统一接口提供分页目录、`size/mtime` 元数据、规范化逻辑路径、受限整文件读取、随机/Range 读取和能力探测。分页全部成功后才返回 `complete=true`。错误统一分类为认证过期、权限拒绝、未找到、限流、临时错误、Range 不可用、响应格式错误和取消。

Range 能力按资源指纹缓存，必须验证 `206 + Content-Range`；`Accept-Ranges` 不是成功条件。压缩包沿用现有 Range/下载策略；图片文件夹在 Range 不可用时只允许受单图片大小上限限制的读取。

## 4. 清单、资产与扫描状态

权威清单在 Rust 数据库，`folder_snapshots.json` 仅是 UI 快照。扩展 `library_index` 保存 `asset_kind`、内容指纹、generation、目录分页完整标志；新增每来源 `remote_scan_state`、目录 listing 状态和 `remote_cover_dependency`。`FolderSnapshotEntry` 升级为 v2，兼容旧 v1 但不以旧快照判定删除。

资产分为 `archive_file`、`image_file`、`image_folder`、`container_dir` 和普通目录/文件。图片文件夹按自然排序识别首图；容器封面依赖第一个稳定排序的子漫画。

增量扫描先刷新根清单，再只展开新增、删除、未完成或指纹变化目录。缺少 mtime 时使用子清单哈希和短 TTL。每个完整目录在事务中提交清单和 tombstone；失败目录保留上一份完整清单。generation 只有在全量任务队列完成后才标记 complete。

## 5. 调度与阅读

扫描使用与 Reader 共享的 Rust governor，优先级为当前页 > 阅读器预取 > 扫描。全局/单来源并发、provider 限速、列表批次和字节缓冲均有硬上限。任务键为来源、逻辑路径、内容指纹和封面参数；暂停、取消、退避和检查点均可观测且有界。

`RemoteFolderBook` 实现现有 `Document`。每页按图片子项独立读取，使用相同 `BookKey`、page cache、预取和完成清理；不把整个远程文件夹转换为一个 raw 文件。压缩包的 `stream/download/auto` 语义不变。

## 6. 缓存与删除

阅读完成只删除 page/raw，保留 cover、元数据、书源、凭据、标签、历史、完成状态和自定义封面参数。已验证完整目录产生 tombstone 后，才按依赖关系清理漫画 cover、cover alias、图片子项缓存和 page/raw；认证/网络/分页失败、取消和暂时 404 不得触发删除。

## 7. UI、设置与回滚

新增 `remoteBackgroundScanEnabled`，默认开启；现有 `remoteCoverFetchEnabled` 仍控制远程封面网络请求。来源页显示排队、处理中、完成、Range 不可用、失败和暂停，并提供继续、重试、增量重扫、全量重扫。保留 kill switch，可回退到旧的可见卡片按需封面行为而不删除清单或封面。

## 8. 验证与分阶段

先完成 schema/adapter contract 和 fake provider，再实现 worker、图片文件夹 Reader、清理、UI，最后按五类 provider 做真实 smoke。自动化覆盖错误响应、取消恢复、内存上限、封面保留和删除一致性；缺失真实设备或账号证据只能标记 pending，不能宣称 release ready。

## 9. 兼容与风险

- additive schema migration；旧索引、设置、快照和缓存可读取。
- provider 能力差异通过结构化 capability 表示，不复制 provider 下载逻辑。
- 扫描不会启动整本下载；Range 不可用沿用占位符/提示，显式自定义封面下载除外。
- 不修改 M8、凭据存储边界、应用版本或已有 dirty 工作区内容。
