# RCH 全云端远程扫描与封面索引设计

**状态：** 设计已由用户分章节确认，尚未进入实现阶段  
**日期：** 2026-09-14  
**关联 Trellis 任务：** `09-14-remote-cloud-scan`  
**适用版本：** 当前工作区基线（不修改版本号）

## 1. 背景与问题

当前远程来源的目录浏览主要依赖已有快照，封面主要由可见卡片按需请求。根目录打开不会递归发现所有漫画，直接包含图片的远程文件夹不能作为可阅读漫画，阅读完成后的内容清理与远程删除后的封面失效也没有一条统一的跨 provider 链路。现有 `08-02-cover-loading-perf` R8 明确禁止自动全目录爬取和后台批量预取；本设计在不改变其他历史任务语义的前提下，以新的统一云端扫描契约取代该行为。

本任务不是 M8 智能刮削。M8 仍保持 Catalog-only/local-only，线上智能刮削作为未来插件，不成为本功能的运行依赖。

## 2. 目标、范围与非目标

### 2.1 目标

1. 所有云端会话来源首次授权或首次打开根目录后启动一次可恢复的后台全量扫描。
2. 后续打开或授权执行增量扫描，并提供用户可控的增量重扫和全量重扫。
3. 发现压缩包漫画和直接包含图片的远程文件夹漫画，为每个可识别资产生成可复用的封面状态。
4. 让远程图片文件夹通过现有 Reader 按页读取，不影响前台流式阅读、预取和阅读完成清理。
5. 远程删除只有在完整、成功的目录校验后才会清理对应的封面、别名、依赖和内容缓存。
6. 所有 provider 的能力差异、错误和降级可观测且不泄露凭据。

### 2.2 范围

纳入 WebDAV、SFTP、百度网盘、115（扫码 Cookie 及现有兼容模式）和夸克。SMB/NAS 继续按本地文件系统处理，只复用图片文件夹识别。现有凭据存储、压缩包 `stream/download/auto` 语义、Reader 的逻辑书籍键和 M8 边界保持不变。

### 2.3 非目标

- 实现 M8 线上元数据 provider 或插件系统。
- 远程整库/整文件夹预下载、常驻 Android Service/WorkManager 或新权限。
- 改造 SMB 的授权/云端扫描生命周期。
- 修改应用版本号、升级 AGP/Gradle/Kotlin/NDK、提交 tag/release，或覆盖用户已有 dirty 工作区。

## 3. 设计原则与不变量

### 3.1 数据和安全不变量

- Rust 数据库是远程清单、扫描状态和封面依赖的权威来源；Dart JSON 快照只是 UI 缓存。
- 逻辑书籍键由 `source_type + source_id + provider-normalized logical path` 构成，不能使用临时 raw 路径、下载直链或凭据。
- 目录响应只有在所有分页成功、响应格式通过验证后才是完整清单；空列表、截断列表、403、认证过期、断线和取消都不是删除证明。
- 阅读完成内容清理保留 cover、元数据、书源、凭据、标签、历史、完成状态和自定义封面参数；经过验证的远程删除是唯一可清理失效封面的额外路径。
- 日志、统计、截图、测试 fixture 和报告不得出现 Authorization、Cookie、token、密码或带凭据的下载 URL；路径使用哈希或脱敏值。

### 3.2 资源不变量

- 请求优先级固定为前台当前页 > Reader 预取 > 扫描目录/封面。
- 所有列表批次、响应体、单图片读取、队列、缓存写入和内存缓冲有上限；扫描不能导致随整本文件大小线性增长的常驻内存。
- Range 成功必须同时满足 HTTP 206 和与请求一致的 `Content-Range`；`Accept-Ranges`、200、416 或缺失/错误 Content-Range 均不能当作局部读取成功。
- Range 不可用时，后台封面任务使用占位符/提示，不自动调用整本下载。自定义封面整本下载只能沿用已有显式用户确认路径。

## 4. 组件边界

```text
Dart RemoteScanCoordinator
  ├─ 会话/根目录触发
  ├─ 增量、全量、暂停、继续、重试
  └─ 进度和来源页 UI

Rust RemoteScanEngine
  ├─ RemoteProviderAdapter
  ├─ 清单、指纹、generation、检查点
  ├─ 受限 worker、provider 限速和取消
  ├─ 资产分类、封面提取、依赖记录
  └─ Reader/cache/tombstone 一致性

Existing Reader / ComicCover / LibraryStore
  ├─ Reader 前台优先
  ├─ ComicCover 读取本地生成的封面
  ├─ 图片文件夹 Document
  └─ page/raw 清理与已验证删除清理
```

