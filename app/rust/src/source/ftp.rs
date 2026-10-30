//! FTP 书源：纯 FTP（RFC 959）只读访问，随机读走 REST + RETR。
//!
//! 与 SMB/SFTP 对称：会话持有独立控制连接，数据连接按需建立；
//! 上层只看到 [`ByteSource`] 的 `read_at`。仅支持明文 FTP（OpenList 默认）。

use super::{webdav::DownloadProgress, ByteSource, Entry};
use anyhow::{anyhow, Context, Result};
use std::io::Read;
use std::net::ToSocketAddrs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use suppaftp::types::FileType;
use suppaftp::FtpStream;

/// FTP 客户端：控制连接串行复用（FTP 协议不支持并发命令）。
pub struct FtpClient {
    ftp: Mutex<FtpStream>,
    endpoint: String,
    root: String,
}

impl FtpClient {
    /// 连接 + 登录 + 校验根目录可列。阻塞调用，应在 spawn_blocking 中执行。
    pub fn connect(host: &str, port: u16, user: &str, pass: &str, root: &str) -> Result<Self> {
        if user.trim().is_empty() {
            return Err(anyhow!("FTP username must not be empty"));
        }
        let address = format!("{host}:{port}");
        let socket = (host, port)
            .to_socket_addrs()
            .map_err(|e| anyhow!("FTP 解析服务器地址失败: {e}"))?
            .next()
            .ok_or_else(|| anyhow!("FTP 服务器地址无法解析"))?;
        let mut ftp = FtpStream::connect_timeout(socket, Duration::from_secs(15))
            .map_err(|e| anyhow!("FTP 连接失败: {e}"))?;
        ftp.get_ref()
            .set_read_timeout(Some(Duration::from_secs(30)))
            .ok();
        ftp.login(user, pass)
            .map_err(|e| anyhow!("FTP 登录失败: {e}"))?;
        ftp.transfer_type(FileType::Binary)
            .map_err(|e| anyhow!("FTP 切换二进制模式失败: {e}"))?;
        let root = effective_root(root);
        // 校验根目录可列，避免保存后才在浏览时暴露权限问题。
        let probe = if root == "/" {
            "/".to_string()
        } else {
            root.clone()
        };
        if let Err(e) = ftp.mlsd(Some(&probe)) {
            return Err(anyhow!("FTP 列目录失败: {e}"));
        }
        Ok(FtpClient {
            ftp: Mutex::new(ftp),
            endpoint: format!("ftp://{address}"),
            root,
        })
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub fn root(&self) -> &str {
        &self.root
    }

    pub fn raw_cache_path(&self, path: &str) -> Option<PathBuf> {
        super::sftp::raw_cache_path(&self.endpoint, path)
    }

    pub fn raw_cache_candidate_path(&self, path: &str) -> PathBuf {
        super::sftp::raw_cache_candidate_path(&self.endpoint, path)
    }

    /// 列出目录（目录在前，自然排序）。使用 MLSD（OpenList 与主流服务器均支持）。
    pub fn list(&self, path: &str) -> Result<Vec<Entry>> {
        let parent = effective_path(path);
        let mut ftp = self
            .ftp
            .lock()
            .map_err(|_| anyhow!("FTP 控制连接已失效"))?;
        let lines = ftp
            .mlsd(Some(&parent))
            .map_err(|e| anyhow!("FTP 列目录失败: {e}"))?;
        let mut out: Vec<Entry> = lines
            .iter()
            .filter_map(|line| parse_mlsd_line(line, &parent))
            .collect();
        out.sort_by(|a, b| {
            b.is_dir
                .cmp(&a.is_dir)
                .then_with(|| crate::util::natural_cmp(&a.name, &b.name))
        });
        Ok(out)
    }

    pub fn file_size(&self, path: &str) -> Result<u64> {
        let mut ftp = self
            .ftp
            .lock()
            .map_err(|_| anyhow!("FTP 控制连接已失效"))?;
        let size = ftp
            .size(remote_path(path))
            .map_err(|e| anyhow!("FTP 获取文件大小失败: {e}"))?;
        Ok(size as u64)
    }

    /// 随机读：REST offset + RETR，读满 buf 后主动关闭数据连接。
    /// 早期关闭会让服务器在控制通道回报 550（OpenList 实测），忽略即可。
    pub fn read_at(&self, path: &str, offset: u64, buf: &mut [u8]) -> std::io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let mut ftp = self
            .ftp
            .lock()
            .map_err(|_| std::io::Error::other("FTP 控制连接已失效"))?;
        ftp.resume_transfer(offset as usize)
            .map_err(|e| std::io::Error::other(format!("FTP REST 失败: {e}")))?;
        let mut stream = ftp
            .retr_as_stream(remote_path(path))
            .map_err(|e| std::io::Error::other(format!("FTP RETR 失败: {e}")))?;
        let read = stream.read(buf)?;
        // 提前结束传输会让 OpenList 在控制通道回报 550；finalize 会消费该响应，
        // 让控制连接保持同步、可继续下一次 REST/RETR。
        let _ = ftp.finalize_retr_stream(stream);
        Ok(read)
    }

