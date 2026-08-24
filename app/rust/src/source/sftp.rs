//! SFTP 书源：russh(纯 Rust) + russh-sftp，随机读走 open→seek→read。
//!
//! 与 WebDAV 对称：会话持有独立 tokio runtime（异步库桥接同步 `ByteSource`），
//! 打开策略由 API 层按全局设置决定（整本下载到 raw/ 缓存或直接流式）。

use super::{webdav::DownloadProgress, ByteSource, Entry};
use anyhow::{anyhow, Context, Result};
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncSeekExt};

/// russh 客户端 handler：接受任意主机密钥，记录指纹。
#[derive(Clone)]
struct SshHandler;

impl russh::client::Handler for SshHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &russh::keys::PublicKey,
    ) -> Result<bool, Self::Error> {
        tracing::info!(
            "SFTP 主机密钥指纹: {}",
            server_public_key.fingerprint(russh::keys::HashAlg::Sha256)
        );
        Ok(true)
    }
}

/// SFTP 客户端：russh 连接 + SftpSession，外加独立 tokio runtime 供同步桥接。
pub struct SftpClient {
    /// 独立 runtime：所有 async 调用经 `block_on` 同步执行。
    runtime: Arc<tokio::runtime::Runtime>,
    /// 保持连接存活（SftpSession 依赖 channel；drop Handle 会断开）。
    _conn: russh::client::Handle<SshHandler>,
    sftp: Arc<russh_sftp::client::SftpSession>,
    /// `host:port`，用于 raw/ 缓存命名空间（与 WebDAV origin 对称）。
    endpoint: String,
}

