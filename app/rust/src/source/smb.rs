use super::{webdav::DownloadProgress, ByteSource, Entry};
use anyhow::{anyhow, Context, Result};
use std::io;
use std::net::Ipv6Addr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

struct SmbSession {
    client: smb2::SmbClient,
    tree: smb2::Tree,
}

/// One authenticated, read-only SMB2/3 share session.
///
/// Network calls use a private Tokio runtime because the document readers
/// expose synchronous positioned reads through [`ByteSource`].
pub struct SmbClient {
    runtime: Arc<tokio::runtime::Runtime>,
    session: Arc<tokio::sync::Mutex<SmbSession>>,
    share: String,
    root_path: String,
    endpoint: String,
}

impl SmbClient {
    /// Connect to one share. Empty username and password request Guest access.
    pub fn connect(
        host: &str,
        port: u16,
        share: &str,
        root_path: &str,
        username: &str,
        password: &str,
    ) -> Result<Self> {
        let addr = server_address(host, port)?;
        let share = validate_share_name(share)?;
        let root_path = provider_root_path(root_path, &share)?;
        let (domain, username) = split_domain_username(username)?;
        if username.is_empty() && !password.is_empty() {
            return Err(anyhow!(
                "SMB username is required when a password is supplied"
            ));
        }

        let runtime = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .context("could not initialize the SMB network runtime")?,
        );
        let addr_for_connect = addr.clone();
        let share_for_connect = share.clone();
        let username = username.to_owned();
        let password = password.to_owned();
        let domain = domain.to_owned();
        let session = runtime.block_on(async move {
            let config = smb2::ClientConfig {
                addr: addr_for_connect,
                username,
                password,
                domain,
                auto_reconnect: true,
                dfs_enabled: false,
                timeout: Duration::from_secs(10),
                response_timeout: Some(Duration::from_secs(20)),
                ..Default::default()
            };
            let mut client = smb2::SmbClient::connect(config)
                .await
                .map_err(|error| sanitized_smb_error("connect", error))?;
            let tree = client
                .connect_share(&share_for_connect)
                .await
                .map_err(|error| sanitized_smb_error("open share", error))?;
            Ok::<_, anyhow::Error>(SmbSession { client, tree })
        })?;

