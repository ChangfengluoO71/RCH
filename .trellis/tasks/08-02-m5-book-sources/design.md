# M5 书源扩展（SMB / SFTP）— 技术设计

## 1. 结论摘要

- **SMB**：Windows 原生 UNC 路径（`\\server\share`）接入，**零 Rust 改动**，复用 `LocalFile` + `list_dir` 本地链路，主要是 Dart 侧 UI / 校验 / 分发。
- **SFTP**：选用 **russh 0.62 + russh-sftp 2.3（纯 Rust）**，弃用 ssh2（libssh2 绑定）。
  - 原因：本机无 cmake / vcpkg / cc（已验证），libssh2-sys 在 Windows MSVC 下依赖 vcpkg 提供预编译库，构建成本高且环境不满足；russh 纯 Rust、仅 cargo 即可构建，与 flutter_rust_bridge cdylib 链路兼容。
  - 异步桥接：russh 是 async API；应用本身用 tokio + `spawn_blocking`，每个 SFTP 会话持有独立 `tokio::runtime::Runtime`，同步 `ByteSource::read_at` 内 `runtime.block_on(...)`。

## 2. 架构与数据流

```
Dart (BookSource type='smb'|'sftp')
  │
  ├─ smb  → listLocalDir / openLocalBook / bookCover   (UNC 路径直通 Rust local 链路)
  └─ sftp → sftp_connect → sftp_list / open_sftp_book / sftp_cover
                │
                └─ Rust: SftpClient { runtime, russh Handle, SftpSession }
                     ├─ open_sftp_book: 优先整本下载到 raw/ 缓存（有进度条）
                     │     失败 → 回退 SftpFile(ByteSource) 流式（seek+read 随机读）
                     └─ SftpFile::read_at: block_on(open → seek → read)
```

与 WebDAV 完全对称：`webdav_*` 会话 API 镜像为 `sftp_*`；打开策略一致（先整本下载 raw/ 缓存 → 回退流式），书 key / 封面 / 缓存命名空间沿用现有模式。

## 3. Rust 侧设计

### 3.1 新增 `src/source/sftp.rs`

- `SftpClient`（`Send + Sync`，存 `Arc`）：
  - 字段：`runtime: tokio::runtime::Runtime`、`handle: russh::client::Handle<Client>`、`sftp: russh_sftp::client::SftpSession`、`endpoint: String`（`host:port`，用于缓存命名空间）。
  - `connect(host, port, user, pass) -> Result<Self>`：TCP 连接 + 握手（host key 首次自动接受并记日志指纹）+ 密码认证 + `channel.request_subsystem(true, "sftp")` + `SftpSession::new`；设置 `set_timeout(10)`（默认 10s）。
  - `list(path) -> Result<Vec<Entry>>`：`read_dir` → `Entry { name, path: join(父路径, name), is_dir, size, mtime }`；目录在前 + 自然排序（复用 `crate::util::natural_cmp`）。
  - `file_size(path) -> Result<u64>`：`metadata`。
  - `read_at(path, offset, buf) -> io::Result<usize>`：block_on 打开只读 File → seek(offset) → read；**每次调用独立开句柄**（无跨线程状态，天然并发安全；代价是每块 1 次 open 往返，先用 256KB 读放大缓存缓解，后续可优化句柄池）。
  - `download_full(path, local_path, progress) -> Result<()>`：循环 open + read 分块写盘（不整包进内存），更新 `DownloadProgress`。
  - `disconnect()`：`sftp.close()` + drop runtime。
- `SftpFile` 实现 `ByteSource`：`len` 来自 stat，`read_at` 委托 `client.read_at`。
- 路径工具（可单测）：`join_remote_path(base, name)`（`/` 分隔、根 `/` 特殊处理）、`parse_endpoint(addr) -> (host, port)`（默认 22）。

### 3.2 `src/api/source.rs` 新增（FRB 需 regenerate）