Rust 暴露稳定的 FRB 方法/状态查询给 Dart；Dart 不直接操作 provider 凭据、下载直链或绕过 Rust 的 governor。可见卡片的按需请求如果发生，必须提交同一个任务键，由 engine 合并并回填状态。

## 5. Provider 适配契约

### 5.1 统一接口

每个云端 adapter 提供以下语义（具体命名在实现计划中根据现有 FRB 命名落地）：

```text
list(path, cursor) -> entries, next_cursor, complete
read_range(path, offset, length) -> validated bytes | range_unavailable
read_file_limited(path, max_bytes) -> bounded bytes
normalize_path(path) -> logical path
probe_capabilities(path/fingerprint) -> listing metadata, random read, range
```

`entries` 至少包含名称、逻辑路径、目录标志、大小和 mtime（provider 没有时为 unknown/0）。目录分页完成前不得写入删除 tombstone。各 provider 继续复用已有会话、鉴权、下行 URL、Range 和 SFTP seek 实现；adapter 只做归一化，不复制下载逻辑。

### 5.2 能力与错误

统一能力值为 `listingMetadata`、`randomRead`、`range`、`directImage`，值为 supported/unsupported/unknown。统一错误为：

- `authExpired`：暂停该来源并等待重新授权；
- `forbidden`：保留旧数据并显示权限错误；
- `notFound`：单项读取暂时失败时不直接删除，完整目录刷新后才可 tombstone；
- `rateLimited(retryAfter)`：遵守服务器退避时间；
- `transient`：有界指数退避；
- `rangeUnavailable`：封面占位，图片页按单文件上限降级；
- `malformed`：丢弃该批次并保留上一份完整清单；
- `cancelled`：不写入成功或删除状态。

Range 能力按资源指纹缓存，目录或内容指纹变化后重新探测。没有 mtime 的 provider 使用子清单哈希和短 TTL，不能把未知当成未变化或已删除。

## 6. 清单、Schema 与资产模型

### 6.1 权威状态

在现有 `library_index` 上增量增加 `asset_kind`、`content_fingerprint`、`scan_generation` 和目录 `listing_complete`（或等价迁移字段）。新增：

1. `remote_scan_state`：来源、状态（never_started/running/paused/complete/degraded）、模式（full/incremental）、generation、检查点、上次成功时间、脱敏错误码。
2. 目录 listing 状态：父目录、子清单哈希、分页游标/完成标志、扫描时间、能力版本。
3. `remote_cover_dependency`：逻辑漫画键、依赖路径、依赖指纹、封面 profile、状态和最近成功时间。

迁移必须是 additive、幂等、可回滚；旧索引可读，旧 JSON 快照只能作为一次性 v1→v2 输入，不能用旧快照推断删除。

### 6.2 资产分类

| 类型 | 进入封面队列 | 可作为 Reader 书籍 | 说明 |
|---|---:|---:|---|
| `archive_file` | 是 | 是 | 现有压缩包/电子书漫画 |
| `image_file` | 否（作为依赖） | 否 | 图片文件夹中的页文件 |
| `image_folder` | 是 | 是 | 直接子项含图片的目录 |
| `container_dir` | 由子项决定 | 否 | 只含子目录/压缩包的容器 |
| `plain_dir`/`other_file` | 否 | 否 | 仅用于导航 |

图片扩展复用 Rust `document::folder` 的 jpg/jpeg/png/webp/gif/bmp/avif 集合；忽略隐藏文件和系统目录，按自然排序确定页序及首图。容器封面依赖第一个稳定排序、已识别的子漫画。

### 6.3 指纹和 generation

文件指纹优先为规范化路径、目录标志、size、mtime；缺失字段由子清单哈希补足。封面缓存 key 由逻辑漫画键、封面参数和内容指纹组成。全量扫描使用 generation；目录完成后可提交局部结果，但来源只有在所有待处理目录完成后才标记 complete。中断或失败保留上一份完整 generation 和可恢复检查点。

