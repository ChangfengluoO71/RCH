//! 百度网盘书源：百度网盘开放平台官方 API（OAuth2 + xpan 文件接口）。
//!
//! - 鉴权：授权码模式（`redirect_uri=oob`），refresh_token 长期有效，access_token 自动刷新。
//! - 列目录：`GET /rest/2.0/xpan/file?method=list`（`web=1` 返回缩略图字段）。
//! - 下载直链：`GET /rest/2.0/xpan/multimedia?method=filemetas&dlink=1`（dlink 约 8h 有效）。
//! - 下载：dlink 必须拼接当前 `access_token` + `User-Agent: pan.baidu.com`（官方要求，否则 31045；>50MB 必须 UA）。
//!
//! 契约细节见 `.trellis/tasks/08-03-m6-netdisk-official-api/research/baidu-openapi-contract.md`。
use super::{ByteSource, Entry, RateGate};
use crate::source::webdav::DownloadProgress;
use anyhow::{anyhow, bail, Context, Result};
use reqwest::blocking::Client;
use reqwest::header::{CONTENT_TYPE, RANGE, USER_AGENT};
use reqwest::{StatusCode, Url};
use serde::Deserialize;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const API_AUTHORIZE: &str = "https://openapi.baidu.com/oauth/2.0/authorize";
const API_TOKEN: &str = "https://openapi.baidu.com/oauth/2.0/token";
const API_FILE: &str = "https://pan.baidu.com/rest/2.0/xpan/file";
const API_MULTIMEDIA: &str = "https://pan.baidu.com/rest/2.0/xpan/multimedia";
/// 百度下载/列表请求的固定 UA（>20MB 文件下载必须，官方示例亦带）。
const BAIDU_UA: &str = "pan.baidu.com";
const PAGE_SIZE: i64 = 200;
const LOOKUP_PAGE_SIZE: i64 = 1000;

/// 授权码换 token 的响应（同时是刷新 token 的响应）。
#[derive(Debug, Clone, Deserialize)]
struct TokenResp {
    access_token: Option<String>,
    refresh_token: Option<String>,
    expires_in: Option<i64>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    error_description: Option<String>,
}

/// 百度 token 对（Dart 侧持久化 refresh_token 用）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct TokenPair {
    pub access_token: String,
    pub refresh_token: String,
}

/// 构造 OAuth 授权链接（桌面应用 redirect_uri=oob：授权后页面直接显示 code）。
pub fn auth_url(app_key: &str) -> String {
    let mut u = Url::parse(API_AUTHORIZE).expect("static url");
    u.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", app_key)
        .append_pair("redirect_uri", "oob")
        .append_pair("scope", "basic,netdisk")
        .append_pair("display", "popup");
    u.to_string()
}

fn http_client() -> Result<Client> {
    Client::builder()
        .timeout(Duration::from_secs(60))
        .connect_timeout(Duration::from_secs(10))
        .build()
        .context("创建 HTTP 客户端失败")
}

/// 官方要求：使用 dlink 必须拼接 `&access_token=xxx`。此函数把 dlink 上已有的
/// access_token 替换为传入的当前 token，避免残留已失效的旧 token（31045 根因）。
fn dlink_with_access_token(dlink: &str, token: &str) -> Result<String> {
    let mut u = Url::parse(dlink).context("解析百度 dlink 失败")?;
    let pairs: Vec<(String, String)> = u
        .query_pairs()
        .filter(|(k, _)| k != "access_token")
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    u.query_pairs_mut()
        .clear()
        .extend_pairs(pairs)
        .append_pair("access_token", token);
    Ok(u.to_string())
}

/// 授权码换 token（纯函数，不建会话）。
pub fn exchange_code(app_key: &str, secret: &str, code: &str) -> Result<TokenPair> {
    let resp = http_client()?
        .post(API_TOKEN)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("client_id", app_key),
            ("client_secret", secret),
            ("redirect_uri", "oob"),
        ])
        .send()
        .context("请求百度 token 失败")?;
    let status = resp.status();
    let body: TokenResp = resp.json().context("解析百度 token 响应失败")?;
    let (at, rt) = match (body.access_token, body.refresh_token) {
        (Some(a), Some(r)) => (a, r),
        _ => {
            bail!(
                "授权失败:{} {}",
                body.error.unwrap_or_default(),
                body.error_description.unwrap_or_default()
            )
        }
    };
    if !status.is_success() {
        bail!("百度 token 接口返回 HTTP {}", status.as_u16());
    }
    Ok(TokenPair {
        access_token: at,
        refresh_token: rt,
    })
}