- 会话表：`SFTP_SESSIONS: OnceLock<Mutex<HashMap<u64, Arc<SftpClient>>>>`（镜像 WebDAV `SESSIONS`）。
- 新 API：
  - `sftp_connect(host, port, username, password) -> SftpSessionInfo { id, root }`（root = `/`）
  - `sftp_disconnect(id)`
  - `sftp_list(session, path) -> Vec<DirEntry>`
  - `open_sftp_book(session, path, strategy) -> BookInfo`；strategy 由全局设置传入（见 §4.5）：
    - `auto`（默认）：先 `download_to_raw_cache`（sftp 版本，hash = `endpoint+path`，写 raw/），失败回退 `SftpFile` 流式；
    - `download`：强制整本下载，无缓存且下载失败则报错，不静默转流式；
    - `stream`：直接 `SftpFile` 流式打开，不触发整本下载。
  - `cache_ns = "sftp|{endpoint}|{path}"`。
  - `sftp_download_progress(session) -> f64`
  - `sftp_has_raw_cache(session, path) -> bool`
  - `sftp_cover(session, path, page, width, height, crop) -> PageImage`（先 cover/ 磁盘缓存 → raw/ 本地缓存 → 流式解码，镜像 `webdav_cover`）
- 错误提示：连接拒绝 / 认证失败 / 超时 / 路径不存在 → 中文提示（镜像 WebDAV 风格）。

### 3.3 依赖

```toml
russh = { version = "0.62.5", default-features = false, features = ["flate2", "ring", "rsa"] }
russh-sftp = "2.3.0"
```

**为什么关闭默认特性**（探针实测，见 `research/russh-api-probe.md`）：默认特性含 `aws-lc-rs`，其 aws-lc-sys 在 Windows x64 构建需要 NASM（本机没有）；改用 `ring` 提供 chacha20-poly1305（cl.exe 可构建，无需 NASM），`rsa` 为纯 Rust。打包体积增加可接受（静态进 cdylib，无外部 DLL）。

## 4. Dart 侧设计

### 4.1 `models.dart`

- `BookSource.type` 扩展为 `'local' | 'webdav' | 'smb' | 'sftp'`。
- 新增字段 `int? port`（仅 SFTP，默认 22）；`toJson/fromJson` 补 `port`（旧 JSON 无此字段 → null，兼容）。
- `AppSettings` 新增 `BookOpenStrategy bookOpenStrategy`（默认 `auto`）；`toJson/fromJson` 缺省回退 `auto`，经现有 saveSettings 链路自动落入 SQLite `app_settings`，无需改 Rust 设置层。
- 字段映射：
  - SMB：`path` = UNC 根目录（`\\server\share`），无 url/username/password。
  - SFTP：`url` = 服务器地址（host 或 host:port）、`port`、`username`、`password`、`path` = 初始远程目录（默认 `/`）。
- Getter：`isWebDav` / `isSftp` / `isSmb` / `isLocalFs`（local+smb，走本地文件系统链路）/ `needsSession`（webdav+sftp）。
- `capabilityDisplay`：SMB → 🟢「本地/NAS」；SFTP → 🟡「SFTP」。

### 4.2 会话缓存 `store/sftp_session.dart`

镜像 `webdav_session.dart`：`sftpSessionFor(BookSource) -> Future<BigInt>`，按 sourceId 缓存会话，失败抛中文错误。

### 4.5 全局设置（新增）

- **打开策略**（设置页新增「远程书源」区块，`SegmentedButton` 三选一）：
  - `auto` 自动（推荐，默认）：先整本下载到缓存（有进度条），失败回退流式；
  - `download` 优先下载整本：有进度条、之后秒开，适合网速快 / 服务器弱 / 想离线读；
  - `stream` 直接流式：即点即读、不占缓存，适合网速慢 / 大文件（SFTP 协议原生支持随机读，WebDAV 依赖服务器 Range 支持）。
  - 生效范围：WebDAV 与 SFTP 打开书籍共用此设置；本地 / SMB 本就直读，不受影响。