impl SftpClient {
    /// 连接 + 密码认证 + 打开 SFTP 子系统。
    /// 阻塞调用（内部 block_on）；应在 spawn_blocking 中执行。
    pub fn connect(host: &str, port: u16, user: &str, pass: &str) -> Result<Self> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .context("创建 SFTP runtime 失败")?;
        let host = host.to_string();
        let user = user.to_string();
        let pass = pass.to_string();
        let (conn, sftp, endpoint) = runtime.block_on(async move {
            let connect_fut = async {
                let config = Arc::new(russh::client::Config::default());
                let mut session = russh::client::connect(config, (host.as_str(), port), SshHandler)
                    .await
                    .map_err(|e| anyhow!("连接 SFTP 服务器失败: {e}"))?;
                let authed = session
                    .authenticate_password(user.as_str(), pass.as_str())
                    .await
                    .map_err(|e| anyhow!("SFTP 认证失败: {e}"))?;
                if !authed.success() {
                    return Err(anyhow!("SFTP 认证失败: 用户名或密码错误"));
                }
                let channel = session
                    .channel_open_session()
                    .await
                    .map_err(|e| anyhow!("打开 SFTP channel 失败: {e}"))?;
                channel
                    .request_subsystem(true, "sftp")
                    .await
                    .map_err(|e| anyhow!("启动 SFTP 子系统失败: {e}"))?;
                let sftp = russh_sftp::client::SftpSession::new(channel.into_stream())
                    .await
                    .map_err(|e| anyhow!("初始化 SFTP 会话失败: {e}"))?;
                sftp.set_timeout(10);
                Ok::<
                    (
                        russh::client::Handle<SshHandler>,
                        russh_sftp::client::SftpSession,
                    ),
                    anyhow::Error,
                >((session, sftp))
            };
            let (conn, sftp) = tokio::time::timeout(Duration::from_secs(20), connect_fut)
                .await
                .map_err(|_| anyhow!("连接 SFTP 服务器超时(20s)"))?
                .map_err(|e: anyhow::Error| e)?;
            let endpoint = if port == 22 {
                host.clone()
            } else {
                format!("{host}:{port}")
            };
            Ok::<_, anyhow::Error>((conn, sftp, endpoint))
        })?;
        Ok(SftpClient {
            runtime: Arc::new(runtime),
            _conn: conn,
            sftp: Arc::new(sftp),
            endpoint,
        })
    }

    /// 缓存命名空间标识（`host:port`，默认端口省略）。
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// raw/ 缓存路径探测（与 WebDAV 同名逻辑，命名空间用 endpoint）。
    pub fn raw_cache_path(&self, path: &str) -> Option<PathBuf> {
        raw_cache_path(&self.endpoint, path)
    }

    /// 列出目录（目录在前，自然排序；与本地/WebDAV 一致）。
    pub fn list(&self, path: &str) -> Result<Vec<Entry>> {
        let sftp = Arc::clone(&self.sftp);
        let path = path.to_string();
        let dir = self
            .runtime
            .block_on(async move { sftp.read_dir(path).await })
            .map_err(|e| anyhow!("SFTP 列目录失败: {e}"))?;
        let mut out: Vec<Entry> = dir
            .into_iter()
            .map(|e| Entry {
                name: e.file_name(),
                path: e.path(),
                is_dir: e.file_type().is_dir(),
                size: e.metadata().len(),
                mtime: e
                    .metadata()
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0),
            })
            .collect();
        out.sort_by(|a, b| {
            b.is_dir
                .cmp(&a.is_dir)
                .then_with(|| crate::util::natural_cmp(&a.name, &b.name))
        });
        Ok(out)
    }

    /// 文件大小（SFTP stat）。
    pub fn file_size(&self, path: &str) -> Result<u64> {
        let sftp = Arc::clone(&self.sftp);
        let path = path.to_string();
        let md = self
            .runtime
            .block_on(async move { sftp.metadata(path).await })
            .map_err(|e| anyhow!("SFTP 获取文件大小失败: {e}"))?;
        Ok(md.len())
    }

    /// 随机读：每次调用独立打开只读句柄 → seek → read。
    /// 并发安全（无跨线程共享状态）；读放大由上层 SourceReader(256KB) 缓解。
    pub fn read_at(&self, path: &str, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let sftp = Arc::clone(&self.sftp);
        let path = path.to_string();
        self.runtime.block_on(async move {
            let mut f = sftp.open(path).await.map_err(|e| {
                io::Error::new(io::ErrorKind::Other, format!("SFTP 打开文件失败: {e}"))
            })?;
            f.seek(io::SeekFrom::Start(offset)).await.map_err(|e| {
                io::Error::new(io::ErrorKind::Other, format!("SFTP seek 失败: {e}"))
            })?;
            let n = f
                .read(buf)
                .await
                .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("SFTP 读取失败: {e}")))?;
            Ok::<usize, io::Error>(n)
        })
    }

    /// 整本下载到 raw/ 缓存目录（与 WebDAV 相同命名规则）。
    /// 已有缓存（非空）直接复用；返回本地路径。
    pub fn download_to_raw_cache(
        &self,
        path: &str,
        progress: Option<Arc<DownloadProgress>>,
    ) -> Result<PathBuf> {
        use std::io::Write;

        let raw_dir = crate::cache::CacheDir::Raw
            .ensure()
            .context("创建 raw/ 缓存目录失败")?;
        let name = path.rsplit('/').next().unwrap_or("file.cbz");
        let hash = cache_hash(&self.endpoint, path);
        let dir = raw_dir.join(&hash);
        std::fs::create_dir_all(&dir).ok();
        let file_path = dir.join(name);

        // 已有缓存(非空)则直接复用
        if let Ok(meta) = std::fs::metadata(&file_path) {
            if meta.len() > 0 {
                if let Some(p) = &progress {
                    p.downloaded.store(
                        p.total.load(std::sync::atomic::Ordering::SeqCst),
                        std::sync::atomic::Ordering::SeqCst,
                    );
                }
                return Ok(file_path);
            }
        }

        let total = self.file_size(path)?;
        if let Some(p) = &progress {
            p.total.store(total, std::sync::atomic::Ordering::SeqCst);
        }

        let sftp = Arc::clone(&self.sftp);
        let path = path.to_string();
        let file_path_for_write = file_path.clone();
        let progress_clone = progress;
        self.runtime
            .block_on(async move {
                let mut remote = sftp
                    .open(path)
                    .await
                    .map_err(|e| anyhow!("SFTP 打开文件失败: {e}"))?;
                let mut disk =
                    std::fs::File::create(&file_path_for_write).context("创建缓存文件失败")?;
                let mut buf = vec![0u8; 64 * 1024];
                let mut written: u64 = 0;
                loop {
                    let n = remote
                        .read(&mut buf)
                        .await
                        .map_err(|e| anyhow!("SFTP 下载读取失败: {e}"))?;
                    if n == 0 {
                        break;
                    }
                    std::io::Write::write_all(&mut disk, &buf[..n]).context("写入缓存文件失败")?;
                    written += n as u64;
                    if let Some(p) = &progress_clone {
                        p.downloaded
                            .store(written, std::sync::atomic::Ordering::SeqCst);
                    }
                }
                disk.flush().context("同步缓存文件失败")?;
                Ok::<(), anyhow::Error>(())
            })
            .context("SFTP 整本下载失败")?;

        Ok(file_path)
    }

    /// 关闭会话（释放连接与 runtime）。
    pub fn disconnect(&self) {
        let sftp = Arc::clone(&self.sftp);
        let _ = self.runtime.block_on(async move { sftp.close().await });
    }
}