## 7. 扫描生命周期与数据流

### 7.1 触发规则

1. 会话连接成功或首次打开已连接的云端根目录时，若来源为 `never_started`，提交一次 full job。
2. 相同来源的重复打开/授权只加入现有 job；不会创建第二个递归任务。
3. 后续打开/授权提交增量 job；根清单未变化时可快速结束，不递归未变化子树。
4. UI 提供增量重扫和全量重扫；全量重扫是用户主动核对删除的权威操作。
5. 自动任务受 `remoteBackgroundScanEnabled` 控制；手动任务是显式操作。`remoteCoverFetchEnabled=false` 时仍可更新清单，但封面 I/O 必须跳过并显示暂停状态。

### 7.2 全量扫描

```text
root list -> persist complete root -> queue changed/new dirs
          -> list each page -> classify -> checkpoint
          -> queue comic covers -> commit dependencies
          -> all work complete -> generation=complete
```

每个目录的旧完整清单保留到新分页全部成功；新清单与缺失项在事务中提交。某目录失败不会清空该目录，也不会让整个来源初始化为空库。

### 7.3 增量扫描

增量先比较根指纹，再只递归新增、删除、未完成或指纹变化的目录。子项 size/mtime/哈希变化会使图片文件夹和容器封面依赖失效。若 provider 无可靠版本字段，使用子清单哈希和短 TTL；仍无法判断时保守重列举，不进行危险清理。

## 8. 调度、取消与资源控制

### 8.1 共享请求预算

扫描 worker 使用与 Reader 相同的 Rust governor，并新增最低优先级的 scan lane。每来源和全局均有并发/队列上限，provider 另有 API rate gate。前台当前页保留至少一个槽位；扫描任务不得长期占满 Reader 预取预算。

### 8.2 去重和背压

任务键为 `(source_id, logical_path, content_fingerprint, cover_profile)`。同键可见封面、后台封面和重试任务合并；列表任务按目录和 generation 去重。列表批次和封面 body 逐项消费，不使用无界 `Future.wait` 或一次性收集整棵树。

### 8.3 暂停、取消和恢复

暂停阻止新任务，已开始的 I/O 可完成但结果在提交前检查 cancellation token。取消丢弃未开始任务、不写成功/删除状态，并保留最后一个完整目录检查点。进程被杀、系统挂起或网络断开后，恢复从检查点重新排队；已验证的封面任务可通过指纹命中缓存。

### 8.4 重试

429/5xx/断线采用有界指数退避并遵守 `Retry-After`；认证过期、权限拒绝、Range 不可用、malformed 和用户取消不重试。单项暂时 404 只记失败，不产生 tombstone。

## 9. 封面与图片文件夹 Reader

### 9.1 封面

扫描任务调用 provider 的安全局部读取策略，成功后写入 stable cover cache 和依赖表。`ComicCover` 只读本地缓存；没有结果时显示排队/Range 不可用/失败状态。Range 不可用不得隐式调用 `download_to_raw_cache`；自定义封面仍走现有显式整本下载确认流程。

### 9.2 `RemoteFolderBook`

Rust 新增远程图片文件夹 `Document`：输入为已提交的图片子清单和 provider session，页数和自然排序固定，逻辑书籍键使用文件夹路径。每页独立读取，Range 可用时使用随机读取；Range 不可用时只允许受 `max_page_bytes` 限制的单图片响应，不生成整本 raw。超过上限返回可解释的页错误，不破坏当前页状态。

Reader 继续使用现有前台优先 governor、L1/page cache、预取和 read record。压缩包的 `stream/download/auto` 不变；图片文件夹的 `download` 也不转换为单一整本 raw。

## 10. 缓存与远程删除一致性

### 10.1 阅读完成

`purge_remote_book_content_cache` 对图片文件夹和压缩包都按相同逻辑键删除 page/raw，先保留或迁移封面 alias；封面、元数据、书源、凭据、标签、历史、完成状态和自定义封面参数保持不变。

### 10.2 已验证 tombstone

当一个目录的所有分页成功、响应完整且来源会话有效时，比较旧/新清单并写缺失项 tombstone。删除文件夹需按路径前缀和依赖表处理子项；删除图片首图或改名会使文件夹封面失效并重新生成。随后清理漫画 cover、cover aliases、依赖图片 page/raw 和相关缓存。未完成扫描、认证失败、权限错误、截断响应、取消和单项暂时 404 绝不能调用该清理路径。

