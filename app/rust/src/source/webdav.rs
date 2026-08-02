//! WebDAV 书源:PROPFIND 列目录 + HTTP Range 流式读取。
//!
//! 用 reqwest(blocking)手写,精确控制 Range 行为,不引入现成 WebDAV 客户端库。
//! 远程 ZIP/CBZ 复用与本地相同的流式路径:打开只读文件尾部中心目录,
//! 读某页只发该页所需的 Range 请求——远程大文件也能即点即读。

use super::{ByteSource, Entry};
use anyhow::{anyhow, bail, Context, Result};
use percent_encoding::{percent_decode_str, utf8_percent_encode, AsciiSet, CONTROLS};
use reqwest::blocking::Client;
use reqwest::header::RANGE;
use reqwest::{Method, StatusCode, Url};
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// 路径段需要 percent-encode 的字符。
const PATH_SEGMENT: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'`')
    .add(b'{')
    .add(b'}');

/// 对路径逐段 percent-encode,保留 `/` 分隔。
fn encode_path(path: &str) -> String {
    path.split('/')
        .map(|seg| utf8_percent_encode(seg, PATH_SEGMENT).to_string())
        .collect::<Vec<_>>()
        .join("/")
}

/// percent-decode(处理服务器返回的编码 href)。
fn decode_str(s: &str) -> String {
    percent_decode_str(s)
        .decode_utf8()
        .map(|c| c.into_owned())
        .unwrap_or_else(|_| s.to_string())
}

/// 把 href(可能是完整 URL 或服务器绝对路径)统一为服务器绝对路径。
fn href_to_path(href: &str) -> String {
    // 先按完整 URL 解析取路径部分,否则按路径处理;最后统一 percent-decode。
    let path_part = if let Ok(u) = Url::parse(href) {
        u.path().to_string()
    } else {
        href.to_string()
    };
    decode_str(&path_part)
}

/// WebDAV 客户端(blocking,配合 spawn_blocking 使用)。
pub struct WebDavClient {
    client: Client,
    /// scheme://host[:port],不含路径(路径由每次调用的绝对路径给出)。
    origin: String,
    user: String,
    pass: String,
    /// 服务器能力报告(连接时自动探测)。
    pub capability: ServerCapability,
}

/// 服务器能力探测结果。
#[derive(Debug, Clone)]
pub struct ServerCapability {
    pub range_supported: bool,
    /// 平均 RTT(毫秒),基于几次 HEAD 请求。
    pub avg_rtt_ms: f64,
    /// 建议的最大并发请求数。
    pub max_concurrency: u32,
}

impl Default for ServerCapability {
    fn default() -> Self {
        Self {
            range_supported: true,
            avg_rtt_ms: 0.0,
            max_concurrency: 2,
        }
    }
}

/// WebDAV 下载进度追踪(用于 Flutter 端展示进度条)。
pub struct DownloadProgress {
    pub total: AtomicU64,
    pub downloaded: AtomicU64,
}

impl DownloadProgress {
    pub fn new(total: u64) -> Self {
        DownloadProgress { total: AtomicU64::new(total), downloaded: AtomicU64::new(0) }
    }

    pub fn fraction(&self) -> f64 {
        let t = self.total.load(Ordering::SeqCst);
        if t == 0 { return 1.0; }
        (self.downloaded.load(Ordering::SeqCst) as f64) / (t as f64)
    }
}

/// 根据 origin + path 计算出 raw/ 缓存路径并检查是否存在。
/// 若已存在（非空文件），返回其本地路径；否则返回 None。
pub fn raw_cache_path(origin: &str, path: &str) -> Option<PathBuf> {
    use std::hash::{Hash, Hasher};
    let name = path.rsplit('/').next().unwrap_or("file.cbz");
    let hash = {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        format!("{}{}", origin, path).hash(&mut h);
        format!("{:016x}", h.finish())
    };
    let file_path = crate::cache::CacheDir::Raw
        .ensure().ok()?
        .join(&hash)
        .join(name);
    match std::fs::metadata(&file_path) {
        Ok(meta) if meta.len() > 0 => Some(file_path),
        _ => None,
    }
}

