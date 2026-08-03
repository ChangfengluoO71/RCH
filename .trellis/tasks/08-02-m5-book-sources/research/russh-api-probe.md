# russh API 探针结论（Step 0，2026-08-03）

## 环境

- Windows x64 MSVC，rustc/cargo 1.97.1，无 cmake / vcpkg / NASM；VS BuildTools 的 cl.exe 可用（cargo 自动定位）。

## 关键发现：必须关闭默认特性

`russh = "0.62.5"` 默认特性 = `["flate2", "aws-lc-rs", "rsa"]`，其中 **aws-lc-rs → aws-lc-sys 在 Windows x64 需要 NASM**，本机没有，构建直接 panic：

```
thread 'main' panicked at aws-lc-sys-0.43.0/builder/nasm_builder.rs:138: NASM command not found!
```

**可用组合**（已在临时工程验证编译通过）：

```toml
russh = { version = "0.62.5", default-features = false, features = ["flate2", "ring", "rsa"] }
```

- `ring`：提供 chacha20-poly1305 cipher（russh 必须启用 aws-lc-rs 或 ring 之一才能编译，见 `src/cipher/chacha20poly1305.rs` 的 cfg）；ring 0.17 在 Windows 用 cl.exe 构建（无 NASM 需求），已验证。
- `rsa`：纯 Rust RSA（host key 验证 / 服务器为 RSA 主机时需要）。
- `flate2`：SSH zlib 压缩（后端为纯 Rust miniz_oxide）。

## 确认的 API 形态（russh 0.62.5 + russh-sftp 2.3.0）

```rust
use russh::client;
use russh::keys::PublicKey; // = ssh_key::PublicKey（key::PublicKey 是私有的，勿用）

#[derive(Clone)]
struct Handler;
impl client::Handler for Handler {
    type Error = russh::Error;
    async fn check_server_key(&mut self, key: &PublicKey) -> Result<bool, Self::Error> {
        Ok(true) // 接受任意主机密钥；指纹 = key.fingerprint(HashAlg::Sha256)
    }
}

let config = Arc::new(client::Config::default());
let mut session = client::connect(config, (host, port), Handler).await?;
let authed = session.authenticate_password(user, pass).await?; // AuthResult::success()
let channel = session.channel_open_session().await?;           // Channel<client::Msg>
channel.request_subsystem(true, "sftp").await?;
let sftp = russh_sftp::client::SftpSession::new(channel.into_stream()).await?; // 注意 into_stream()
sftp.set_timeout(10);

let entries: Vec<DirEntry> = sftp.read_dir("/").await?;   // 同步收集的 Vec（Iterator），非流
// DirEntry: file_name() -> String; path() -> 已用 / 拼接的完整路径; file_type().is_dir(); metadata().len()
let md = sftp.metadata("/x.cbz").await?;                  // Metadata::len()/is_dir()（方法非字段）
let mut file = sftp.open("/x.cbz").await?;                // 只读；AsyncRead + AsyncSeek（seek 后 read）
file.seek(std::io::SeekFrom::Start(offset)).await?;
let n = file.read(&mut buf).await?;
sftp.close().await?;
```

## 对实现的约束

- `SftpSession` 内部是 `Arc<RawSftpSession>`，整体可放 `Arc<SftpSession>` 跨线程共享；每个请求由协议层多路复用。
- 随机读 = 每次 `open`（只读）→ `seek` → `read`；无 `read_at` API。每块读一次 open 往返可接受（SourceReader 256KB 读放大缓解）。
- 整本下载：`open` 后循环 `read` 分块写盘（勿用 `sftp.read(path)`——整包进内存）。
- 会话持有独立 `tokio::runtime::Runtime`，同步代码内 `runtime.block_on(...)`；只在 spawn_blocking 线程调用。
- 编译耗时：首次全量编译约 10-12 分钟（ring 等），后续增量快。
