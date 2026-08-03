# SFTP 书源实现规范（russh）

## 场景

RCH 的远程书源（WebDAV / SFTP）统一走「会话 + raw/ 缓存 + 流式回退」链路。SFTP 使用 russh 纯 Rust 栈实现随机读。

## 设计决策：russh over ssh2

**背景**：需要 SFTP 随机读（对齐 `ByteSource` 同步 trait）。

**选项**：
1. `ssh2`（libssh2 绑定）— 阻塞 API 天然契合，但 Windows MSVC 下 libssh2-sys 依赖 vcpkg 提供预编译库，且本机无 vcpkg/cmake/cc。
2. `russh + russh-sftp`（纯 Rust 异步）— 无 C 工具链依赖，但需桥接异步到同步。

**决策**：russh 0.62 + russh-sftp 2.3。桥接 = 每个会话持有独立 `tokio::runtime::Runtime`，同步代码内 `runtime.block_on(...)`，只在 `spawn_blocking` 线程调用（tokio worker 内 block_on 会 panic）。

## Gotcha：russh 默认特性需要 NASM

> **Warning**：`russh = "0.62.5"` 默认特性 `["flate2","aws-lc-rs","rsa"]` 中的 aws-lc-rs 在 Windows x64 构建需要 NASM 汇编器，本机没有会直接 panic（`aws-lc-sys builder/nasm_builder.rs: NASM command not found`）。

必须显式关闭默认特性，改用 ring：

```toml
russh = { version = "0.62.5", default-features = false, features = ["flate2", "ring", "rsa"] }
```

- `ring`：提供 chacha20-poly1305（russh 必须启用 aws-lc-rs 或 ring 之一才能编译，见 `src/cipher/chacha20poly1305.rs`）；ring 0.17 在 Windows 用 cl.exe 构建，无需 NASM。
- `rsa`：纯 Rust RSA（RSA 主机密钥验证需要）。

## 会话模式（模式约定）

```rust
// source/sftp.rs
pub struct SftpClient {
    runtime: Arc<tokio::runtime::Runtime>,     // 独立 multi_thread runtime
    _conn: russh::client::Handle<SshHandler>,  // 保持连接存活
    sftp: Arc<russh_sftp::client::SftpSession>,
    endpoint: String,                          // host:port，缓存命名空间
}
```

- 随机读 = 每次调用独立 `open`（只读）→ `seek` → `read`（russh-sftp 无 `read_at` API）；并发安全，上层 SourceReader 256KB 读放大缓解往返。
- 整本下载：循环 `read` 分块写盘，**勿用** `sftp.read(path)`（整包进内存）。
- 主机密钥：首次自动接受并 `tracing::info!` 记录 SHA256 指纹（简化 TOFU）。
- 打开策略（全局设置 `bookOpenStrategy`）：`"auto"` 先整本下载 raw/ 失败回退流式；`"download"` 强制整本失败报错；`"stream"` 直接流式。

## API 契约（FRB 生成）

- `sftp_connect(host, port, username, password) -> SftpSessionInfo { id, root: "/" }`
- `sftp_disconnect(id)` / `sftp_list(session, path) -> Vec<DirEntry>`
- `open_sftp_book(session, path, strategy) -> BookInfo`（`cache_ns = "sftp|{endpoint}|{path}"`）
- `sftp_download_progress(session) -> f64` / `sftp_has_raw_cache(session, path) -> bool`
- `sftp_cover(session, path, page, width, height, crop) -> PageImage`

书源 `type` 扩展为 `local | webdav | smb | sftp`；`book_sources` 表新增 `port INTEGER` 列（迁移：`PRAGMA table_info` 检测缺失时 `ALTER TABLE ADD COLUMN`，与 rotations/sort_order 同模式）。

## 常见错误

- **把 SFTP 当本地处理**：`!isWebDav` 分支默认走本地文件系统，会把远程路径当本地文件检查（如 `purgeStale` 误删记录）。远程判定统一用 `needsSession`（webdav+sftp），本地文件系统用 `isLocalFs`（local+smb）。
- **无 Range 的 WebDAV 在 stream 模式**：无法流式，只能整本下载（download/ 回退），不要对用户报"不支持"。
