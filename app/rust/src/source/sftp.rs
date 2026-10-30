//! SFTP 书源：russh(纯 Rust) + russh-sftp，随机读走 open→seek→read。
//!
//! 与 WebDAV 对称：会话持有独立 tokio runtime（异步库桥接同步 `ByteSource`），
//! 打开策略由 API 层按全局设置决定（整本下载到 raw/ 缓存或直接流式）。

use super::{webdav::DownloadProgress, ByteSource, Entry};
use anyhow::{anyhow, Context, Result};
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncSeekExt};

/// russh client handler with explicit host-key pinning.
#[derive(Clone)]
struct SshHandler {
    expected_fingerprint: Option<String>,
    allow_unpinned: bool,
    observed_fingerprint: Arc<Mutex<Option<String>>>,
}

impl russh::client::Handler for SshHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &russh::keys::PublicKey,
    ) -> Result<bool, Self::Error> {
        let fingerprint = server_public_key
            .fingerprint(russh::keys::HashAlg::Sha256)
            .to_string();
        if let Ok(mut observed) = self.observed_fingerprint.lock() {
            *observed = Some(fingerprint.clone());
        }
        Ok(self.allow_unpinned
            || self
                .expected_fingerprint
                .as_deref()
                .is_some_and(|expected| fingerprint_matches(expected, &fingerprint)))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SftpAuthMethod {
    Password,
    PrivateKey,
}

fn resolve_auth_method(method: &str, password: &str, private_key: &str) -> Result<SftpAuthMethod> {
    match method {
        "password" if !password.is_empty() => Ok(SftpAuthMethod::Password),
        "password" => Err(anyhow!("SFTP password authentication requires a password")),
        "privateKey" if !private_key.trim().is_empty() => Ok(SftpAuthMethod::PrivateKey),
        "privateKey" => Err(anyhow!(
            "SFTP public-key authentication requires a private key"
        )),
        _ => Err(anyhow!("Unsupported SFTP authentication method")),
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
    host_fingerprint: String,
}

impl SftpClient {
    /// 连接 + 密码认证 + 打开 SFTP 子系统。
    /// 阻塞调用（内部 block_on）；应在 spawn_blocking 中执行。
    pub fn connect(
        host: &str,
        port: u16,
        user: &str,
        auth_method: &str,
        pass: &str,
        private_key: &str,
        key_passphrase: &str,
        expected_fingerprint: &str,
    ) -> Result<Self> {
        let auth_method = resolve_auth_method(auth_method, pass, private_key)?;
        if expected_fingerprint.trim().is_empty() {
            return Err(anyhow!("SFTP host key must be trusted before connecting"));
        }
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .context("create SFTP runtime")?;
        let host = host.to_string();
        let user = user.to_string();
        let pass = pass.to_string();
        let private_key = private_key.to_string();
        let key_passphrase = key_passphrase.to_string();
        let expected_fingerprint = expected_fingerprint.trim().to_string();
        let observed_fingerprint = Arc::new(Mutex::new(None));
        let observed_for_connect = Arc::clone(&observed_fingerprint);
        let (conn, sftp, endpoint, host_fingerprint) = runtime.block_on(async move {
            let connect_fut = async {
                let handler = SshHandler {
                    expected_fingerprint: Some(expected_fingerprint.clone()),
                    allow_unpinned: false,
                    observed_fingerprint: observed_for_connect,
                };
                let mut session = russh::client::connect(
                    Arc::new(russh::client::Config::default()),
                    (host.as_str(), port),
                    handler,
                )
                .await
                .map_err(|_| anyhow!("SFTP connection failed or saved host key changed"))?;
                let authed = match auth_method {
                    SftpAuthMethod::Password => session
                        .authenticate_password(user.as_str(), pass.as_str())
                        .await
                        .map_err(|_| anyhow!("SFTP authentication failed"))?,
                    SftpAuthMethod::PrivateKey => {
                        let key = russh::keys::decode_secret_key(
                            &private_key,
                            (!key_passphrase.is_empty()).then_some(key_passphrase.as_str()),
                        )
                        .map_err(|_| anyhow!("SFTP private key or passphrase is invalid"))?;
                        let rsa_hash = session
                            .best_supported_rsa_hash()
                            .await
                            .map_err(|_| anyhow!("SFTP public-key authentication is unavailable"))?
                            .unwrap_or(Some(russh::keys::HashAlg::Sha256));
                        let key = russh::keys::PrivateKeyWithHashAlg::new(Arc::new(key), rsa_hash);
                        session
                            .authenticate_publickey(user.as_str(), key)
                            .await
                            .map_err(|_| anyhow!("SFTP public-key authentication failed"))?
                    }
                };
                if !authed.success() {
                    return Err(anyhow!("SFTP authentication failed"));
                }
                let channel = session
                    .channel_open_session()
                    .await
                    .map_err(|_| anyhow!("could not open the SFTP channel"))?;
                channel
                    .request_subsystem(true, "sftp")
                    .await
                    .map_err(|_| anyhow!("could not start the SFTP subsystem"))?;
                let sftp = russh_sftp::client::SftpSession::new(channel.into_stream())
                    .await
                    .map_err(|_| anyhow!("could not initialize the SFTP subsystem"))?;
                sftp.set_timeout(10);
                let host_fingerprint = observed_fingerprint
                    .lock()
                    .ok()
                    .and_then(|value| value.clone())
                    .ok_or_else(|| anyhow!("SFTP server did not provide a host fingerprint"))?;
                if !fingerprint_matches(&expected_fingerprint, &host_fingerprint) {
                    return Err(anyhow!("SFTP server host key changed"));
                }
                Ok::<
                    (
                        russh::client::Handle<SshHandler>,
                        russh_sftp::client::SftpSession,
                        String,
                    ),
                    anyhow::Error,
                >((session, sftp, host_fingerprint))
            };
            let (conn, sftp, host_fingerprint) =
                tokio::time::timeout(Duration::from_secs(20), connect_fut)
                    .await
                    .map_err(|_| anyhow!("SFTP connection timed out"))?
                    .map_err(|error: anyhow::Error| error)?;
            let endpoint = endpoint_for(&host, port);
            Ok::<_, anyhow::Error>((conn, sftp, endpoint, host_fingerprint))
        })?;
        Ok(SftpClient {
            runtime: Arc::new(runtime),
            _conn: conn,
            sftp: Arc::new(sftp),
            endpoint,
            host_fingerprint,
        })
    }

    pub fn probe_host_key(host: &str, port: u16) -> Result<String> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .context("create SFTP probe runtime")?;
        let host = host.to_string();
        let observed_fingerprint = Arc::new(Mutex::new(None));
        let observed_for_handler = Arc::clone(&observed_fingerprint);
        runtime.block_on(async move {
            let handler = SshHandler {
                expected_fingerprint: None,
                allow_unpinned: true,
                observed_fingerprint: observed_for_handler,
            };
            let session = tokio::time::timeout(
                Duration::from_secs(20),
                russh::client::connect(
                    Arc::new(russh::client::Config::default()),
                    (host.as_str(), port),
                    handler,
                ),
            )
            .await
            .map_err(|_| anyhow!("SFTP host-key probe timed out"))?
            .map_err(|_| anyhow!("SFTP host-key probe failed"))?;
            let fingerprint = observed_fingerprint
                .lock()
                .ok()
                .and_then(|value| value.clone())
                .ok_or_else(|| anyhow!("SFTP server did not provide a host fingerprint"))?;
            let _ = session
                .disconnect(
                    russh::Disconnect::ByApplication,
                    "host-key probe only",
                    "en",
                )
                .await;
            Ok(fingerprint)
        })
    }

    /// 缓存命名空间标识（`host:port`，默认端口省略）。
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub fn host_fingerprint(&self) -> &str {
        &self.host_fingerprint
    }

    /// raw/ 缓存路径探测（与 WebDAV 同名逻辑，命名空间用 endpoint）。
    pub fn raw_cache_path(&self, path: &str) -> Option<PathBuf> {
        raw_cache_path(&self.endpoint, path)
    }

    /// 列出目录（目录在前，自然排序；与本地/WebDAV 一致）。
    ///
    /// 兼容性说明（2026-10-30，OpenList 4.2.4）：
    /// * 空路径归一为 `/`。OpenList 的 sftpd 会把 OPENDIR 的空路径存成 `""`，
    ///   后续 READDIR 因 `getDir(handle) == ""` 直接返回 invalid handle。
    /// * 使用独立的 raw 通道逐包 READDIR：OpenList sftpd 在目录读完时只 `continue`，
    ///   不发送 `SSH_FXP_STATUS(EOF)`，`SftpSession::read_dir` 会一直等到超时。
    ///   这里把 `Timeout` 视为目录结束（OpenList 兼容），标准服务器仍以显式 EOF 结束。
    pub fn list(&self, path: &str) -> Result<Vec<Entry>> {
        let path = effective_list_path(path);
        let parent = path.clone();
        let files = self.runtime.block_on(async {
            let channel = self
                ._conn
                .channel_open_session()
                .await
                .map_err(|e| anyhow!("SFTP 列目录失败: cannot open channel: {e}"))?;
            channel
                .request_subsystem(true, "sftp")
                .await
                .map_err(|e| anyhow!("SFTP 列目录失败: cannot start subsystem: {e}"))?;
            let raw = russh_sftp::client::RawSftpSession::new(channel.into_stream());
            raw.init()
                .await
                .map_err(|e| anyhow!("SFTP 列目录失败: {e}"))?;
            raw.set_timeout(5);
            let handle = raw
                .opendir(path)
                .await
                .map_err(|e| anyhow!("SFTP 列目录失败: {e}"))?;
            let mut files = Vec::new();
            loop {
                match raw.readdir(handle.handle.clone()).await {
                    Ok(name) => {
                        if name.files.is_empty() {
                            break;
                        }
                        files.extend(name.files);
                    }
                    Err(russh_sftp::client::error::Error::Status(status))
                        if status.status_code == russh_sftp::protocol::StatusCode::Eof =>
                    {
                        break;
                    }
                    // OpenList sftpd 不发 EOF 状态包；等满 5s 即视为目录结束。
                    Err(russh_sftp::client::error::Error::Timeout) => break,
                    Err(error) => {
                        let _ = tokio::time::timeout(
                            Duration::from_secs(2),
                            raw.close(handle.handle.clone()),
                        )
                        .await;
                        return Err(anyhow!("SFTP 列目录失败: {error}"));
                    }
                }
            }
            let _ = tokio::time::timeout(Duration::from_secs(2), raw.close(handle.handle.clone()))
                .await;
            Ok::<_, anyhow::Error>(
                files
                    .into_iter()
                    .filter(|file| file.filename != "." && file.filename != "..")
                    .map(|file| Entry {
                        name: file.filename.clone(),
                        path: readdir_child_path(&parent, &file.filename),
                        is_dir: file.attrs.is_dir(),
                        size: file.attrs.len(),
                        mtime: file.attrs.mtime.map(i64::from).unwrap_or(0),
                    })
                    .collect::<Vec<_>>(),
            )
        })?;
        let mut out = files;
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
                let mut writer = crate::cache::AtomicCacheFile::create(&file_path_for_write)?;
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
                    writer.write_all(&buf[..n])?;
                    written += n as u64;
                    if let Some(p) = &progress_clone {
                        p.downloaded
                            .store(written, std::sync::atomic::Ordering::SeqCst);
                    }
                }
                writer.commit()?;
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

fn fingerprint_matches(expected: &str, actual: &str) -> bool {
    let expected = expected.trim();
    let actual = actual.trim();
    !expected.is_empty() && !actual.is_empty() && expected == actual
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

/// 列目录前归一化根路径：空/空白视为 `/`。
///
/// OpenList sftpd 把 OPENDIR 的 `""` 存成空串，后续 READDIR 会直接返回
/// invalid handle；而 RCH 的空初始路径历史上会传 `""`。
fn effective_list_path(path: &str) -> String {
    if path.trim().is_empty() {
        "/".to_string()
    } else {
        path.to_string()
    }
}

/// READDIR 子项路径，规则与 `russh_sftp::client::fs::ReadDir` 一致。
fn readdir_child_path(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_string()
    } else if parent.ends_with('/') {
        format!("{parent}{name}")
    } else {
        format!("{parent}/{name}")
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
/// raw/ 缓存的**确定性候选路径**（P1-D-2，Family 1）。
///
/// 复用既有 `cache_hash`，逐字保持 hash / 目录 / 文件名，**零 filesystem probe**。
pub fn raw_cache_candidate_path(endpoint: &str, path: &str) -> PathBuf {
    let name = path.rsplit('/').next().unwrap_or("file.cbz");
    crate::cache::CacheDir::Raw
        .path()
        .join(cache_hash(endpoint, path))
        .join(name)
}

pub fn raw_cache_path(endpoint: &str, path: &str) -> Option<PathBuf> {
    crate::cache::CacheDir::Raw.ensure().ok()?;
    let file_path = raw_cache_candidate_path(endpoint, path);
    match std::fs::metadata(&file_path) {
        Ok(meta) if meta.len() > 0 => Some(file_path),
        _ => None,
    }
}

/// 解析 `host` / `host:port` / `[ipv6]:port` / 裸 IPv6 → (host, port)，默认 22。
/// raw/ 缓存命名空间的 authority（P1-D-2）。
///
/// 这是从 `SftpClient::connect` **原样抽取**的历史规则（extraction，**不是**
/// normalization redesign），writer 与此 helper 必须产出逐字相同的字符串，
/// 否则磁盘上已存在的旧缓存会静默 miss：
///   * `port == 22` → `host` 原样；
///   * 其它端口      → `{host}:{port}`。
///
/// 刻意**不**做任何规范化：不小写 host、不去尾点、不改 IPv6 方括号表达、
/// 不做 IDNA、不 normalize localhost、不参与 username。
///
/// 纯函数、零副作用、零网络。
pub fn endpoint_for(host: &str, port: u16) -> String {
    if port == 22 {
        host.to_string()
    } else {
        format!("{host}:{port}")
    }
}

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
    fn sftp_auth_method_is_explicit_and_requires_its_selected_credential() {
        assert_eq!(
            resolve_auth_method("password", "secret", "private-key").unwrap(),
            SftpAuthMethod::Password
        );
        assert_eq!(
            resolve_auth_method("privateKey", "secret", "private-key").unwrap(),
            SftpAuthMethod::PrivateKey
        );
        assert!(resolve_auth_method("password", "", "private-key").is_err());
        assert!(resolve_auth_method("privateKey", "secret", "").is_err());
        assert!(resolve_auth_method("auto", "secret", "private-key").is_err());
    }

    #[test]
    fn live_fixture_pins_host_key_authenticates_with_password_and_reads_random_offsets() {
        let Some(config) = LiveConfig::from_env() else {
            return;
        };
        let probed_fingerprint = SftpClient::probe_host_key(&config.host, config.port)
            .expect("live SFTP host-key probe should not require credentials");
        assert_eq!(probed_fingerprint, config.fingerprint);
        let client = SftpClient::connect(
            &config.host,
            config.port,
            &config.username,
            "password",
            &config.password,
            "",
            "",
            &config.fingerprint,
        )
        .expect("live SFTP password credentials should authenticate");
        assert_live_fixture(&client, &config.file);
        client.disconnect();

        let wrong_password = format!("{}-invalid", config.password);
        let error = SftpClient::connect(
            &config.host,
            config.port,
            &config.username,
            "password",
            &wrong_password,
            "",
            "",
            &config.fingerprint,
        )
        .err()
        .expect("invalid SFTP password must fail");
        assert!(!error.to_string().contains(&wrong_password));
    }

    #[test]
    fn live_fixture_authenticates_with_private_key_and_rejects_changed_host_key() {
        let Some(config) = LiveConfig::from_env() else {
            return;
        };
        let Ok(key_path) = std::env::var("RCH_SFTP_TEST_PRIVATE_KEY_FILE") else {
            return;
        };
        let private_key = std::fs::read_to_string(key_path)
            .expect("live SFTP private-key fixture should be readable");
        let key_passphrase = std::env::var("RCH_SFTP_TEST_KEY_PASSPHRASE").unwrap_or_default();
        let client = SftpClient::connect(
            &config.host,
            config.port,
            &config.username,
            "privateKey",
            "",
            &private_key,
            &key_passphrase,
            &config.fingerprint,
        )
        .expect("live SFTP private-key credentials should authenticate");
        assert_live_fixture(&client, &config.file);
        client.disconnect();

        let auth_log_before = std::fs::read_to_string(&config.auth_log_file)
            .expect("SFTP test server should record authentication attempts");
        let rotated_port = std::env::var("RCH_SFTP_TEST_ROTATED_PORT")
            .ok()
            .and_then(|port| port.parse().ok())
            .unwrap_or(config.port);
        let stale_pin = if rotated_port == config.port {
            "SHA256:deliberately-wrong-pin"
        } else {
            &config.fingerprint
        };
        let changed_pin = SftpClient::connect(
            &config.host,
            rotated_port,
            &config.username,
            "password",
            &config.password,
            "",
            "",
            stale_pin,
        )
        .err()
        .expect("a changed host key must fail before login");
        assert!(!changed_pin.to_string().contains(&config.password));
        assert_eq!(
            std::fs::read_to_string(&config.auth_log_file)
                .expect("SFTP test server should record authentication attempts"),
            auth_log_before,
            "a changed host key must be rejected before the server sees credentials"
        );
    }

    #[test]
    fn live_fixture_accepts_encrypted_private_key_and_rejects_wrong_passphrase() {
        let Some(config) = LiveConfig::from_env() else {
            return;
        };
        let Ok(key_path) = std::env::var("RCH_SFTP_TEST_ENCRYPTED_PRIVATE_KEY_FILE") else {
            return;
        };
        let private_key = std::fs::read_to_string(key_path)
            .expect("encrypted SFTP private-key fixture should be readable");
        let key_passphrase = std::env::var("RCH_SFTP_TEST_KEY_PASSPHRASE")
            .expect("encrypted SFTP test passphrase should be set");
        let client = SftpClient::connect(
            &config.host,
            config.port,
            &config.username,
            "privateKey",
            "",
            &private_key,
            &key_passphrase,
            &config.fingerprint,
        )
        .expect("correct encrypted SFTP private key should authenticate");
        assert_live_fixture(&client, &config.file);
        client.disconnect();

        let wrong_passphrase = format!("{key_passphrase}-invalid");
        let error = SftpClient::connect(
            &config.host,
            config.port,
            &config.username,
            "privateKey",
            "",
            &private_key,
            &wrong_passphrase,
            &config.fingerprint,
        )
        .err()
        .expect("incorrect private-key passphrase must fail");
        assert!(!error.to_string().contains(&wrong_passphrase));
    }

    fn assert_live_fixture(client: &SftpClient, file: &str) {
        const CONTENT: &[u8] = b"RCH-SFTP-READ-FIXTURE-2026";
        assert!(client
            .list(".")
            .unwrap()
            .iter()
            .any(|entry| entry.name == file));
        assert_eq!(client.file_size(file).unwrap(), CONTENT.len() as u64);

        let mut head = [0; 7];
        assert_eq!(client.read_at(file, 0, &mut head).unwrap(), head.len());
        assert_eq!(&head, &CONTENT[..7]);

        let mut middle = [0; 9];
        assert_eq!(client.read_at(file, 6, &mut middle).unwrap(), middle.len());
        assert_eq!(&middle, &CONTENT[6..15]);

        let mut tail = [0; 16];
        let tail_len = CONTENT.len() - 20;
        assert_eq!(client.read_at(file, 20, &mut tail).unwrap(), tail_len);
        assert_eq!(&tail[..tail_len], &CONTENT[20..]);
        assert_eq!(
            client
                .read_at(file, CONTENT.len() as u64, &mut tail)
                .unwrap(),
            0
        );
    }

    struct LiveConfig {
        host: String,
        port: u16,
        username: String,
        password: String,
        fingerprint: String,
        file: String,
        auth_log_file: String,
    }

    impl LiveConfig {
        fn from_env() -> Option<Self> {
            Some(Self {
                host: std::env::var("RCH_SFTP_TEST_HOST").ok()?,
                port: std::env::var("RCH_SFTP_TEST_PORT").ok()?.parse().ok()?,
                username: std::env::var("RCH_SFTP_TEST_USERNAME").ok()?,
                password: std::env::var("RCH_SFTP_TEST_PASSWORD").ok()?,
                fingerprint: std::env::var("RCH_SFTP_TEST_FINGERPRINT").ok()?,
                file: std::env::var("RCH_SFTP_TEST_FILE").ok()?,
                auth_log_file: std::env::var("RCH_SFTP_TEST_AUTH_LOG_FILE").ok()?,
            })
        }
    }

    #[test]
    fn sftp_host_key_pin_requires_an_exact_nonempty_fingerprint() {
        assert!(fingerprint_matches("SHA256:abc123", " SHA256:abc123 "));
        assert!(!fingerprint_matches("", "SHA256:abc123"));
        assert!(!fingerprint_matches("SHA256:abc123", "SHA256:abc124"));
        assert!(!fingerprint_matches("sha256:abc123", "SHA256:abc123"));
    }

    #[test]
    fn join_remote_path_works() {
        assert_eq!(join_remote_path("/", "漫画.cbz"), "/漫画.cbz");
        assert_eq!(join_remote_path("/comic", "a.cbz"), "/comic/a.cbz");
        assert_eq!(join_remote_path("/comic/", "a.cbz"), "/comic/a.cbz");
        assert_eq!(join_remote_path("", "a.cbz"), "a.cbz");
    }

    #[test]
    fn effective_list_path_only_normalizes_empty_roots() {
        assert_eq!(effective_list_path(""), "/");
        assert_eq!(effective_list_path("   "), "/");
        assert_eq!(effective_list_path("/"), "/");
        assert_eq!(effective_list_path("/日漫"), "/日漫");
        assert_eq!(effective_list_path("."), ".", "相对路径语义必须保留");
    }

    #[test]
    fn readdir_child_path_matches_readdir_rules() {
        assert_eq!(readdir_child_path("", "a.cbz"), "a.cbz");
        assert_eq!(readdir_child_path("/", "a.cbz"), "/a.cbz");
        assert_eq!(readdir_child_path("/日漫", "a.cbz"), "/日漫/a.cbz");
        assert_eq!(readdir_child_path("/日漫/", "a.cbz"), "/日漫/a.cbz");
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

    // ------------------------------------------------------------------
    // P1-D-2 cache authority：SFTP endpoint extraction 的兼容性冻结
    // ------------------------------------------------------------------

    /// SFTP-CA-1/2/3：`endpoint_for` 必须**逐字**复现当前 `connect()` 的历史规则。
    ///
    /// 这些字面量就是抽取前 `SftpClient::connect` 会产出的字符串：
    /// `if port == 22 { host.clone() } else { format!("{host}:{port}") }`。
    /// **这是 extraction，不是 normalization redesign**：不做任何规范化。
    #[test]
    fn sftp_ca_endpoint_freezes_historical_rules_without_normalization() {
        // SFTP-CA-1 默认端口：host 原样，不做任何规范化。
        assert_eq!(endpoint_for("nas.local", 22), "nas.local");
        assert_eq!(endpoint_for("NAS.LOCAL", 22), "NAS.LOCAL", "不得小写 host");
        assert_eq!(endpoint_for("nas.local.", 22), "nas.local.", "不得去尾点");
        assert_eq!(endpoint_for("[::1]", 22), "[::1]", "不得改 IPv6 方括号表达");
        assert_eq!(endpoint_for("", 22), "", "空 host 也必须原样保留");
        assert_eq!(
            endpoint_for(" nas ", 22),
            " nas ",
            "不得 trim（Dart 侧已 trim）"
        );

        // SFTP-CA-2 非默认端口：`{host}:{port}`。
        assert_eq!(endpoint_for("nas.local", 2222), "nas.local:2222");
        assert_eq!(endpoint_for("[::1]", 2222), "[::1]:2222");
        assert_eq!(endpoint_for("NAS.LOCAL", 2222), "NAS.LOCAL:2222");

        // SFTP-CA-3 当前 Dart `_parseHostPort` 实际可能传进来的形式。
        // `[::1]:2222` → Dart 的 lastIndexOf(':') 切分 → host 保留方括号。
        assert_eq!(endpoint_for("[::1]", 2222), "[::1]:2222");
        // url 为空、端口取自 source.port 时 Dart 传空 host。
        assert_eq!(endpoint_for("", 2222), ":2222");
    }

    /// SFTP-CA-4：writer authority 与 helper 产出的 authority 让 **existing key helper**
    /// 得到**完全相同**的 raw cache identity（因此磁盘上的旧缓存依旧可寻址）。
    ///
    /// 注意：这里**没有手拼 cache path**，两侧都调用 production 的
    /// `cache_hash` / `raw_cache_path`。
    #[test]
    fn sftp_ca_raw_cache_identity_is_unchanged_by_the_extraction() {
        let path = "/books/demo.cbz";
        // (host, port, 抽取前 connect() 会产出的历史 endpoint 字面量)
        let cases = [
            ("nas.local", 22u16, "nas.local"),
            ("nas.local", 2222, "nas.local:2222"),
            ("[::1]", 22, "[::1]"),
            ("[::1]", 2222, "[::1]:2222"),
        ];
        for (host, port, historical) in cases {
            let via_helper = endpoint_for(host, port);
            assert_eq!(
                via_helper, historical,
                "helper 必须与历史 endpoint 逐字一致（host={host} port={port}）"
            );
            assert_eq!(
                cache_hash(&via_helper, path),
                cache_hash(historical, path),
                "raw cache identity 必须完全相同（host={host} port={port}）"
            );
            assert_eq!(
                raw_cache_path(&via_helper, path),
                raw_cache_path(historical, path),
                "raw cache path 必须完全相同（host={host} port={port}）"
            );
        }
    }

    /// 防回归：`parse_endpoint` 是**死代码**（生产零调用者），且它的规范化会剥离
    /// IPv6 方括号 —— 一旦有人把它接成 authority，就会产出不同的 endpoint，
    /// 导致磁盘上的旧缓存**静默 miss**。此测试把这条边界钉住。
    #[test]
    fn sftp_ca_parse_endpoint_must_not_become_the_cache_authority() {
        let (host, port) = parse_endpoint("[::1]:2222");
        assert_eq!((host.as_str(), port), ("::1", 2222));
        assert_eq!(endpoint_for(&host, port), "::1:2222");
        assert_ne!(
            endpoint_for(&host, port),
            endpoint_for("[::1]", 2222),
            "两条派生路径对 IPv6 输入并不等价；authority 只能是 Dart parser + endpoint_for"
        );
    }
}