### 10.3 路径替换

同一路径的新 size/mtime/内容指纹视为新版本：旧封面不会被复用，成功新封面覆盖逻辑 key；旧 revision 的可回收缓存按既有 cache policy 处理。与该路径无依赖的漫画不受影响。

## 11. UI、设置与生命周期

- 来源页和来源根目录显示扫描状态、已处理/总数、最近成功时间和脱敏错误原因。
- 操作包括继续、暂停、失败重试、增量重扫和全量重扫；同来源点击合并。
- 新增 `remoteBackgroundScanEnabled` 默认开启；现有 `remoteCoverFetchEnabled` 仍是远程封面网络总开关。关闭前者只阻止自动任务，关闭后者阻止封面 I/O；两者都不能让已有本地封面消失。
- 应用生命周期只保证“进程内后台”；Android 系统挂起/进程终止由检查点恢复，不引入常驻服务。
- 全局 kill switch 将自动行为退回旧的可见卡片按需策略，保留清单和已经生成的封面。

## 12. 测试设计

### 12.1 单元与契约

- Rust：分类、自然排序、路径归一化、指纹、分页完整性、generation、检查点、队列优先级/去重/取消、依赖清理、Reader 页边界和内存上限。
- Dart：授权/首次打开触发一次、重启续跑、增量/全量入口、设置兼容、状态 UI 和可见请求合并。
- 五个 provider fake contract：分页、缺失 mtime、206/200/416/坏 Content-Range、429/403、认证失效、截断、删除和嵌套图片文件夹。

### 12.2 集成与性能

- Reader/cache 回归确认图片文件夹按页打开、完成清理保留封面和关键状态、失败刷新不清理。
- 20/100/300 MiB 和 50+ 页长条样本记录段数、wall time、吞吐、峰值 RSS、`.part`、重试/恢复、最终 SHA-256；确认扫描不抢占前台且内存固定上限。
- 每个 provider 至少一次真实脱敏 smoke；没有账号或样本时标记 pending，不用 fake 结果冒充真实通过。

### 12.3 工程门禁

按项目要求执行并记录：

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

格式和 Clippy 输出按本轮新增、既有 dirty、历史无关分类，只修本轮新增问题。

## 13. 分阶段交付、回滚与验收

### 13.1 子任务顺序

1. `remote-scan-manifest`：schema、adapter contract、指纹和 tombstone。
2. `remote-scan-worker`：共享调度、限速、检查点和封面批处理。
3. `remote-folder-reader`：图片文件夹 `Document`、按页读取和 Reader 接入。
4. `remote-cover-cleanup`：封面依赖、远程删除和清理回归。
5. `remote-scan-ui-verify`：Dart 触发、状态 UI、设置和跨 provider 证据。

各子任务独立可测试，但必须按上述依赖顺序合并；父任务负责跨层审查。

### 13.2 回滚

实现期间保留 kill switch。出现 provider 协议变化、Reader 延迟回归、误删或数据迁移异常时，关闭后台扫描并回到旧按需封面；保留数据库旧 generation、封面缓存和用户状态，禁止通过清空数据库“修复”。

### 13.3 放行判定

只有所有云端 provider 的首次全量、增量、手动重扫、Range 降级、取消恢复、封面保留、完整删除清理、图片文件夹阅读、Rust/Flutter/Windows 回归和内存/性能证据齐全，才可写 `GREEN / RELEASE CANDIDATE READY`。任何核心证据缺失或失败均保持 `YELLOW / DO NOT RELEASE`，并继续列出真实设备、真实 provider 或样本 pending 项。

## 14. 已确认决策

- 首次授权/首次打开触发全量，后续增量，手动重扫可选增量或全量。
- 范围为所有云端会话来源，不限 115；SMB/NAS 保持本地逻辑。
- 图片文件夹既要有封面，也必须可通过 Reader 按页阅读。
- 扫描可慢但必须后台、可暂停、可恢复，且前台流式阅读优先。
- 阅读完成清理保留封面；经过完整远程删除验证后清理失效封面及其依赖。
- M8 线上智能刮削继续作为未来插件，不进入本任务。