impl WebDavClient {
    /// 创建客户端;返回 (client, 初始浏览路径)。
    pub fn new(base: &str, user: &str, pass: &str) -> Result<(Self, String)> {
        let client = Client::builder()
            .timeout(Duration::from_secs(60))
            .connect_timeout(Duration::from_secs(10))
            .build()
            .context("创建 HTTP 客户端失败")?;
        let u = Url::parse(base).context("无效的 WebDAV URL")?;
        let host = u.host_str().ok_or_else(|| anyhow!("URL 缺少主机"))?;
        let mut origin = format!("{}://{}", u.scheme(), host);
        if let Some(port) = u.port() {
            origin = format!("{}:{}", origin, port);
        }
        let root = if u.path().is_empty() {
            "/".to_string()
        } else {
            u.path().to_string()
        };
        Ok((
            WebDavClient {
                client,
                origin,
                user: user.to_string(),
                pass: pass.to_string(),
                capability: ServerCapability::default(),
            },
            root,
        ))
    }

    fn url(&self, path: &str) -> String {
        let p = if path.starts_with('/') {
            path.to_string()
        } else {
            format!("/{}", path)
        };
        format!("{}{}", self.origin, encode_path(&p))
    }

    /// 服务器 origin(scheme://host[:port]),用于缓存命名空间等。
    pub fn origin(&self) -> &str {
        &self.origin
    }