/// xpan 文件列表响应。
#[derive(Debug, Deserialize)]
struct ListResp {
    #[serde(default)]
    list: Vec<ListItem>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ListItem {
    fs_id: u64,
    path: String,
    isdir: i32,
    server_filename: String,
    size: u64,
    server_mtime: i64,
}

/// filemetas 响应（dlink）。
#[derive(Debug, Deserialize)]
struct FileMetasResp {
    #[serde(default)]
    list: Vec<FileMetaItem>,
}

#[derive(Debug, Deserialize)]
struct FileMetaItem {
    fs_id: u64,
    dlink: Option<String>,
    size: Option<u64>,
}

/// 百度网盘客户端（blocking，配合 spawn_blocking 使用）。
pub struct BaiduClient {
    client: Client,
    app_key: String,
    secret: String,
    refresh_token: Mutex<String>,
    access: Mutex<Option<(String, i64)>>, // (access_token, 过期时间戳 unix 秒)
    root: String,
    gate: RateGate,
}

impl BaiduClient {
    pub fn new(app_key: &str, secret: &str, refresh_token: &str, root: &str) -> Result<Self> {
        Ok(BaiduClient {
            client: http_client()?,
            app_key: app_key.to_string(),
            secret: secret.to_string(),
            refresh_token: Mutex::new(refresh_token.to_string()),
            access: Mutex::new(None),
            root: if root.is_empty() { "/".to_string() } else { root.to_string() },
            gate: RateGate::new(5.0), // 百度接口有频率限制，5 r/s 保守节流
        })
    }

    /// 缓存命名空间前缀（raw/ 缓存 hash 与 session 标识）。
    pub fn origin(&self) -> String {
        format!("baidu:{}:{}", self.app_key, self.root)
    }

    pub fn root(&self) -> &str {
        &self.root
    }

