//! S3 书源：path-style S3 REST + SigV4，支持 ListObjectsV2 / HeadObject / Range GetObject。
//!
//! 只依赖公开的 S3 API，不绑定具体厂商；OpenList 的 S3 网关、MinIO、AWS 等均可。
//! 随机读走 `GET` + `Range`，与 WebDAV Range 的消费方式一致。

use super::{webdav::DownloadProgress, ByteSource, Entry};
use anyhow::{anyhow, Context, Result};
use hmac::{Hmac, Mac};
use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const SERVICE: &str = "s3";
const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

/// RFC 3986 非保留字符之外全部编码；S3 canonical URI 保留 `/`。
const ENCODE_SET: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

/// S3 客户端（单个 bucket，可选根前缀）。
pub struct S3Client {
    http: reqwest::blocking::Client,
    /// 形如 `http://127.0.0.1:5246`，无尾斜杠。
    endpoint: String,
    /// endpoint 的 authority（含端口），用于 Host 与 canonical headers。
    authority: String,
    bucket: String,
    access_key: String,
    secret_key: String,
    region: String,
    root: String,
    endpoint_ns: String,
    /// ListObjectsV2 已返回每个对象的 size；封面/阅读器再次 stat 时直接复用，
    /// 省掉每本书一次 `Range: bytes=0-0` 请求（S3 封面场景 240 本 = 240 次）。
    sizes: Mutex<HashMap<String, u64>>,
}

impl S3Client {
    pub fn connect(
        endpoint: &str,
        bucket: &str,
        access_key: &str,
        secret_key: &str,
        region: &str,
        root: &str,
    ) -> Result<Self> {
        let endpoint = normalize_endpoint(endpoint)?;
        let authority = endpoint_authority(&endpoint)?;
        let bucket = bucket.trim().trim_matches('/').to_string();
        if bucket.is_empty() {
            return Err(anyhow!("S3 bucket must not be empty"));
        }
        if access_key.trim().is_empty() || secret_key.is_empty() {
            return Err(anyhow!("S3 access key and secret key must not be empty"));
        }
        let http = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .context("创建 S3 HTTP 客户端失败")?;
        let client = S3Client {
            http,
            endpoint_ns: format!("{endpoint}/{bucket}"),
            endpoint,
            authority,
            bucket,
            access_key: access_key.to_string(),
            secret_key: secret_key.to_string(),
            region: if region.trim().is_empty() {
                "us-east-1".to_string()
            } else {
                region.trim().to_string()
            },
            root: normalize_root(root),
            sizes: Mutex::new(HashMap::new()),
        };
        // 校验 endpoint/bucket/凭据可用。
        client.list("/")?;
        Ok(client)
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint_ns
    }

    pub fn root(&self) -> &str {
        &self.root
    }

    pub fn raw_cache_path(&self, path: &str) -> Option<PathBuf> {
        super::sftp::raw_cache_path(&self.endpoint_ns, path)
    }

    pub fn raw_cache_candidate_path(&self, path: &str) -> PathBuf {
        super::sftp::raw_cache_candidate_path(&self.endpoint_ns, path)
    }

    /// 列出目录（目录在前，自然排序）。ListObjectsV2 + delimiter=/。
    pub fn list(&self, path: &str) -> Result<Vec<Entry>> {
        let prefix = self.key_prefix(path);
        let mut token: Option<String> = None;
        let mut out: Vec<Entry> = Vec::new();
        for _ in 0..1000 {
            let mut query = vec![
                ("list-type".to_string(), "2".to_string()),
                ("delimiter".to_string(), "/".to_string()),
            ];
            if !prefix.is_empty() {
                query.push(("prefix".to_string(), prefix.clone()));
            }
            if let Some(token) = &token {
                query.push(("continuation-token".to_string(), token.clone()));
            }
            // 桶根必须带尾斜杠：gofakes3/OpenList 把 `/bucket` 当作对象 key。
            let (status, body) = self.request("GET", &format!("/{}/", self.bucket), &query, &[], None)?;
            if !status.is_success() {
                return Err(anyhow!("S3 列目录失败: HTTP {status}"));
            }
            let result: ListBucketResult = quick_xml::de::from_str(&body)
                .map_err(|e| anyhow!("S3 列目录响应解析失败: {e}"))?;
            for object in result.contents {
                let size = object.size.unwrap_or(0);
                if let Some(entry) = self.object_entry(&object.key, size) {
                    self.sizes
                        .lock()
                        .unwrap()
                        .insert(entry.path.clone(), size);
                    out.push(entry);
                }
            }
            for common in result.common_prefixes {
                if let Some(entry) = self.prefix_entry(&common.prefix) {
                    out.push(entry);
                }
            }
            if result.is_truncated.unwrap_or(false) {
                token = result.next_continuation_token;
                if token.is_none() {
                    break;
                }
            } else {
                break;
            }
        }
        out.sort_by(|a, b| {
            b.is_dir
                .cmp(&a.is_dir)
                .then_with(|| crate::util::natural_cmp(&a.name, &b.name))
        });
        Ok(out)
    }