        let endpoint = endpoint_for(host, port, &share)?;
        Ok(Self {
            runtime,
            session: Arc::new(tokio::sync::Mutex::new(session)),
            share,
            root_path,
            endpoint,
        })
    }

    /// Stable server/share identity for remote cache namespaces.
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// The share mounted by this session.
    pub fn share(&self) -> &str {
        &self.share
    }

    /// The configured source root, relative to the SMB share.
    pub fn configured_root_path(&self, requested_root: &str) -> Result<String> {
        if requested_root.trim().is_empty() || requested_root == "/" {
            return Ok(self.root_path.clone());
        }
        let (provider_path, _) = resolve_share_path(requested_root, &self.share, &self.root_path)?;
        Ok(provider_path)
    }

    /// Probe the stable raw-cache location for a share-relative file.
    pub fn raw_cache_path(&self, path: &str) -> Option<PathBuf> {
        let path = normalize_file_path(path).ok()?;
        raw_cache_path(&self.endpoint, &path)
    }

    /// Build the stable raw-cache candidate without probing the filesystem.
    pub fn raw_cache_candidate_path(&self, path: &str) -> Result<PathBuf> {
        let path = normalize_file_path(path)?;
        Ok(raw_cache_candidate_path(&self.endpoint, &path))
    }

    /// Download the complete remote file into an atomic raw-cache entry.
    pub fn download_to_raw_cache(
        &self,
        path: &str,
        progress: Option<Arc<DownloadProgress>>,
    ) -> Result<PathBuf> {
        let file_path = self.raw_cache_candidate_path(path)?;
        let raw_dir = crate::cache::CacheDir::Raw
            .ensure()
            .context("could not create the SMB raw cache directory")?;
        if let Some(parent) = file_path.parent() {
            std::fs::create_dir_all(parent)?;
        } else {
            std::fs::create_dir_all(&raw_dir)?;
        }
        if let Ok(metadata) = std::fs::metadata(&file_path) {
            if metadata.len() > 0 {
                if let Some(progress) = progress {
                    progress
                        .total
                        .store(metadata.len(), std::sync::atomic::Ordering::SeqCst);
                    progress
                        .downloaded
                        .store(metadata.len(), std::sync::atomic::Ordering::SeqCst);
                }
                return Ok(file_path);
            }
        }

        let total = self.file_size(path)?;
        if let Some(progress) = &progress {
            progress
                .total
                .store(total, std::sync::atomic::Ordering::SeqCst);
            progress
                .downloaded
                .store(0, std::sync::atomic::Ordering::SeqCst);
        }
        let source = self.open_file(path)?;
        let mut writer = crate::cache::AtomicCacheFile::create(&file_path)?;
        let mut buffer = vec![0; 64 * 1024];
        let mut downloaded = 0_u64;
        while downloaded < total {
            let requested = usize::try_from((total - downloaded).min(buffer.len() as u64))
                .unwrap_or(buffer.len());
            let count = source.read_at(downloaded, &mut buffer[..requested])?;
            if count == 0 {
                return Err(anyhow!("SMB file ended before its advertised size"));
            }
            writer.write_all(&buffer[..count])?;
            downloaded += count as u64;
            if let Some(progress) = &progress {
                progress
                    .downloaded
                    .store(downloaded, std::sync::atomic::Ordering::SeqCst);
            }
        }
        writer.commit()?;
        Ok(file_path)
    }

    /// List a share-relative directory. The empty path lists the share root.
    pub fn list(&self, path: &str) -> Result<Vec<Entry>> {
        let (provider_path, logical_path) = resolve_share_path(path, &self.share, &self.root_path)?;
        // smb2's list_directory treats an empty path as the share root. A dot
        // is sent as a literal path component and fails on Windows shares.
        let query_path = provider_path;
        let session = Arc::clone(&self.session);
        let entries = self.runtime.block_on(async move {
            let mut session = session.lock().await;
            let SmbSession { client, tree } = &mut *session;
            client
                .list_directory(tree, &query_path)
                .await
                .map_err(|error| sanitized_smb_error("list directory", error))
        })?;

        let mut entries: Vec<Entry> = entries
            .into_iter()
            .filter(|entry| entry.name != "." && entry.name != "..")
            .map(|entry| Entry {
                path: join_share_path(&logical_path, &entry.name),
                name: entry.name,
                is_dir: entry.is_directory,
                size: entry.size,
                mtime: entry
                    .modified
                    .to_system_time()
                    .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|duration| duration.as_secs() as i64)
                    .unwrap_or(0),
            })
            .collect();
        entries.sort_by(|left, right| {
            right
                .is_dir
                .cmp(&left.is_dir)
                .then_with(|| crate::util::natural_cmp(&left.name, &right.name))
        });
        Ok(entries)
    }

    /// Return a file's size using the server's metadata response.
    pub fn file_size(&self, path: &str) -> Result<u64> {
        let (path, _) = resolve_share_path(path, &self.share, &self.root_path)?;
        let path = normalize_file_path(&path)?;
        let session = Arc::clone(&self.session);
        self.runtime.block_on(async move {
            let mut session = session.lock().await;
            let SmbSession { client, tree } = &mut *session;
            client
                .stat(tree, &path)
                .await
                .map(|info| info.size)
                .map_err(|error| sanitized_smb_error("get file size", error))
        })
    }

    /// Open a share-relative file for concurrent positioned reads.
    pub fn open_file(&self, path: &str) -> Result<SmbFile> {
        let (path, _) = resolve_share_path(path, &self.share, &self.root_path)?;
        let path = normalize_file_path(&path)?;
        let session = Arc::clone(&self.session);
        let reader = self.runtime.block_on(async move {
            let mut session = session.lock().await;
            let SmbSession { client, tree } = &mut *session;
            client
                .open_file_reader(tree, &path)
                .await
                .map_err(|error| sanitized_smb_error("open file", error))
        })?;
        let len = reader.size();
        Ok(SmbFile::new(
            Arc::new(Smb2PositionedReader {
                runtime: Arc::clone(&self.runtime),
                reader,
            }),
            len,
        ))
    }

    /// Close the share session and release its connection.
    pub fn disconnect(&self) -> Result<()> {
        let session = Arc::clone(&self.session);
        self.runtime.block_on(async move {
            let mut session = session.lock().await;
            let SmbSession { client, tree } = &mut *session;
            client
                .disconnect_share(tree)
                .await
                .map_err(|error| sanitized_smb_error("close share", error))
        })
    }

    /// Read a share-relative file at an arbitrary offset.
    pub fn read_at(&self, path: &str, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        self.open_file(path)
            .map_err(|error| io::Error::other(error.to_string()))?
            .read_at(offset, buf)
    }
}