    /// 刷新 access_token（同时轮换 refresh_token 并更新内存），返回新 token 对供回写。
    pub fn refresh(&self) -> Result<TokenPair> {
        let rt = self.refresh_token.lock().unwrap().clone();
        let resp = self
            .client
            .post(API_TOKEN)
            .form(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", &rt),
                ("client_id", &self.app_key),
                ("client_secret", &self.secret),
            ])
            .send()
            .context("刷新百度 token 失败")?;
        let body: TokenResp = resp.json().context("解析刷新 token 响应失败")?;
        let (at, rt) = match (body.access_token, body.refresh_token) {
            (Some(a), Some(r)) => (a, r),
            _ => bail!(
                "刷新 token 失败:{} {}",
                body.error.unwrap_or_default(),
                body.error_description.unwrap_or_default()
            ),
        };
        let expires_in = body.expires_in.unwrap_or(30 * 24 * 3600);
        let expires_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64 + expires_in - 300)
            .unwrap_or(i64::MAX);
        *self.refresh_token.lock().unwrap() = rt.clone();
        *self.access.lock().unwrap() = Some((at.clone(), expires_at));
        Ok(TokenPair {
            access_token: at,
            refresh_token: rt,
        })
    }

    /// 取有效 access_token；过期则自动刷新。
    fn ensure_token(&self) -> Result<String> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        if let Some((t, exp)) = self.access.lock().unwrap().as_ref() {
            if *exp > now {
                return Ok(t.clone());
            }
        }
        let pair = self.refresh()?;
        Ok(pair.access_token)
    }

    /// 给 dlink 附加当前有效 access_token（官方要求；token 过期会自动刷新）。
    fn dlink_with_token(&self, dlink: &str) -> Result<String> {
        let token = self.ensure_token()?;
        dlink_with_access_token(dlink, &token)
    }

    /// 带当前 access_token + UA 的 dlink GET（range 可选），下载/探测统一入口。
    fn dlink_get(&self, dlink: &str, range: Option<&str>) -> Result<reqwest::blocking::Response> {
        let url = self.dlink_with_token(dlink)?;
        let mut req = self.client.get(url).header(USER_AGENT, BAIDU_UA);
        if let Some(range) = range {
            req = req.header(RANGE, range);
        }
        req.send().context("请求百度 dlink 失败")
    }

    /// 统一 GET 封装：带 UA + access_token；遇 -6/110/31045 自动刷新重试一次。
    fn get(&self, url: &str, params: &[(&str, String)]) -> Result<(i64, String)> {
        let mut attempts = 0;
        loop {
            self.gate.wait();
            let token = self.ensure_token()?;
            let req = self
                .client
                .get(url)
                .header(USER_AGENT, BAIDU_UA)
                .query(&[("access_token", &token)])
                .query(params);
            let resp = req
                .send()
                .context("百度 API 请求失败")?;
            let status = resp.status();
            let body = resp.text().unwrap_or_default();
            if !status.is_success() {
                bail!(
                    "百度 API HTTP {}: {}",
                    status.as_u16(),
                    body.chars().take(200).collect::<String>()
                );
            }
            let parsed: serde_json::Value =
                serde_json::from_str(&body).context("解析百度 API 响应失败")?;
            let errno = parsed.get("errno").and_then(|v| v.as_i64()).unwrap_or(0);
            if (errno == -6 || errno == 110 || errno == 31045) && attempts == 0 {
                attempts += 1;
                self.access.lock().unwrap().take();
                continue; // 刷新后重试一次
            }
            return Ok((errno, body));
        }
    }

    /// 列目录（分页拉全，目录在前自然排序）。
    pub fn list(&self, dir: &str) -> Result<Vec<Entry>> {
        let mut all = Vec::new();
        let mut start = 0i64;
        loop {
            let (errno, body) = self.get(
                API_FILE,
                &[
                    ("method", "list".to_string()),
                    ("dir", dir.to_string()),
                    ("start", start.to_string()),
                    ("limit", PAGE_SIZE.to_string()),
                    ("order", "name".to_string()),
                    ("desc", "0".to_string()),
                    ("web", "1".to_string()),
                ],
            )?;
            check_errno(errno)?;
            let parsed: ListResp = serde_json::from_str(&body).context("解析文件列表失败")?;
            let n = parsed.list.len();
            all.extend(parsed.list.into_iter().map(|it| Entry {
                name: it.server_filename,
                path: it.path,
                is_dir: it.isdir == 1,
                size: it.size,
                mtime: it.server_mtime,
            }));
            if n < PAGE_SIZE as usize {
                break;
            }
            start += PAGE_SIZE;
        }
        all.sort_by(|a, b| {
            b.is_dir
                .cmp(&a.is_dir)
                .then_with(|| crate::util::natural_cmp(&a.name, &b.name))
        });
        Ok(all)
    }

    /// 按完整路径查找 fs_id（列父目录分页查找）。
    pub fn fs_id_of_path(&self, path: &str) -> Result<u64> {
        let path = path.trim_end_matches('/');
        let (parent, name) = match path.rfind('/') {
            Some(idx) => {
                if idx == 0 {
                    ("/".to_string(), path[1..].to_string())
                } else {
                    (path[..idx].to_string(), path[idx + 1..].to_string())
                }
            }
            None => ("/".to_string(), path.to_string()),
        };
        if name.is_empty() {
            bail!("无效路径:{}", path);
        }
        let mut start = 0i64;
        loop {
            let (errno, body) = self.get(
                API_FILE,
                &[
                    ("method", "list".to_string()),
                    ("dir", parent.clone()),
                    ("start", start.to_string()),
                    ("limit", LOOKUP_PAGE_SIZE.to_string()),
                    ("order", "name".to_string()),
                    ("desc", "0".to_string()),
                    ("web", "0".to_string()),
                ],
            )?;
            check_errno(errno)?;
            let parsed: ListResp = serde_json::from_str(&body).context("解析文件列表失败")?;
            if let Some(it) = parsed.list.iter().find(|it| {
                it.path.trim_end_matches('/') == path
                    || it.server_filename == name
            }) {
                return Ok(it.fs_id);
            }
            let n = parsed.list.len();
            if n < LOOKUP_PAGE_SIZE as usize {
                break;
            }
            start += LOOKUP_PAGE_SIZE;
        }
        bail!("在百度网盘中找不到文件:{}", path)
    }

    /// 取文件下载直链 + 大小（filemetas，dlink 约 8h 有效）。
    pub fn dlink(&self, path: &str) -> Result<(String, u64)> {
        let fs_id = self.fs_id_of_path(path)?;
        let fsids = format!("[{}]", fs_id);
        let (errno, body) = self.get(
            API_MULTIMEDIA,
            &[
                ("method", "filemetas".to_string()),
                ("fsids", fsids),
                ("dlink", "1".to_string()),
            ],
        )?;
        check_errno(errno)?;
        let parsed: FileMetasResp = serde_json::from_str(&body).context("解析文件元信息失败")?;
        let item = parsed
            .list
            .into_iter()
            .find(|it| it.fs_id == fs_id)
            .ok_or_else(|| anyhow!("未取到文件元信息:{}", path))?;
        let link = item.dlink.ok_or_else(|| anyhow!("文件无下载链接:{}", path))?;
        Ok((link, item.size.unwrap_or(0)))
    }

    /// 探测 dlink 是否支持 Range（bytes=0-0 → 206）。
    pub fn probe_range(&self, dlink: &str) -> bool {
        self.dlink_get(dlink, Some("bytes=0-0"))
            .map(|r| r.status() == StatusCode::PARTIAL_CONTENT)
            .unwrap_or(false)
    }

    /// Range 读：GET dlink + Range；dlink 失效（403）时重取一次。
    pub fn read_range_with_dlink(
        &self,
        dlink: &str,
        path: &str,
        offset: u64,
        buf: &mut [u8],
    ) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let end = offset + buf.len() as u64 - 1;
        let range = format!("bytes={}-{}", offset, end);
        let mut resp = self
            .dlink_get(dlink, Some(&range))
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("Range 请求失败:{e}")))?;
        let status = resp.status();
        if status == StatusCode::FORBIDDEN {
            // dlink 失效（过期）或 access_token 被轮换（31045）：强制刷新 token 后重取 dlink 再试一次
            self.access.lock().unwrap().take();
            let (new_link, _) = self
                .dlink(path)
                .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("重取 dlink 失败:{e}")))?;
            resp = self
                .dlink_get(&new_link, Some(&range))
                .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("Range 请求失败:{e}")))?;
        }
        if resp.status() != StatusCode::PARTIAL_CONTENT {
            let final_status = resp.status();
            let body = resp.text().unwrap_or_default();
            return Err(io::Error::new(
                io::ErrorKind::Other,
                format!(
                    "百度 dlink 未支持 Range(HTTP {})：{}",
                    final_status.as_u16(),
                    body.chars().take(160).collect::<String>()
                ),
            ));
        }
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

    /// 整本下载到 raw/ 缓存（有进度）；已缓存则复用。
    pub fn download_to_raw_cache(
        &self,
        path: &str,
        progress: Option<Arc<DownloadProgress>>,
    ) -> Result<PathBuf> {
        if let Some(p) = raw_cache_path(&self.origin(), path) {
            if let Some(prog) = &progress {
                if let Ok(meta) = std::fs::metadata(&p) {
                    prog.downloaded
                        .store(meta.len(), Ordering::SeqCst);
                    prog.total.store(meta.len(), Ordering::SeqCst);
                }
            }
            return Ok(p);
        }
        let (link, size) = self.dlink(path)?;
        let raw_dir = crate::cache::CacheDir::Raw
            .ensure()
            .context("创建 raw/ 缓存目录失败")?;
        let name = path.rsplit('/').next().unwrap_or("file.cbz");
        let hash = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            format!("{}{}", self.origin(), path).hash(&mut h);
            format!("{:016x}", h.finish())
        };
        let dir = raw_dir.join(&hash);
        std::fs::create_dir_all(&dir).ok();
        let file_path = dir.join(name);
        if let Ok(meta) = std::fs::metadata(&file_path) {
            if meta.len() > 0 {
                return Ok(file_path);
            }
        }
        let mut resp = self
            .dlink_get(&link, None)
            .map_err(|e| anyhow!("下载失败:{e}"))?;
        let status = resp.status();
        if status == StatusCode::FORBIDDEN {
            // 同上：dlink 失效或 access_token 被轮换（31045），先强制刷新 token 再重试
            self.access.lock().unwrap().take();
            let (link2, _) = self.dlink(path)?;
            resp = self
                .dlink_get(&link2, None)
                .map_err(|e| anyhow!("下载失败:{e}"))?;
        }
        if !resp.status().is_success() {
            bail!(
                "下载失败:HTTP {} {}",
                resp.status().as_u16(),
                resp.text()
                    .unwrap_or_default()
                    .chars()
                    .take(200)
                    .collect::<String>()
            );
        }
        // 200 + JSON 错误体（如 31045）也要拦截，避免把错误内容当文件写入缓存。
        let content_type = resp
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if content_type.to_ascii_lowercase().contains("json") {
            bail!(
                "下载失败:{}",
                resp.text()
                    .unwrap_or_default()
                    .chars()
                    .take(200)
                    .collect::<String>()
            );
        }
        let total = resp.content_length().unwrap_or(size);
        if let Some(p) = &progress {
            p.total.store(total, Ordering::SeqCst);
        }
        let mut disk = std::fs::File::create(&file_path).context("创建缓存文件失败")?;
        let mut buf = [0u8; 64 * 1024];
        let mut written: u64 = 0;
        loop {
            let n = resp.read(&mut buf).context("读取下载流失败")?;
            if n == 0 {
                break;
            }
            disk.write_all(&buf[..n]).context("写入缓存文件失败")?;
            written += n as u64;
            if let Some(p) = &progress {
                p.downloaded.store(written, Ordering::SeqCst);
            }
        }
        disk.flush().ok();
        Ok(file_path)
    }
}