    pub fn file_size(&self, path: &str) -> Result<u64> {
        if let Some(size) = self
            .sizes
            .lock()
            .ok()
            .and_then(|sizes| sizes.get(path).copied())
        {
            return Ok(size);
        }
        let key = self.object_key(path);
        // 签名用未编码的 key：OpenList 的 S3 网关按原始路径计算 canonical URI；
        // 请求 URL 再用百分号编码，避免 `#`/`|`/空格被 reqwest 当成 fragment。
        let signing_path = format!("/{}/{}", self.bucket, key);
        let request_path = self.encoded_object_path(path);
        // OpenList 的 HEAD 可能只回 Content-Length: 0；用 Range: bytes=0-0 的
        // Content-Range 总长度更可靠，失败时再退回 Content-Length。
        let range = "bytes=0-0";
        let amz_date = now_amz_date();
        let auth = self.authorization(
            "GET",
            &signing_path,
            &[],
            &[("range", range)],
            EMPTY_SHA256,
            &amz_date,
        )?;
        let response = self
            .http
            .get(format!("{}{}", self.endpoint, request_path))
            .header("x-amz-content-sha256", EMPTY_SHA256)
            .header("x-amz-date", &amz_date)
            .header("Range", range)
            .header("Authorization", auth)
            .send()
            .map_err(|e| anyhow!("S3 获取文件大小失败: {e}"))?;
        if !response.status().is_success() {
            return Err(anyhow!(
                "S3 获取文件大小失败: HTTP {}",
                response.status()
            ));
        }
        if let Some(total) = response
            .headers()
            .get(reqwest::header::CONTENT_RANGE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.rsplit('/').next())
            .and_then(|value| value.trim().parse::<u64>().ok())
        {
            self.sizes
                .lock()
                .unwrap()
                .insert(path.to_string(), total);
            return Ok(total);
        }
        response
            .content_length()
            .ok_or_else(|| anyhow!("S3 获取文件大小失败: 响应缺少长度"))
    }