trait PositionedReader: Send + Sync {
    fn read_at(&self, offset: u64, len: usize) -> io::Result<Vec<u8>>;
}

struct Smb2PositionedReader {
    runtime: Arc<tokio::runtime::Runtime>,
    reader: smb2::FileReader,
}

impl PositionedReader for Smb2PositionedReader {
    fn read_at(&self, offset: u64, len: usize) -> io::Result<Vec<u8>> {
        if len == 0 {
            return Ok(Vec::new());
        }
        let len =
            u64::try_from(len).map_err(|_| io::Error::other("SMB read length is out of range"))?;
        self.runtime
            .block_on(self.reader.read_at(offset, len))
            .map_err(|error| io::Error::other(sanitized_smb_message(error.kind())))
    }
}

/// Read-only SMB file implementing the random-access reader contract.
pub struct SmbFile {
    reader: Arc<dyn PositionedReader>,
    len: u64,
}

impl SmbFile {
    fn new(reader: Arc<dyn PositionedReader>, len: u64) -> Self {
        Self { reader, len }
    }
}

impl ByteSource for SmbFile {
    fn len(&self) -> u64 {
        self.len
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() || offset >= self.len {
            return Ok(0);
        }
        let requested = buf
            .len()
            .min(usize::try_from(self.len - offset).unwrap_or(usize::MAX));
        let bytes = self.reader.read_at(offset, requested)?;
        let count = bytes.len().min(requested);
        buf[..count].copy_from_slice(&bytes[..count]);
        Ok(count)
    }
}

fn normalize_share_path(path: &str) -> Result<String> {
    let mut segments = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => return Err(anyhow!("SMB path cannot leave its configured share")),
            segment => segments.push(segment),
        }
    }
    Ok(segments.join("/"))
}

fn normalize_file_path(path: &str) -> Result<String> {
    let path = normalize_share_path(path)?;
    if path.is_empty() {
        return Err(anyhow!("SMB file path is empty"));
    }
    Ok(path)
}

fn provider_root_path(path: &str, _share: &str) -> Result<String> {
    normalize_share_path(path)
}

/// Map canonical `share/path` coordinates into a provider path while retaining
/// the canonical logical path used by source records and the offline catalog.
fn resolve_share_path(path: &str, share: &str, default_root: &str) -> Result<(String, String)> {
    if path.trim().is_empty() || path == "/" {
        let logical = if default_root.is_empty() {
            share.to_owned()
        } else {
            format!("{share}/{default_root}")
        };
        return Ok((default_root.to_owned(), logical));
    }
    let logical = normalize_share_path(path)?;
    let mut segments = logical.split('/');
    let first = segments.next().unwrap_or_default();
    let provider = if first.eq_ignore_ascii_case(share) {
        segments.collect::<Vec<_>>().join("/")
    } else {
        logical.clone()
    };
    Ok((provider, logical))
}

fn join_share_path(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_owned()
    } else {
        format!("{parent}/{name}")
    }
}

fn server_address(host: &str, port: u16) -> Result<String> {
    let host = host.trim();
    if host.is_empty() || host.contains('/') || host.contains('\\') || port == 0 {
        return Err(anyhow!("SMB server address or port is invalid"));
    }
    let unbracketed = host
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .unwrap_or(host);
    if unbracketed.is_empty() || unbracketed.chars().any(|c| c == '[' || c == ']') {
        return Err(anyhow!("SMB server address is invalid"));
    }
    let formatted_host = if unbracketed.parse::<Ipv6Addr>().is_ok() || unbracketed.contains(':') {
        format!("[{unbracketed}]")
    } else {
        unbracketed.to_owned()
    };
    Ok(format!("{formatted_host}:{port}"))
}