    fn propfind(&self, path: &str, depth: &str) -> Result<String> {
        let resp = self
            .client
            .request(Method::from_bytes(b"PROPFIND").unwrap(), self.url(path))
            .header("Depth", depth)
            .basic_auth(&self.user, Some(&self.pass))
            .send()
            .context("PROPFIND 请求失败")?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().unwrap_or_default();
            let hint = match status.as_u16() {
                401 => "用户名或密码错误",
                403 => "没有访问权限",
                404 => "路径不存在",
                405 => "地址可能缺少 WebDAV 路径前缀(如 /dav),或服务不支持 PROPFIND",
                429 => "请求过于频繁，请稍后再试",
                _ => "",
            };
            if body.len() > 300 {
                bail!("PROPFIND 失败:HTTP {} {}{}", status.as_u16(), hint, &body[..300]);
            } else if !body.is_empty() {
                bail!("PROPFIND 失败:HTTP {} {}. 请检查地址是否包含完整的 WebDAV 路径(如 /dav)。{}", status.as_u16(), hint, body);
            } else {
                bail!("PROPFIND 失败:HTTP {} {}. ({})", status.as_u16(), hint, self.url(path));
            }
        }
        resp.text().context("读取 PROPFIND 响应失败")
    }

    /// 测试连接 + 自动探测服务器能力。
    pub fn check_and_probe(&mut self, root: &str) -> Result<()> {
        // 1. 基础连通性测试
        self.propfind(root, "0")?;

        // 2. Range 支持探测
        self.capability.range_supported = self.probe_range(root)?;

        // 3. RTT 探测(发 3 次 HEAD,取平均值)
        self.capability.avg_rtt_ms = self.probe_rtt(root)?;

        // 4. 并发建议:根据 RTT 分级
        self.capability.max_concurrency = if self.capability.avg_rtt_ms < 20.0 {
            4 // 本地/NAS
        } else if self.capability.avg_rtt_ms < 100.0 {
            3 // 远程但较快
        } else {
            2 // 慢速远程
        };

        Ok(())
    }

    fn probe_range(&self, root: &str) -> Result<bool> {
        let resp = self
            .client
            .get(self.url(root))
            .header(RANGE, "bytes=0-0")
            .basic_auth(&self.user, Some(&self.pass))
            .send()
            .context("Range 探测失败")?;
        Ok(resp.status() == StatusCode::PARTIAL_CONTENT)
    }

    fn probe_rtt(&self, root: &str) -> Result<f64> {
        let mut total_ms = 0.0;
        let probes = 3;
        for _ in 0..probes {
            let start = std::time::Instant::now();
            let result = self
                .client
                .head(self.url(root))
                .basic_auth(&self.user, Some(&self.pass))
                .send();
            let elapsed = start.elapsed().as_secs_f64() * 1000.0;
            match result {
                Ok(resp) => {
                    let _ = resp.text();
                    total_ms += elapsed;
                }
                Err(_) => {
                    // HEAD 失败不算致命,计入 RTT 上限
                    total_ms += 500.0;
                }
            }
        }
        Ok(total_ms / probes as f64)
    }

    /// 测试连接(仅连通性,不做能力探测)。
    pub fn check(&self, root: &str) -> Result<()> {
        self.propfind(root, "0").map(|_| ())
    }

    /// 列目录(Depth:1),去掉父目录自身,目录在前自然排序。
    pub fn list(&self, path: &str) -> Result<Vec<Entry>> {
        let xml = self.propfind(path, "1")?;
        let mut entries = parse_multistatus(&xml)?;
        let norm = |p: &str| p.trim_end_matches('/').to_string();
        let target = norm(path);
        entries.retain(|e| norm(&e.path) != target && !e.name.is_empty());
        entries.sort_by(|a, b| {
            b.is_dir
                .cmp(&a.is_dir)
                .then_with(|| crate::util::natural_cmp(&a.name, &b.name))
        });
        Ok(entries)
    }

    /// 获取文件大小(getcontentlength,Depth:0)。
    pub fn file_size(&self, path: &str) -> Result<u64> {
        let xml = self.propfind(path, "0")?;
        let entries = parse_multistatus(&xml)?;
        entries
            .first()
            .map(|e| e.size)
            .filter(|s| *s > 0)
            .ok_or_else(|| anyhow!("无法获取文件大小:{}", path))
    }

    /// 探测服务器是否支持 Range(对 bytes=0-0 应返回 206)。
    pub fn range_supported(&self, path: &str) -> Result<bool> {
        let resp = self
            .client
            .get(self.url(path))
            .header(RANGE, "bytes=0-0")
            .basic_auth(&self.user, Some(&self.pass))
            .send()
            .context("Range 探测请求失败")?;
        Ok(resp.status() == StatusCode::PARTIAL_CONTENT)
    }

    /// 下载完整文件到本地磁盘缓存(用于不支持 Range 的服务器回退)。
    /// 若本地已有缓存(非空),直接复用;否则 GET 整包落盘后返回本地 ByteSource 包装。
    pub fn download_full(
        &self,
        path: &str,
    ) -> Result<
        WebDavFile, // 返回 WebDavFile 包装,而非 LocalFile(避免中间再装箱)
    > {
        use std::hash::{Hash, Hasher};

        let cache_dir = crate::cache::cache_root().join("download");
        std::fs::create_dir_all(&cache_dir).ok();
        let name = path.rsplit('/').next().unwrap_or("file.cbz");
        let hash = {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            format!("{}{}", self.origin, path).hash(&mut h);
            format!("{:016x}", h.finish())
        };
        let dir = cache_dir.join(&hash);
        std::fs::create_dir_all(&dir).ok();
        let file_path = dir.join(name);

        // 已有缓存则直接用
        if let Ok(meta) = std::fs::metadata(&file_path) {
            if meta.len() > 0 {
                let f = std::fs::File::open(&file_path).context("打开缓存文件失败")?;
                return Ok(WebDavFile::from_local(
                    Arc::new(WebDavClient {
                        client: self.client.clone(),
                        origin: self.origin.clone(),
                        user: self.user.clone(),
                        pass: self.pass.clone(),
                        capability: self.capability.clone(),
                    }),
                    path.to_string(),
                    meta.len(),
                    f,
                ));
            }
        }

        // 下载整包落盘
        let mut resp = self
            .client
            .get(self.url(path))
            .basic_auth(&self.user, Some(&self.pass))
            .send()
            .map_err(|e| anyhow!("下载整文件失败:{}", e))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().unwrap_or_default();
            anyhow::bail!(
                "下载失败:HTTP {} {} {}",
                status.as_u16(),
                match status.as_u16() {
                    401 => "用户名或密码错误",
                    403 => "没有访问权限",
                    429 => "请求过于频繁",
                    _ => "",
                },
                body.chars().take(200).collect::<String>(),
            );
        }

        let mut disk = std::fs::File::create(&file_path).context("创建缓存文件失败")?;
        std::io::copy(&mut resp, &mut disk).context("写入缓存文件失败")?;
        let f = std::fs::File::open(&file_path).context("打开缓存文件失败")?;
        Ok(WebDavFile::from_local(
            Arc::new(WebDavClient {
                client: self.client.clone(),
                origin: self.origin.clone(),
                user: self.user.clone(),
                pass: self.pass.clone(),
                capability: self.capability.clone(),
            }),
            path.to_string(),
            std::fs::metadata(&file_path)
                .map(|m| m.len())
                .unwrap_or(0),
            f,
        ))
    }

    /// 将远程文件下载到 raw/ 缓存目录。
    /// 若本地已有缓存（非空），直接复用；否则 GET 整包落盘。
    /// 返回本地文件路径，供 LocalFile 打开。
    /// 这是四层架构的入口：WebDAV -> raw/ -> 本地阅读。
    /// `progress` 可选: 提供进度追踪,下载过程中更新 `downloaded` 字段。
    pub fn download_to_raw_cache(
        &self,
        path: &str,
        progress: Option<Arc<DownloadProgress>>,
    ) -> Result<PathBuf> {
        use std::hash::{Hash, Hasher};

        let raw_dir = crate::cache::CacheDir::Raw.ensure()
            .context("创建 raw/ 缓存目录失败")?;

        let name = path.rsplit('/').next().unwrap_or("file.cbz");
        let hash = {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            format!("{}{}", self.origin, path).hash(&mut h);
            format!("{:016x}", h.finish())
        };
        let dir = raw_dir.join(&hash);
        std::fs::create_dir_all(&dir).ok();
        let file_path = dir.join(name);

        // 已有缓存(非空)则直接复用
        if let Ok(meta) = std::fs::metadata(&file_path) {
            if meta.len() > 0 {
                if let Some(p) = &progress {
                    p.downloaded.store(p.total.load(Ordering::SeqCst), Ordering::SeqCst);
                }
                return Ok(file_path);
            }
        }

        // 下载整包
        let mut resp = self
            .client
            .get(self.url(path))
            .basic_auth(&self.user, Some(&self.pass))
            .send()
            .map_err(|e| anyhow!("下载整文件失败: {e}"))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().unwrap_or_default();
            anyhow::bail!(
                "下载失败: HTTP {} {} {}",
                status.as_u16(),
                match status.as_u16() {
                    401 => "用户名或密码错误",
                    403 => "没有访问权限",
                    429 => "请求过于频繁",
                    _ => "",
                },
                body.chars().take(200).collect::<String>(),
            );
        }

        let total = resp.content_length().unwrap_or(0);
        if let Some(p) = &progress {
            p.total.store(total, Ordering::SeqCst); // 更新文件实际大小
        }

        let mut disk = std::fs::File::create(&file_path).context("创建缓存文件失败")?;
        let mut buf = [0u8; 64 * 1024]; // 64KB 读缓冲
        let mut written: u64 = 0;
        loop {
            let n = resp.read(&mut buf).context("读取响应流失败")?;
            if n == 0 { break; }
            disk.write_all(&buf[..n]).context("写入缓存文件失败")?;
            written += n as u64;
            if let Some(p) = &progress {
                p.downloaded.store(written, Ordering::SeqCst);
            }
        }
        disk.flush().context("同步缓存文件失败")?;

        Ok(file_path)
    }

    /// Range 读取:从 offset 读满 buf 或到文件尾,返回实际字节数。
    pub fn read_range(&self, path: &str, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let end = offset + buf.len() as u64 - 1;
        let mut resp = self
            .client
            .get(self.url(path))
            .header(RANGE, format!("bytes={}-{}", offset, end))
            .basic_auth(&self.user, Some(&self.pass))
            .send()
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("Range 请求失败:{e}")))?;
        let status = resp.status();
        if status != StatusCode::PARTIAL_CONTENT {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                format!("服务器未按 Range 返回(HTTP {})", status.as_u16()),
            ));
        }
        // 循环读满 buf(或到响应结束):resp.read 一次可能读不满。
        let mut filled = 0;
        while filled < buf.len() {
            match resp.read(&mut buf[filled..]) {
                Ok(0) => break,
                Ok(n) => filled += n,
                Err(e) => return Err(e),
            }
        }
        Ok(filled)
    }
}