/// SFTP 远程文件作为 [`ByteSource`]：read_at 委托给会话（open→seek→read）。
pub struct SftpFile {
    client: Arc<SftpClient>,
    path: String,
    len: u64,
}

impl SftpFile {
    pub fn new(client: Arc<SftpClient>, path: String, len: u64) -> Self {
        SftpFile { client, path, len }
    }
}

impl ByteSource for SftpFile {
    fn len(&self) -> u64 {
        self.len
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        self.client.read_at(&self.path, offset, buf)
    }
}

/// 远程路径拼接（SFTP 恒用 `/`）。
pub fn join_remote_path(base: &str, name: &str) -> String {
    if base.is_empty() || base == "/" {
        if base == "/" {
            return format!("/{}", name.trim_start_matches('/'));
        }
        return name.to_string();
    }
    let base = base.trim_end_matches('/');
    format!("{}/{}", base, name.trim_start_matches('/'))
}

/// raw/ 缓存目录 hash（namespace = endpoint，与 WebDAV 规则一致）。
pub fn cache_hash(endpoint: &str, path: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    format!("{endpoint}{path}").hash(&mut h);
    format!("{:016x}", h.finish())
}

/// 探测 raw/ 缓存文件（非空视为已缓存）。
pub fn raw_cache_path(endpoint: &str, path: &str) -> Option<PathBuf> {
    let name = path.rsplit('/').next().unwrap_or("file.cbz");
    let file_path = crate::cache::CacheDir::Raw
        .ensure()
        .ok()?
        .join(cache_hash(endpoint, path))
        .join(name);
    match std::fs::metadata(&file_path) {
        Ok(meta) if meta.len() > 0 => Some(file_path),
        _ => None,
    }
}

/// 解析 `host` / `host:port` / `[ipv6]:port` / 裸 IPv6 → (host, port)，默认 22。
pub fn parse_endpoint(addr: &str) -> (String, u16) {
    let a = addr.trim();
    if let Some(rest) = a.strip_prefix('[') {
        if let Some(idx) = rest.find(']') {
            let host = &rest[..idx];
            let tail = &rest[idx + 1..];
            if let Some(p) = tail.strip_prefix(':') {
                if let Ok(port) = p.parse::<u16>() {
                    if port != 0 {
                        return (host.to_string(), port);
                    }
                }
            }
            return (host.to_string(), 22);
        }
    }
    if let Some((h, p)) = a.rsplit_once(':') {
        if !h.is_empty() && !h.contains(':') {
            if let Ok(port) = p.parse::<u16>() {
                if port != 0 {
                    return (h.to_string(), port);
                }
            }
        }
        // 裸 IPv6（如 `::1`）或非法端口：按整体当 host，默认端口。
    }
    (a.to_string(), 22)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_remote_path_works() {
        assert_eq!(join_remote_path("/", "漫画.cbz"), "/漫画.cbz");
        assert_eq!(join_remote_path("/comic", "a.cbz"), "/comic/a.cbz");
        assert_eq!(join_remote_path("/comic/", "a.cbz"), "/comic/a.cbz");
        assert_eq!(join_remote_path("", "a.cbz"), "a.cbz");
    }

    #[test]
    fn parse_endpoint_works() {
        assert_eq!(parse_endpoint("nas.local"), ("nas.local".into(), 22));
        assert_eq!(parse_endpoint("nas.local:2222"), ("nas.local".into(), 2222));
        assert_eq!(parse_endpoint("[::1]:2222"), ("::1".into(), 2222));
        assert_eq!(parse_endpoint("[::1]"), ("::1".into(), 22));
        assert_eq!(parse_endpoint("::1"), ("::1".into(), 22));
        assert_eq!(parse_endpoint("  nas:22  "), ("nas".into(), 22));
    }
}