/// Stable SMB server/share authority used by remote raw-cache identities.
pub fn endpoint_for(host: &str, port: u16, share: &str) -> Result<String> {
    Ok(format!(
        "smb://{}/{share}",
        server_address(host, port)?,
        share = validate_share_name(share)?
    ))
}

/// Probe a deterministic SMB raw-cache path.
pub fn raw_cache_path(endpoint: &str, path: &str) -> Option<PathBuf> {
    crate::source::sftp::raw_cache_path(endpoint, path)
}

/// Produce the deterministic SMB raw-cache candidate path.
pub fn raw_cache_candidate_path(endpoint: &str, path: &str) -> PathBuf {
    crate::source::sftp::raw_cache_candidate_path(endpoint, path)
}

fn validate_share_name(share: &str) -> Result<String> {
    if share.trim().is_empty()
        || matches!(share, "." | "..")
        || share.contains('/')
        || share.contains('\\')
    {
        return Err(anyhow!("SMB share name is invalid"));
    }
    Ok(share.to_owned())
}

fn split_domain_username(username: &str) -> Result<(&str, &str)> {
    if let Some((domain, user)) = username.split_once('\\') {
        if domain.is_empty() || user.is_empty() {
            return Err(anyhow!("SMB domain and username must both be provided"));
        }
        Ok((domain, user))
    } else {
        Ok(("", username))
    }
}

fn sanitized_smb_message(kind: smb2::ErrorKind) -> &'static str {
    match kind {
        smb2::ErrorKind::AuthRequired
        | smb2::ErrorKind::GuestSessionRefused
        | smb2::ErrorKind::SigningRequired => {
            "authentication was rejected; check the username and password"
        }
        smb2::ErrorKind::AccessDenied => "access to the SMB share or path was denied",
        smb2::ErrorKind::NotFound => "the SMB share or path was not found",
        smb2::ErrorKind::ConnectionLost | smb2::ErrorKind::TimedOut => {
            "the SMB server is unreachable or timed out"
        }
        _ => "the SMB operation failed",
    }
}

fn sanitized_smb_error(operation: &str, error: smb2::Error) -> anyhow::Error {
    anyhow!(
        "SMB {operation} failed: {}",
        sanitized_smb_message(error.kind())
    )
}

#[cfg(test)]
mod tests {
    use super::{
        normalize_share_path, provider_root_path, resolve_share_path, PositionedReader, SmbClient,
        SmbFile,
    };
    use crate::source::ByteSource;
    use std::io;
    use std::sync::Arc;

    struct MemoryReader(Vec<u8>);

    impl PositionedReader for MemoryReader {
        fn read_at(&self, offset: u64, len: usize) -> io::Result<Vec<u8>> {
            let start = usize::try_from(offset).unwrap_or(usize::MAX);
            if start >= self.0.len() {
                return Ok(Vec::new());
            }
            let end = start.saturating_add(len).min(self.0.len());
            Ok(self.0[start..end].to_vec())
        }
    }

    #[test]
    fn share_paths_are_normalized_and_cannot_escape_the_share() {
        assert_eq!(normalize_share_path("").unwrap(), "");
        assert_eq!(
            normalize_share_path("//manga///One Piece/Vol 1").unwrap(),
            "manga/One Piece/Vol 1"
        );
        assert!(normalize_share_path("../outside").is_err());
        assert!(normalize_share_path("manga/../../outside").is_err());
        assert_eq!(
            resolve_share_path("comics/One Piece", "comics", "old").unwrap(),
            ("One Piece".to_string(), "comics/One Piece".to_string())
        );
        assert_eq!(
            resolve_share_path("", "comics", "One Piece").unwrap(),
            ("One Piece".to_string(), "comics/One Piece".to_string())
        );
        assert_eq!(
            provider_root_path("comics/child", "comics").unwrap(),
            "comics/child",
            "a configured root is already relative to the mounted share"
        );
    }