/// WebDAV 远程文件作为 [`ByteSource`]:每次 `read_at` 即一次 HTTP Range 请求。
/// 无内部状态,可并发调用(并行预取多页,各自独立发 Range 请求)。
///
/// 特殊模式:当 `local_cache` 不为空时,读取操作从本地缓存文件读(用于不支持 Range 的服务器)。
pub struct WebDavFile {
    client: Arc<WebDavClient>,
    path: String,
    len: u64,
    local_cache: Option<std::sync::Mutex<std::fs::File>>,
}

impl WebDavFile {
    pub fn new(client: Arc<WebDavClient>, path: String, len: u64) -> Self {
        WebDavFile { client, path, len, local_cache: None }
    }

    /// 包装本地文件缓存(用于不支持 Range 的服务器回退)。
    pub fn from_local(client: Arc<WebDavClient>, path: String, len: u64, cache_file: std::fs::File) -> Self {
        WebDavFile { client, path, len, local_cache: Some(std::sync::Mutex::new(cache_file)) }
    }
}

impl ByteSource for WebDavFile {
    fn len(&self) -> u64 {
        self.len
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        if let Some(local) = &self.local_cache {
            use std::os::windows::fs::FileExt;
            local.lock().unwrap().seek_read(buf, offset)
        } else {
            self.client.read_range(&self.path, offset, buf)
        }
    }
}