    /// 整本下载到 raw/ 缓存（与 WebDAV/SFTP 相同命名规则）。
    pub fn download_to_raw_cache(
        &self,
        path: &str,
        progress: Option<Arc<DownloadProgress>>,
    ) -> Result<PathBuf> {
        let file_path = self.raw_cache_candidate_path(path);
        let raw_dir = crate::cache::CacheDir::Raw
            .ensure()
            .context("创建 raw/ 缓存目录失败")?;
        if let Some(parent) = file_path.parent() {
            std::fs::create_dir_all(parent)?;
        } else {
            std::fs::create_dir_all(&raw_dir)?;
        }
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
        let mut ftp = self
            .ftp
            .lock()
            .map_err(|_| anyhow!("FTP 控制连接已失效"))?;
        let mut stream = ftp
            .retr_as_stream(remote_path(path))
            .map_err(|e| anyhow!("FTP RETR 失败: {e}"))?;
        let mut writer = crate::cache::AtomicCacheFile::create(&file_path)?;
        let mut buf = vec![0u8; 64 * 1024];
        let mut written: u64 = 0;
        loop {
            let n = stream
                .read(&mut buf)
                .map_err(|e| anyhow!("FTP 下载读取失败: {e}"))?;
            if n == 0 {
                break;
            }
            writer.write_all(&buf[..n])?;
            written += n as u64;
            if let Some(p) = &progress {
                p.downloaded
                    .store(written, std::sync::atomic::Ordering::SeqCst);
            }
        }
        writer.commit()?;
        Ok(file_path)
    }

    pub fn disconnect(&self) {
        if let Ok(mut ftp) = self.ftp.lock() {
            let _ = ftp.quit();
        }
    }
}

/// FTP 远程文件作为 [`ByteSource`]。
pub struct FtpFile {
    client: Arc<FtpClient>,
    path: String,
    len: u64,
}

impl FtpFile {
    pub fn new(client: Arc<FtpClient>, path: String, len: u64) -> Self {
        FtpFile { client, path, len }
    }
}

impl ByteSource for FtpFile {
    fn len(&self) -> u64 {
        self.len
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> std::io::Result<usize> {
        if offset >= self.len {
            return Ok(0);
        }
        let limit = usize::try_from(self.len - offset).unwrap_or(usize::MAX);
        let take = buf.len().min(limit);
        let slice = &mut buf[..take];
        self.client.read_at(&self.path, offset, slice)
    }
}

fn effective_root(root: &str) -> String {
    let root = root.trim();
    if root.is_empty() || root == "." {
        "/".to_string()
    } else if root.starts_with('/') {
        let trimmed = root.trim_end_matches('/');
        if trimmed.is_empty() {
            "/".to_string()
        } else {
            trimmed.to_string()
        }
    } else {
        format!("/{}", root.trim_end_matches('/'))
    }
}

fn effective_path(path: &str) -> String {
    let path = path.trim();
    if path.is_empty() || path == "." {
        "/".to_string()
    } else if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    }
}

fn remote_path(path: &str) -> String {
    effective_path(path)
}

/// 组合父目录与子项名（与 SFTP/WebDAV 语义一致）。
fn join_child_path(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_string()
    } else if parent.ends_with('/') {
        format!("{parent}{name}")
    } else {
        format!("{parent}/{name}")
    }
}

/// 解析一行 MLSD：`type=file;size=123;modify=20260607123532; name`。
fn parse_mlsd_line(line: &str, parent: &str) -> Option<Entry> {
    let (facts, name) = line.split_once(' ')?;
    if name.is_empty() || name == "." || name == ".." {
        return None;
    }
    let mut is_dir = false;
    let mut size = 0u64;
    for fact in facts.split(';') {
        let Some((key, value)) = fact.split_once('=') else {
            continue;
        };
        match key.to_ascii_lowercase().as_str() {
            "type" => {
                let value = value.to_ascii_lowercase();
                is_dir = value == "dir" || value == "cdir" || value == "pdir";
            }
            "size" => size = value.parse().unwrap_or(0),
            _ => {}
        }
    }
    Some(Entry {
        name: name.to_string(),
        path: join_child_path(parent, name),
        is_dir,
        size,
        mtime: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_mlsd_file_and_directory() {
        let file = parse_mlsd_line("type=file;size=1234;modify=20260607123532; a.cbz", "/日漫")
            .expect("file");
        assert_eq!(file.name, "a.cbz");
        assert_eq!(file.path, "/日漫/a.cbz");
        assert!(!file.is_dir);
        assert_eq!(file.size, 1234);

        let dir = parse_mlsd_line("type=dir;modify=20260607123532; 子目录", "/").expect("dir");
        assert!(dir.is_dir);
        assert_eq!(dir.path, "/子目录");
    }

    #[test]
    fn normalizes_roots_and_paths() {
        assert_eq!(effective_root(""), "/");
        assert_eq!(effective_root("/日漫/"), "/日漫");
        assert_eq!(effective_root("日漫"), "/日漫");
        assert_eq!(effective_path(""), "/");
        assert_eq!(effective_path("."), "/");
        assert_eq!(effective_path("/日漫"), "/日漫");
    }
}