    #[test]
    fn positioned_reads_return_short_reads_and_eof_without_overreading() {
        let file = SmbFile::new(Arc::new(MemoryReader(b"abcdef".to_vec())), 6);
        let mut buf = [0_u8; 8];

        let count = file.read_at(2, &mut buf).unwrap();
        assert_eq!(count, 4);
        assert_eq!(&buf[..count], b"cdef");
        assert_eq!(file.read_at(6, &mut buf).unwrap(), 0);
        assert_eq!(file.read_at(99, &mut buf).unwrap(), 0);
        assert_eq!(file.read_at(0, &mut []).unwrap(), 0);
    }

    #[test]
    fn exact_read_reports_unexpected_eof_for_short_remote_files() {
        let file = SmbFile::new(Arc::new(MemoryReader(b"abc".to_vec())), 3);
        let mut buf = [0_u8; 4];

        assert_eq!(
            file.read_exact_at(0, &mut buf).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
    }

    #[test]
    fn supported_protocol_dialects_do_not_include_smb1() {
        assert!(smb2::types::Dialect::ALL
            .iter()
            .all(|dialect| *dialect as u16 >= 0x0202));
    }

    #[test]
    fn live_fixture_authenticates_lists_and_reads_at_arbitrary_offsets() {
        let Some(config) = LiveConfig::from_env() else {
            return;
        };
        const CONTENT: &[u8] = b"RCH-SMB-READ-FIXTURE-2026";

        let client = SmbClient::connect(
            &config.host,
            config.port,
            &config.share,
            "",
            &config.username,
            &config.password,
        )
        .expect("live SMB credentials should authenticate");
        assert!(client
            .list("")
            .unwrap()
            .iter()
            .any(|entry| entry.name == config.file));
        assert_eq!(
            client.file_size(&config.file).unwrap(),
            CONTENT.len() as u64
        );
        {
            let file = client.open_file(&config.file).unwrap();
            let mut head = [0; 7];
            assert_eq!(file.read_at(0, &mut head).unwrap(), head.len());
            assert_eq!(&head, &CONTENT[..7]);

            let mut middle = [0; 9];
            assert_eq!(file.read_at(6, &mut middle).unwrap(), middle.len());
            assert_eq!(&middle, &CONTENT[6..15]);

            let mut tail = [0; 16];
            let tail_len = CONTENT.len() - 20;
            assert_eq!(file.read_at(20, &mut tail).unwrap(), tail_len);
            assert_eq!(&tail[..tail_len], &CONTENT[20..]);
            assert_eq!(file.read_at(CONTENT.len() as u64, &mut tail).unwrap(), 0);
            assert_eq!(file.read_at(u64::MAX, &mut tail).unwrap(), 0);
        }
        client.disconnect().unwrap();
    }

    #[test]
    fn live_fixture_rejects_an_invalid_password_without_echoing_it() {
        let Some(config) = LiveConfig::from_env() else {
            return;
        };
        let wrong_password = format!("{}-invalid", config.password);
        let result = SmbClient::connect(
            &config.host,
            config.port,
            &config.share,
            "",
            &config.username,
            &wrong_password,
        );
        assert!(result.is_err());
        let error = result
            .err()
            .expect("wrong password should fail")
            .to_string();

        assert!(!error.contains(&wrong_password));
    }

    struct LiveConfig {
        host: String,
        port: u16,
        share: String,
        username: String,
        password: String,
        file: String,
    }

    impl LiveConfig {
        fn from_env() -> Option<Self> {
            Some(Self {
                host: std::env::var("RCH_SMB_TEST_HOST").ok()?,
                port: std::env::var("RCH_SMB_TEST_PORT").ok()?.parse().ok()?,
                share: std::env::var("RCH_SMB_TEST_SHARE").ok()?,
                username: std::env::var("RCH_SMB_TEST_USERNAME").ok()?,
                password: std::env::var("RCH_SMB_TEST_PASSWORD").ok()?,
                file: std::env::var("RCH_SMB_TEST_FILE").ok()?,
            })
        }
    }
}