/// 百度远端文件作为 ByteSource：Range 流式读；dlink 会话级缓存 + 失效重取。
pub struct BaiduFile {
    client: Arc<BaiduClient>,
    path: String,
    len: u64,
    dlink: Mutex<Option<String>>,
}

impl BaiduFile {
    pub fn new(client: Arc<BaiduClient>, path: String, len: u64, dlink: String) -> Self {
        BaiduFile {
            client,
            path,
            len,
            dlink: Mutex::new(Some(dlink)),
        }
    }

    fn get_dlink(&self) -> io::Result<String> {
        if let Some(d) = self.dlink.lock().unwrap().clone() {
            return Ok(d);
        }
        let (d, _) = self
            .client
            .dlink(&self.path)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("取 dlink 失败:{e}")))?;
        *self.dlink.lock().unwrap() = Some(d.clone());
        Ok(d)
    }
}

impl ByteSource for BaiduFile {
    fn len(&self) -> u64 {
        self.len
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        let dlink = self.get_dlink()?;
        match self.client.read_range_with_dlink(&dlink, &self.path, offset, buf) {
            Ok(n) => Ok(n),
            Err(e) => {
                // 403 已由 read_range_with_dlink 内部重取；其它错误视为 dlink 失效，清缓存再试一次
                *self.dlink.lock().unwrap() = None;
                let d2 = self.get_dlink()?;
                self.client
                    .read_range_with_dlink(&d2, &self.path, offset, buf)
                    .map_err(|_| e)
            }
        }
    }
}