- **自动转 CBZ**（沿用现有全局开关 `autoConvertCbz`，更新设置页副标题说明适用范围）：作用于 `local` + `smb`（UNC 文件系统路径，用户有写权限时生效）；`webdav` / `sftp` 无远端写能力，不适用。

### 4.3 UI（`home_page.dart`）

- 添加书源对话框：SegmentedButton 扩为 4 段（本地目录 / WebDAV / SMB / SFTP），按类型切换字段：
  - 本地目录：目录路径
  - WebDAV：服务器地址 / 用户名 / 密码 / 初始路径
  - SMB：UNC 路径（hint `\\192.168.1.10\comic`），添加时 `listLocalDir(root)` 连通性测试（WinError 53/5 → 中文提示）
  - SFTP：服务器地址 / 端口(默认22) / 用户名 / 密码 / 初始路径(默认`/`)，添加时 `sftp_connect` + `sftp_list` 测试
- 编辑书源对话框：按类型显示对应字段（同上映射）。
- 书源列表图标 / 详情文案：按类型区分。

### 4.4 分发点三路改造

| 位置 | 现状 | 改造 |
|---|---|---|
| `source_browser.dart` | `isWebDav ? webdavList : listLocalDir` | local/smb → `listLocalDir`；webdav → `webdavList`；sftp → `sftpList`（`needsSession` 分支） |
| `source_browser.dart` 自动转 CBZ / 漫画文件夹检测 | 仅非 WebDAV | 自动转 CBZ 按全局开关执行，目标范围 `local` + `smb`（webdav/sftp 无写能力跳过）；文件夹检测 local+smb 复用本地链路，sftp 跳过 |
| `book_detail_page.dart` | `isWebDav ? openWebdavBook : openLocalBook` | 增加 sftp 分支 `openSftpBook` |
| `ai_upscale_manager.dart` | 同上 | 增加 sftp 分支 |
| `comic_cover.dart` | `bookCover` / `webdavCover` | 增加 `sftpCover`（session 来自 `sftpSessionFor`） |

书 key 仍由 `bookKeyOf(type, sourceId, path)` 生成，阅读记录 / 标签 / 搜索 / `removeSourceWithCleanup` 均无需改动。

**打开 API 签名变更**：`open_webdav_book` / `open_sftp_book` 均增加 `strategy` 参数（Rust 侧用字符串 `"auto"|"download"|"stream"`，FRB 生成成本最低；Dart wrapper 默认 `auto`）。所有调用点（`book_detail_page`、`ai_upscale_manager`）统一从 `AppSettings.bookOpenStrategy` 读取，旧调用不传则保持现状行为。

## 5. 兼容性与回滚

- **无 DB schema 变更**；书源 JSON 只增 `port` 字段（可选），旧数据可读。
- 回滚：移除 Cargo.toml 依赖 + 删除 sftp 模块 / API / Dart 分支即可，不动数据层。
- 已知边界：SMB 带用户名/密码的共享本版本不支持（匿名 / 已映射访问；后续可加 `net use` / Credential Manager）；SFTP 仅密码认证（PRD 范围）。
- 不自动转 CBZ 到 WebDAV/SFTP（无远端写能力）；SMB 是否转换由全局开关 + 共享写权限决定。

## 6. 风险与对策

| 风险 | 对策 |
|---|---|
| russh 0.62 API 形态与文档有出入 | implement.md 步骤 0：独立最小工程先验证 connect/auth/sftp/read_dir/seek-read，再进主仓 |
| `block_on` 在 tokio 工作线程内调用 | 会话持有独立 runtime；所有 block_on 只发生在 `spawn_blocking` 线程（非 tokio worker） |
| 每次 read_at 开句柄的往返开销 | SourceReader 已有 256KB 读放大；后续可加句柄池（设计预留，不在首版范围） |
| SFTP 服务器兼容性（read_dir/metadata 扩展） | 协议 v3 基础操作；失败回退整本下载 raw/ 缓存（与 WebDAV 同策略） |
| cdylib 打包引入新依赖失败 | 步骤 8 做 `cargo build --release` + `flutter build windows --release` 冒烟 |