/// 提取 tag 本地名(去命名空间前缀,如 `d:response` → `response`)。
fn local_name(tag: &[u8]) -> String {
    let s = String::from_utf8_lossy(tag);
    s.rsplit(':').next().unwrap_or(&s).to_string()
}

/// 解析 PROPFIND 的 multistatus XML,提取条目列表。
fn parse_multistatus(xml: &str) -> Result<Vec<Entry>> {
    use quick_xml::events::Event;
    use quick_xml::Reader;

    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    #[derive(Default)]
    struct Cur {
        href: String,
        name: String,
        size: u64,
        is_dir: bool,
    }

    let mut entries = Vec::new();
    let mut cur: Option<Cur> = None;
    let mut field = String::new();
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let name = local_name(e.name().as_ref());
                match name.as_str() {
                    "response" => cur = Some(Cur::default()),
                    "href" | "displayname" | "getcontentlength" => field = name,
                    "collection" => {
                        if let Some(c) = cur.as_mut() {
                            c.is_dir = true;
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Empty(e)) => {
                // 自闭合空元素:`<d:collection/>` 表示该条目是目录。
                if local_name(e.name().as_ref()) == "collection" {
                    if let Some(c) = cur.as_mut() {
                        c.is_dir = true;
                    }
                }
            }
            Ok(Event::End(e)) => {
                let name = local_name(e.name().as_ref());
                if name == "response" {
                    if let Some(c) = cur.take() {
                        let path = href_to_path(&c.href);
                        let name = if c.name.is_empty() {
                            path.trim_end_matches('/')
                                .rsplit('/')
                                .next()
                                .unwrap_or("")
                                .to_string()
                        } else {
                            c.name.clone()
                        };
                        entries.push(Entry {
                            name,
                            path,
                            is_dir: c.is_dir,
                            size: c.size,
                            mtime: 0,
                        });
                    }
                } else if name == field {
                    field.clear();
                }
            }
            Ok(Event::Text(e)) => {
                if let Some(c) = cur.as_mut() {
                    let raw = String::from_utf8_lossy(e.as_ref());
                    let text = quick_xml::escape::unescape(&raw)
                        .map(|c| c.into_owned())
                        .unwrap_or_else(|_| raw.into_owned());
                    match field.as_str() {
                        "href" => c.href = text,
                        "displayname" => c.name = text,
                        "getcontentlength" => c.size = text.trim().parse().unwrap_or(0),
                        _ => {}
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(anyhow!("解析 PROPFIND XML 失败:{e}")),
            _ => {}
        }
        buf.clear();
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<d:multistatus xmlns:d="DAV:">
  <d:response>
    <d:href>/dav/comic/</d:href>
    <d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype></d:prop></d:propstat>
  </d:response>
  <d:response>
    <d:href>/dav/comic/%E5%A4%8F%E7%9B%AE.cbz</d:href>
    <d:propstat><d:prop>
      <d:displayname>夏目.cbz</d:displayname>
      <d:getcontentlength>12345678</d:getcontentlength>
      <d:resourcetype/>
    </d:prop></d:propstat>
  </d:response>
  <d:response>
    <d:href>/dav/comic/sub/</d:href>
    <d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype></d:prop></d:propstat>
  </d:response>
</d:multistatus>"#;

    #[test]
    fn parse_multistatus_works() {
        let entries = parse_multistatus(SAMPLE).unwrap();
        assert_eq!(entries.len(), 3);
        assert!(entries[0].is_dir);
        let f = entries.iter().find(|e| !e.is_dir).unwrap();
        assert_eq!(f.size, 12345678);
        assert_eq!(f.path, "/dav/comic/夏目.cbz"); // percent-decode 生效
        assert_eq!(f.name, "夏目.cbz");
        let dir_count = entries.iter().filter(|e| e.is_dir).count();
        assert_eq!(dir_count, 2);
    }

    #[test]
    fn encode_path_works() {
        assert_eq!(
            encode_path("/a b/夏目.cbz"),
            "/a%20b/%E5%A4%8F%E7%9B%AE.cbz"
        );
        assert_eq!(encode_path("/plain/name.cbz"), "/plain/name.cbz");
    }

    #[test]
    fn href_to_path_handles_full_url() {
        assert_eq!(
            href_to_path("https://nas.example.com/dav/a%20b.cbz"),
            "/dav/a b.cbz"
        );
        assert_eq!(href_to_path("/dav/x.cbz"), "/dav/x.cbz");
    }
}