    /// 随机读：GET + `Range: bytes=offset-end`。
    pub fn read_at(&self, path: &str, offset: u64, buf: &mut [u8]) -> std::io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let end = offset.saturating_add(buf.len() as u64).saturating_sub(1);
        let range = format!("bytes={offset}-{end}");
        let key = self.object_key(path);
        let signing_path = format!("/{}/{}", self.bucket, key);
        let url = format!("{}{}", self.endpoint, self.encoded_object_path(path));
        // OpenList 的 S3 端点在并发封面抓取时会偶发复位连接；对瞬时错误重试一次，
        // 避免把可恢复的网络抖动记成封面失败。
        let mut last_error = None;
        for attempt in 0..2 {
            let amz_date = now_amz_date();
            let auth = self
                .authorization(
                    "GET",
                    &signing_path,
                    &[],
                    &[("range", range.as_str())],
                    EMPTY_SHA256,
                    &amz_date,
                )
                .map_err(|e| std::io::Error::other(format!("S3 签名失败: {e}")))?;
            match self
                .http
                .get(&url)
                .header("x-amz-content-sha256", EMPTY_SHA256)
                .header("x-amz-date", &amz_date)
                .header("Range", &range)
                .header("Authorization", auth)
                .send()
            {
                Ok(mut response) if response.status().is_success() => {
                    return response.read(buf);
                }
                Ok(response) => {
                    let transient = response.status().is_server_error()
                        || response.status().as_u16() == 429;
                    let status = response.status();
                    if attempt == 0 && transient {
                        last_error = Some(format!("HTTP {status}"));
                        continue;
                    }
                    return Err(std::io::Error::other(format!("S3 GET 失败: HTTP {status}")));
                }
                Err(error) => {
                    let transient =
                        error.is_timeout() || error.is_connect() || error.is_request();
                    if attempt == 0 && transient {
                        last_error = Some(error.to_string());
                        continue;
                    }
                    return Err(std::io::Error::other(format!("S3 GET 请求失败: {error}")));
                }
            }
        }
        Err(std::io::Error::other(format!(
            "S3 GET 请求失败: {}",
            last_error.unwrap_or_else(|| "unknown error".to_string())
        )))
    }

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
        let key = self.object_key(path);
        let url = self.signed_url("GET", &format!("/{}/{}", self.bucket, key), &[], &[])?;
        let mut response = self
            .http
            .get(url)
            .header("x-amz-content-sha256", EMPTY_SHA256)
            .header("x-amz-date", now_amz_date())
            .header(
                "Authorization",
                self.authorization(
                    "GET",
                    &format!("/{}/{}", self.bucket, key),
                    &[],
                    &[],
                    EMPTY_SHA256,
                    &now_amz_date(),
                )?,
            )
            .send()
            .map_err(|e| anyhow!("S3 GET 请求失败: {e}"))?;
        if !response.status().is_success() {
            return Err(anyhow!("S3 下载失败: HTTP {}", response.status()));
        }
        let mut writer = crate::cache::AtomicCacheFile::create(&file_path)?;
        let mut buf = vec![0u8; 64 * 1024];
        let mut written = 0u64;
        loop {
            let n = response
                .read(&mut buf)
                .map_err(|e| anyhow!("S3 下载读取失败: {e}"))?;
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

    fn key_prefix(&self, path: &str) -> String {
        let root = self.root.trim_matches('/');
        let sub = path.trim_matches('/');
        let mut prefix = match (root.is_empty(), sub.is_empty()) {
            (true, true) => String::new(),
            (true, false) => sub.to_string(),
            (false, true) => root.to_string(),
            (false, false) => format!("{root}/{sub}"),
        };
        if !prefix.is_empty() {
            prefix.push('/');
        }
        prefix
    }

    fn object_key(&self, path: &str) -> String {
        let root = self.root.trim_matches('/');
        let sub = path.trim_matches('/');
        match (root.is_empty(), sub.is_empty()) {
            (true, _) => sub.to_string(),
            (false, true) => root.to_string(),
            (false, false) => format!("{root}/{sub}"),
        }
    }

    /// 对象的 URI 路径：各段做 RFC 3986 编码，保留 `/`。
    /// `#`、`?`、`|`、空格、非 ASCII 等如果不编码，会被 reqwest 当成
    /// fragment/query 分隔符或直接拒绝，导致 S3 返回 404/400。
    fn encoded_object_path(&self, path: &str) -> String {
        let key = self.object_key(path);
        let encoded = key
            .split('/')
            .map(|segment| utf8_percent_encode(segment, ENCODE_SET).to_string())
            .collect::<Vec<_>>()
            .join("/");
        format!("/{}/{}", self.bucket, encoded)
    }

    fn object_entry(&self, key: &str, size: u64) -> Option<Entry> {
        let relative = self.relative_key(key)?;
        if relative.is_empty() || relative.ends_with('/') {
            return None;
        }
        let name = relative.rsplit('/').next()?.to_string();
        Some(Entry {
            name,
            path: format!("/{relative}"),
            is_dir: false,
            size,
            mtime: 0,
        })
    }

    fn prefix_entry(&self, prefix: &str) -> Option<Entry> {
        let relative = self.relative_key(prefix.trim_end_matches('/'))?;
        if relative.is_empty() {
            return None;
        }
        let name = relative.rsplit('/').next()?.to_string();
        Some(Entry {
            name,
            path: format!("/{relative}"),
            is_dir: true,
            size: 0,
            mtime: 0,
        })
    }

    fn relative_key(&self, key: &str) -> Option<String> {
        let root = self.root.trim_matches('/');
        if root.is_empty() {
            return Some(key.trim_start_matches('/').to_string());
        }
        let with_slash = format!("{root}/");
        if key == root {
            return Some(String::new());
        }
        key.strip_prefix(&with_slash).map(|rest| rest.to_string())
    }

    fn signed_url(
        &self,
        method: &str,
        path: &str,
        query: &[(String, String)],
        extra_headers: &[(&str, &str)],
    ) -> Result<reqwest::Url> {
        let amz_date = now_amz_date();
        let auth = self.authorization(
            method,
            path,
            query,
            extra_headers,
            EMPTY_SHA256,
            &amz_date,
        )?;
        let mut url = format!("{}{}", self.endpoint, path);
        let qs = canonical_query(query);
        if !qs.is_empty() {
            url.push('?');
            url.push_str(&qs);
        }
        let mut request = self
            .http
            .request(reqwest::Method::from_bytes(method.as_bytes()).unwrap(), &url)
            .header("x-amz-content-sha256", EMPTY_SHA256)
            .header("x-amz-date", amz_date)
            .header("Authorization", auth);
        for (name, value) in extra_headers {
            request = request.header(*name, *value);
        }
        Ok(request.build()?.url().clone())
    }

    fn authorization(
        &self,
        method: &str,
        path: &str,
        query: &[(String, String)],
        extra_headers: &[(&str, &str)],
        payload_hash: &str,
        amz_date: &str,
    ) -> Result<String> {
        let date = &amz_date[..8];
        let scope = format!("{date}/{}/{SERVICE}/aws4_request", self.region);
        let canonical_uri = path
            .split('/')
            .map(|segment| utf8_percent_encode(segment, ENCODE_SET).to_string())
            .collect::<Vec<_>>()
            .join("/");
        let canonical_query = canonical_query(query);
        let mut headers: Vec<(String, String)> = vec![
            ("host".to_string(), self.authority.clone()),
            ("x-amz-content-sha256".to_string(), payload_hash.to_string()),
            ("x-amz-date".to_string(), amz_date.to_string()),
        ];
        for (name, value) in extra_headers {
            headers.push((name.to_ascii_lowercase(), value.to_string()));
        }
        headers.sort_by(|a, b| a.0.cmp(&b.0));
        let canonical_headers = headers
            .iter()
            .map(|(name, value)| format!("{name}:{value}\n"))
            .collect::<String>();
        let signed_headers = headers
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>()
            .join(";");
        let canonical_request = format!(
            "{method}\n{canonical_uri}\n{canonical_query}\n{canonical_headers}\n{signed_headers}\n{payload_hash}"
        );
        if std::env::var("RCH_S3_DEBUG").is_ok() {
            eprintln!("--- canonical request ---\n{canonical_request}\n---");
        }
        let string_to_sign = format!(
            "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
            hex(&Sha256::digest(canonical_request.as_bytes()))
        );
        let signing_key = signing_key(&self.secret_key, date, &self.region);
        let mut mac = Hmac::<Sha256>::new_from_slice(&signing_key)
            .map_err(|e| anyhow!("S3 签名密钥无效: {e}"))?;
        mac.update(string_to_sign.as_bytes());
        let signature = hex(&mac.finalize().into_bytes());
        Ok(format!(
            "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed_headers}, Signature={signature}",
            self.access_key
        ))
    }

    fn request(
        &self,
        method: &str,
        path: &str,
        query: &[(String, String)],
        extra_headers: &[(&str, &str)],
        body: Option<&[u8]>,
    ) -> Result<(reqwest::StatusCode, String)> {
        let amz_date = now_amz_date();
        let payload_hash = match body {
            Some(bytes) => hex(&Sha256::digest(bytes)),
            None => EMPTY_SHA256.to_string(),
        };
        let auth = self.authorization(
            method,
            path,
            query,
            extra_headers,
            &payload_hash,
            &amz_date,
        )?;
        let url = self.signed_url(method, path, query, extra_headers)?;
        let mut request = self
            .http
            .request(reqwest::Method::from_bytes(method.as_bytes()).unwrap(), url)
            .header("x-amz-content-sha256", &payload_hash)
            .header("x-amz-date", amz_date)
            .header("Authorization", auth);
        for (name, value) in extra_headers {
            request = request.header(*name, *value);
        }
        if let Some(bytes) = body {
            request = request.body(bytes.to_vec());
        }
        let response = request.send().map_err(|e| anyhow!("S3 请求失败: {e}"))?;
        if std::env::var("RCH_S3_DEBUG").is_ok() {
            eprintln!("--- actual url ---\n{}\n---", response.url());
        }
        let status = response.status();
        let body = response.text().unwrap_or_default();
        Ok((status, body))
    }
}