/// raw/ 缓存路径（沿用 WebDAV 的 hash 命名模式）。
pub fn raw_cache_path(origin: &str, path: &str) -> Option<PathBuf> {
    use std::hash::{Hash, Hasher};
    let name = path.rsplit('/').next().unwrap_or("file.cbz");
    let hash = {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        format!("{}{}", origin, path).hash(&mut h);
        format!("{:016x}", h.finish())
    };
    let file_path = crate::cache::CacheDir::Raw
        .ensure()
        .ok()?
        .join(&hash)
        .join(name);
    match std::fs::metadata(&file_path) {
        Ok(meta) if meta.len() > 0 => Some(file_path),
        _ => None,
    }
}

fn check_errno(errno: i64) -> Result<()> {
    match errno {
        0 => Ok(()),
        -9 => bail!("文件不存在"),
        -8 | -10 => bail!("路径或参数错误"),
        31066 => bail!("请求过于频繁，请稍后再试"),
        31119 | 31329 => bail!("百度账号状态异常（风控），请检查账号"),
        -6 | 110 => bail!("登录状态失效，请重新授权"),
        31045 => bail!("百度 access_token 验证未通过：token 可能已过期，或授权时未勾选网盘权限，请重新授权"),
        _ => bail!("百度 API 错误码:{errno}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_url_contains_required_params() {
        let u = auth_url("abc123");
        assert!(u.starts_with(API_AUTHORIZE));
        assert!(u.contains("response_type=code"));
        assert!(u.contains("client_id=abc123"));
        assert!(u.contains("redirect_uri=oob"));
        // form_urlencoded 会把逗号编码为 %2C
        assert!(u.contains("scope=basic%2Cnetdisk") || u.contains("scope=basic,netdisk"));
    }

    #[test]
    fn parse_list_resp() {
        let json = r#"{
          "errno": 0,
          "list": [
            {"fs_id": 1, "path": "/a/书.cbz", "isdir": 0, "server_filename": "书.cbz", "size": 100, "server_mtime": 1700000000},
            {"fs_id": 2, "path": "/a/文件夹", "isdir": 1, "server_filename": "文件夹", "size": 0, "server_mtime": 1700000001}
          ]
        }"#;
        let p: ListResp = serde_json::from_str(json).unwrap();
        assert_eq!(p.list.len(), 2);
        assert!(p.list[0].isdir == 0);
        assert_eq!(p.list[1].server_filename, "文件夹");
    }

    #[test]
    fn parse_filemetas_resp() {
        let json = r#"{"errno":0,"list":[{"fs_id":7,"dlink":"https://d.pcs.baidu.com/file?x=1","size":2048}]}"#;
        let p: FileMetasResp = serde_json::from_str(json).unwrap();
        assert_eq!(p.list[0].fs_id, 7);
        assert_eq!(p.list[0].size, Some(2048));
        assert!(p.list[0].dlink.as_deref().unwrap().starts_with("https://"));
    }

    #[test]
    fn dlink_token_appended_when_missing() {
        let out = dlink_with_access_token(
            "https://d.pcs.baidu.com/file/abc?fid=1&sign=a%2Bb%3D&expires=8h",
            "12.token",
        )
        .unwrap();
        let u = Url::parse(&out).unwrap();
        let pairs: Vec<(String, String)> = u
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        assert!(pairs.contains(&("access_token".to_string(), "12.token".to_string())));
        // 原有参数保留，且百分号编码往返稳定
        assert!(pairs.contains(&("sign".to_string(), "a+b=".to_string())));
        assert!(pairs.contains(&("expires".to_string(), "8h".to_string())));
    }

    #[test]
    fn dlink_token_replaced_when_present() {
        let out = dlink_with_access_token(
            "https://d.pcs.baidu.com/file/abc?fid=1&access_token=old&sign=x",
            "new",
        )
        .unwrap();
        let u = Url::parse(&out).unwrap();
        let pairs: Vec<(String, String)> = u
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        assert_eq!(
            pairs.iter().filter(|(k, _)| k == "access_token").count(),
            1
        );
        assert!(pairs.contains(&("access_token".to_string(), "new".to_string())));
    }

    #[test]
    fn parse_token_error() {
        let json = r#"{"error":"invalid_grant","error_description":"authorization code expired"}"#;
        let p: TokenResp = serde_json::from_str(json).unwrap();
        assert_eq!(p.error.as_deref(), Some("invalid_grant"));
        assert!(p.access_token.is_none());
    }

    #[test]
    fn fs_id_path_split() {
        // 直接测 dlink 路径解析逻辑（父目录 + 文件名）
        let path = "/a/b/漫画.cbz";
        let idx = path.rfind('/').unwrap();
        assert_eq!(&path[..idx], "/a/b");
        assert_eq!(&path[idx + 1..], "漫画.cbz");
    }

    #[test]
    fn raw_cache_path_uses_origin_and_path() {
        // 不依赖真实缓存目录，仅验证 None/路径推导不会 panic
        let _ = raw_cache_path("baidu:app:/", "/1.cbz");
        let _ = raw_cache_path("baidu:app:/", "/深/目录/书.cbz");
    }
}