/// S3 远端文件作为 [`ByteSource`]。
pub struct S3File {
    client: Arc<S3Client>,
    path: String,
    len: u64,
}

impl S3File {
    pub fn new(client: Arc<S3Client>, path: String, len: u64) -> Self {
        S3File { client, path, len }
    }
}

impl ByteSource for S3File {
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

fn normalize_endpoint(endpoint: &str) -> Result<String> {
    let endpoint = endpoint.trim().trim_end_matches('/');
    if endpoint.is_empty() {
        return Err(anyhow!("S3 endpoint must not be empty"));
    }
    if endpoint.starts_with("http://") || endpoint.starts_with("https://") {
        Ok(endpoint.to_string())
    } else {
        Ok(format!("http://{endpoint}"))
    }
}

fn endpoint_authority(endpoint: &str) -> Result<String> {
    let url = reqwest::Url::parse(endpoint).map_err(|e| anyhow!("S3 endpoint 无效: {e}"))?;
    let host = url
        .host_str()
        .ok_or_else(|| anyhow!("S3 endpoint 缺少主机名"))?;
    let default_port = if url.scheme() == "https" { 443 } else { 80 };
    let port = url.port().unwrap_or(default_port);
    if port == default_port {
        Ok(host.to_string())
    } else if host.contains(':') {
        Ok(format!("[{host}]:{port}"))
    } else {
        Ok(format!("{host}:{port}"))
    }
}

fn normalize_root(root: &str) -> String {
    let trimmed = root.trim().trim_matches('/');
    if trimmed.is_empty() {
        String::new()
    } else {
        format!("/{trimmed}")
    }
}

fn canonical_query(query: &[(String, String)]) -> String {
    let mut pairs: Vec<(String, String)> = query
        .iter()
        .map(|(key, value)| {
            (
                utf8_percent_encode(key, ENCODE_SET).to_string(),
                utf8_percent_encode(value, ENCODE_SET).to_string(),
            )
        })
        .collect();
    pairs.sort();
    pairs
        .into_iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("&")
}

fn now_amz_date() -> String {
    // 避免引入 chrono/time：用 SystemTime 手工换算 UTC 日历时间。
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}{month:02}{day:02}T{:02}{:02}{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Howard Hinnant 的 civil_from_days 算法。
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    (y + if m <= 2 { 1 } else { 0 }, m as u32, d as u32)
}

fn signing_key(secret: &str, date: &str, region: &str) -> Vec<u8> {
    let k_date = hmac_sha256(format!("AWS4{secret}").as_bytes(), date.as_bytes());
    let k_region = hmac_sha256(&k_date, region.as_bytes());
    let k_service = hmac_sha256(&k_region, SERVICE.as_bytes());
    hmac_sha256(&k_service, b"aws4_request")
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(Debug, Deserialize)]
struct ListBucketResult {
    #[serde(rename = "Contents", default)]
    contents: Vec<S3Object>,
    #[serde(rename = "CommonPrefixes", default)]
    common_prefixes: Vec<S3CommonPrefix>,
    #[serde(rename = "IsTruncated", default)]
    is_truncated: Option<bool>,
    #[serde(rename = "NextContinuationToken", default)]
    next_continuation_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct S3Object {
    #[serde(rename = "Key")]
    key: String,
    #[serde(rename = "Size", default)]
    size: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct S3CommonPrefix {
    #[serde(rename = "Prefix")]
    prefix: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_keys_and_entries() {
        let client = S3Client {
            http: reqwest::blocking::Client::new(),
            endpoint: "http://127.0.0.1:5246".to_string(),
            authority: "127.0.0.1:5246".to_string(),
            endpoint_ns: "http://127.0.0.1:5246/1".to_string(),
            bucket: "1".to_string(),
            access_key: "a".to_string(),
            secret_key: "s".to_string(),
            region: "us-east-1".to_string(),
            root: "/日漫".to_string(),
            sizes: Mutex::new(HashMap::new()),
        };
        assert_eq!(client.key_prefix("/"), "日漫/");
        assert_eq!(client.key_prefix("/子目录"), "日漫/子目录/");
        assert_eq!(client.object_key("/a.cbz"), "日漫/a.cbz");
        assert_eq!(
            client.encoded_object_path("/a#b|c d.cbz"),
            "/1/%E6%97%A5%E6%BC%AB/a%23b%7Cc%20d.cbz"
        );
        let entry = client
            .object_entry("日漫/a.cbz", 12)
            .expect("object entry");
        assert_eq!(entry.name, "a.cbz");
        assert_eq!(entry.path, "/a.cbz");
        assert_eq!(entry.size, 12);
        let dir = client
            .prefix_entry("日漫/子目录/")
            .expect("prefix entry");
        assert!(dir.is_dir);
        assert_eq!(dir.path, "/子目录");
    }

    #[test]
    fn formats_amz_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20_000), (2024, 10, 4));
    }
}
