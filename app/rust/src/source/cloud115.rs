//! 115 网盘书源：115 生活开放平台官方 API（设备码 + PKCE 扫码授权）。
//!
//! - 鉴权：`authDeviceCode` 出二维码 → 115 APP 扫码 → `deviceCodeToToken` 换 token；
//!   刷新走 `refreshToken`（每次轮换 refresh_token，必须回写 DB）。
//! - 列目录：`GET /open/ufile/files`（按文件夹 ID，根为 0）。
//! - 下载：`POST /open/ufile/downurl` 换直链（带 UA，响应键为 fid）。
//! - 限速：AList 默认 1 r/s，这里默认 1.5 r/s 保守节流。
//!
//! 契约细节见 `.trellis/tasks/08-03-m6-netdisk-official-api/research/115-openapi-contract.md`。
use super::{ByteSource, Entry, RateGate};
use crate::source::webdav::DownloadProgress;
use anyhow::{anyhow, bail, Context, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use reqwest::blocking::Client;
use reqwest::header::{AUTHORIZATION, RANGE, USER_AGENT};
use reqwest::StatusCode;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const API_AUTH_DEVICE_CODE: &str = "https://passportapi.115.com/open/authDeviceCode";
const API_QR_STATUS: &str = "https://qrcodeapi.115.com/get/status/";
const API_CODE_TO_TOKEN: &str = "https://passportapi.115.com/open/deviceCodeToToken";
const API_REFRESH_TOKEN: &str = "https://passportapi.115.com/open/refreshToken";
const API_USER_INFO: &str = "https://proapi.115.com/open/user/info";
const API_FS_FILES: &str = "https://proapi.115.com/open/ufile/files";
const API_FS_DOWNURL: &str = "https://proapi.115.com/open/ufile/downurl";
/// 115 下载直链与取链的 UA 必须一致（115 会校验调用方 UA）。
const APP_UA: &str = "RCH/0.3 (comic reader)";
const PAGE_SIZE: i64 = 200;

/// 设备码扫码阶段的 code_verifier 暂存（uid -> verifier）。
static VERIFIERS: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();

fn verifiers() -> &'static Mutex<HashMap<String, String>> {
    VERIFIERS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 鉴权接口响应（state 为 int，code 0 表示成功）。
#[derive(Debug, Deserialize)]
struct AuthResp<T> {
    #[serde(default)]
    code: i32,
    #[serde(default)]
    message: String,
    data: Option<T>,
}

/// 业务接口响应（state 为 bool）。
#[derive(Debug, Deserialize)]
struct ApiResp<T> {
    data: Option<T>,
}

#[derive(Debug, Deserialize)]
struct DeviceCodeData {
    uid: String,
    time: i64,
    qrcode: String,
    sign: String,
}

#[derive(Debug, Deserialize)]
struct QrStatusData {
    status: i32,
}

#[derive(Debug, Deserialize)]
struct TokenData {
    access_token: String,
    refresh_token: String,
}

#[derive(Debug)]
struct FileItem {
    fid: String,
    fc: String,
    fn_: String,
    fs: u64,
    pc: String,
    upt: Option<i64>,
}

impl<'de> Deserialize<'de> for FileItem {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Raw {
            fid: String,
            fc: String,
            #[serde(rename = "fn")]
            fn_raw: String,
            #[serde(default)]
            fs: u64,
            #[serde(default)]
            pc: String,
            #[serde(default)]
            upt: Option<i64>,
        }
        let r = Raw::deserialize(d)?;
        Ok(FileItem {
            fid: r.fid,
            fc: r.fc,
            fn_: r.fn_raw,
            fs: r.fs,
            pc: r.pc,
            upt: r.upt,
        })
    }
}

#[derive(Debug, Deserialize)]
struct FilesResp {
    #[serde(default)]
    state: bool,
    #[serde(default)]
    code: i64,
    #[serde(default)]
    message: String,
    #[serde(default)]
    data: Vec<FileItem>,
    #[serde(default)]
    count: i64,
}

#[derive(Debug, Deserialize)]
struct DownUrlItem {
    #[serde(rename = "url")]
    url: DownUrlValue,
}

#[derive(Debug, Deserialize)]
struct DownUrlValue {
    url: String,
}

#[derive(Debug, Deserialize)]
struct UserInfoData {
    user_name: Option<String>,
}

/// 扫码授权返回给 Dart 的二维码载荷。
#[derive(Debug, Clone, serde::Serialize)]
pub struct QrPayload {
    pub uid: String,
    pub time: i64,
    pub sign: String,
    pub qrcode: String,
}

/// 扫码轮询结果。
#[derive(Debug, Clone, serde::Serialize)]
pub struct QrPollResult {
    pub status: i32,
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
}

/// 生成 code_verifier（43~128 位，字符集 [A-Za-z0-9-._~]；这里用 sha256 十六进制，64 位）。
fn gen_code_verifier() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut hasher = Sha256::new();
    hasher.update(nanos.to_le_bytes());
    hasher.update(format!("{:?}", std::thread::current().id()).as_bytes());
    let digest = hasher.finalize();
    digest.iter().map(|b| format!("{:02x}", b)).collect()
}

fn code_challenge(verifier: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    B64.encode(hasher.finalize())
}

fn http_client() -> Result<Client> {
    Client::builder()
        .timeout(Duration::from_secs(60))
        .connect_timeout(Duration::from_secs(10))
        .build()
        .context("创建 HTTP 客户端失败")
}

/// 开始设备码授权：生成二维码载荷（Dart 渲染二维码给 115 APP 扫）。
pub fn qr_start(app_id: &str) -> Result<QrPayload> {
    let verifier = gen_code_verifier();
    let challenge = code_challenge(&verifier);
    let client = http_client()?;
    let resp = client
        .post(API_AUTH_DEVICE_CODE)
        .form(&[
            ("client_id", app_id),
            ("code_challenge", &challenge),
            ("code_challenge_method", "sha256"),
        ])
        .send()
        .context("请求 115 设备码失败")?;
    let status = resp.status();
    let body: AuthResp<DeviceCodeData> =
        resp.json().context("解析 115 设备码响应失败")?;
    if !status.is_success() || body.code != 0 {
        bail!("获取 115 二维码失败:{} {}", body.code, body.message);
    }
    let data = body
        .data
        .ok_or_else(|| anyhow!("115 设备码响应缺少 data"))?;
    verifiers()
        .lock()
        .unwrap()
        .insert(data.uid.clone(), verifier);
    Ok(QrPayload {
        uid: data.uid,
        time: data.time,
        sign: data.sign,
        qrcode: data.qrcode,
    })
}

/// 轮询扫码状态；status=2 时自动换 token。
pub fn qr_poll(uid: &str, time: i64, sign: &str) -> Result<QrPollResult> {
    let client = http_client()?;
    let resp = client
        .get(API_QR_STATUS)
        .query(&[
            ("uid", uid.to_string()),
            ("time", time.to_string()),
            ("sign", sign.to_string()),
        ])
        .send()
        .context("查询 115 扫码状态失败")?;
    let body: AuthResp<QrStatusData> = resp.json().context("解析扫码状态失败")?;
    let status = body.data.map(|d| d.status).unwrap_or(0);
    if status != 2 {
        return Ok(QrPollResult {
            status,
            access_token: None,
            refresh_token: None,
        });
    }
    let verifier = verifiers().lock().unwrap().remove(uid);
    let verifier = verifier.ok_or_else(|| anyhow!("扫码会话已过期，请重新获取二维码"))?;
    let token_resp = client
        .post(API_CODE_TO_TOKEN)
        .form(&[("uid", uid.to_string()), ("code_verifier", verifier)])
        .send()
        .context("换取 115 token 失败")?;
    let tbody: AuthResp<TokenData> =
        token_resp.json().context("解析 token 响应失败")?;
    if tbody.code != 0 {
        bail!("换取 token 失败:{} {}", tbody.code, tbody.message);
    }
    let t = tbody
        .data
        .ok_or_else(|| anyhow!("token 响应缺少 data"))?;
    Ok(QrPollResult {
        status,
        access_token: Some(t.access_token),
        refresh_token: Some(t.refresh_token),
    })
}

/// 115 网盘客户端。
pub struct Cloud115Client {
    client: Client,
    app_id: String,
    refresh_token: Mutex<String>,
    access: Mutex<Option<String>>,
    root_id: String,
    gate: RateGate,
}

impl Cloud115Client {
    pub fn new(app_id: &str, refresh_token: &str, root_id: &str) -> Result<Self> {
        Ok(Cloud115Client {
            client: http_client()?,
            app_id: app_id.to_string(),
            refresh_token: Mutex::new(refresh_token.to_string()),
            access: Mutex::new(None),
            root_id: if root_id.is_empty() { "0".to_string() } else { root_id.to_string() },
            gate: RateGate::new(1.5),
        })
    }

    /// 缓存命名空间前缀。
    pub fn origin(&self) -> String {
        format!("115:{}:{}", self.app_id, self.root_id)
    }

    pub fn root_id(&self) -> &str {
        &self.root_id
    }

    /// 刷新 token（115 会轮换 refresh_token），返回新对供回写。
    pub fn refresh(&self) -> Result<(String, String)> {
        let resp = self
            .client
            .post(API_REFRESH_TOKEN)
            .form(&[("refresh_token", &self.refresh_token)])
            .send()
            .context("刷新 115 token 失败")?;
        let body: AuthResp<TokenData> = resp.json().context("解析刷新响应失败")?;
        if body.code != 0 {
            bail!("刷新 token 失败:{} {}", body.code, body.message);
        }
        let t = body
            .data
            .ok_or_else(|| anyhow!("刷新响应缺少 data"))?;
        *self.refresh_token.lock().unwrap() = t.refresh_token.clone();
        *self.access.lock().unwrap() = Some(t.access_token.clone());
        Ok((t.access_token, t.refresh_token))
    }

    fn ensure_token(&self) -> Result<String> {
        if let Some(t) = self.access.lock().unwrap().clone() {
            return Ok(t);
        }
        let (at, _) = self.refresh()?;
        Ok(at)
    }

    /// 统一业务请求：带 Authorization（115 SDK 用 resty SetAuthToken，等价于直接放 token；Bearer 前缀以实测为准）。
    /// state=false 且 code==99 或 401 开头 → 自动刷新重试一次。
    fn api_call(
        &self,
        url: &str,
        method: reqwest::Method,
        form: &[(&str, String)],
        query: &[(&str, String)],
    ) -> Result<(i64, String)> {
        let mut attempts = 0;
        loop {
            self.gate.wait();
            let token = self.ensure_token()?;
            let mut req = self
                .client
                .request(method.clone(), url)
                .header(USER_AGENT, APP_UA)
                .header(AUTHORIZATION, token);
            if !form.is_empty() {
                req = req.form(form);
            }
            if !query.is_empty() {
                req = req.query(query);
            }
            let resp = req.send().context("115 API 请求失败")?;
            let status = resp.status();
            let body = resp.text().unwrap_or_default();
            if !status.is_success() {
                bail!(
                    "115 API HTTP {}: {}",
                    status.as_u16(),
                    body.chars().take(200).collect::<String>()
                );
            }
            let parsed: serde_json::Value =
                serde_json::from_str(&body).context("解析 115 API 响应失败")?;
            let code = parsed.get("code").and_then(|v| v.as_i64()).unwrap_or(0);
            let state = parsed.get("state").cloned().unwrap_or(serde_json::Value::Null);
            let need_refresh = code == 99
                || (40100..40200).contains(&code)
                || (401000..402000).contains(&code);
            if need_refresh && attempts == 0 {
                attempts += 1;
                self.access.lock().unwrap().take();
                continue;
            }
            // 兼容 state 为 bool 或 int
            let state_ok = match state {
                serde_json::Value::Bool(b) => b,
                serde_json::Value::Number(n) => n.as_i64() == Some(1),
                _ => true,
            };
            if !state_ok || code != 0 {
                bail!(
                    "115 API 错误:code={} {}",
                    code,
                    parsed.get("message").cloned().unwrap_or_default()
                );
            }
            return Ok((code, body));
        }
    }

    /// 列目录（按文件夹 ID 分页拉全，目录在前自然排序）。
    pub fn list(&self, cid: &str) -> Result<Vec<Entry>> {
        let mut all = Vec::new();
        let mut offset = 0i64;
        loop {
            let (_, body) = self.api_call(
                API_FS_FILES,
                reqwest::Method::GET,
                &[],
                &[
                    ("cid", cid.to_string()),
                    ("limit", PAGE_SIZE.to_string()),
                    ("offset", offset.to_string()),
                    ("asc", "1".to_string()),
                    ("o", "file_name".to_string()),
                    ("show_dir", "1".to_string()),
                ],
            )?;
            let parsed: FilesResp =
                serde_json::from_str(&body).context("解析 115 文件列表失败")?;
            if !parsed.state || parsed.code != 0 {
                bail!("115 列目录失败:code={} {}", parsed.code, parsed.message);
            }
            let n = parsed.data.len();
            all.extend(parsed.data.into_iter().map(|it| {
                let is_dir = it.fc == "0";
                // 文件用提取码 pc 作为浏览/打开路径（downurl 直接可用，且响应无需按键取）；
                // 目录用 fid（列表下一级需要 cid=fid）。
                let path = if is_dir || it.pc.is_empty() {
                    it.fid
                } else {
                    it.pc
                };
                Entry {
                    name: it.fn_,
                    path,
                    is_dir,
                    size: it.fs,
                    mtime: it.upt.unwrap_or(0),
                }
            }));
            if parsed.count <= offset + n as i64 {
                break;
            }
            offset += PAGE_SIZE;
        }
        all.sort_by(|a, b| {
            b.is_dir
                .cmp(&a.is_dir)
                .then_with(|| crate::util::natural_cmp(&a.name, &b.name))
        });
        Ok(all)
    }

    /// 取下载直链（pick_code 换 url；单文件请求响应只有一项，直接取第一个值）。
    pub fn downurl(&self, pick_code: &str) -> Result<(String, u64)> {
        let (_, body) = self.api_call(
            API_FS_DOWNURL,
            reqwest::Method::POST,
            &[("pick_code", pick_code.to_string())],
            &[],
        )?;
        let parsed: ApiResp<HashMap<String, DownUrlItem>> =
            serde_json::from_str(&body).context("解析 downurl 响应失败")?;
        let map = parsed.data.ok_or_else(|| anyhow!("downurl 响应缺少 data"))?;
        let item = map
            .values()
            .next()
            .ok_or_else(|| anyhow!("downurl 响应为空"))?;
        Ok((item.url.url.clone(), 0))
    }

    /// 探测直链是否支持 Range。
    pub fn probe_range(&self, url: &str) -> bool {
        self.client
            .get(url)
            .header(RANGE, "bytes=0-0")
            .header(USER_AGENT, APP_UA)
            .send()
            .map(|r| r.status() == StatusCode::PARTIAL_CONTENT)
            .unwrap_or(false)
    }

    /// 探测直链 Range 支持并返回总大小（bytes=0-0 → 206 + Content-Range: bytes 0-0/{total}）。
    pub fn probe_size(&self, url: &str) -> Option<u64> {
        let resp = self
            .client
            .get(url)
            .header(RANGE, "bytes=0-0")
            .header(USER_AGENT, APP_UA)
            .send()
            .ok()?;
        if resp.status() != StatusCode::PARTIAL_CONTENT {
            return None;
        }
        let total = resp
            .headers()
            .get(reqwest::header::CONTENT_RANGE)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.rsplit('/').next())
            .and_then(|x| x.trim().parse::<u64>().ok());
        if total.is_some() {
            return total;
        }
        resp.headers()
            .get(reqwest::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok())
            .filter(|n| *n > 1) // 0-0 响应 Content-Length 通常为 1，不能当总大小
    }

    /// Range 读（带 UA）；403 时由调用方重取直链。
    pub fn read_range_url(&self, url: &str, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let end = offset + buf.len() as u64 - 1;
        let mut resp = self
            .client
            .get(url)
            .header(RANGE, format!("bytes={}-{}", offset, end))
            .header(USER_AGENT, APP_UA)
            .send()
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("Range 请求失败:{e}")))?;
        if resp.status() == StatusCode::FORBIDDEN {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "115 直链失效，请重试",
            ));
        }
        if resp.status() != StatusCode::PARTIAL_CONTENT {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                format!("115 直链未支持 Range(HTTP {})", resp.status().as_u16()),
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

    /// 连通性测试：取用户信息。
    pub fn user_info(&self) -> Result<String> {
        let (_, body) = self.api_call(API_USER_INFO, reqwest::Method::GET, &[], &[])?;
        let parsed: ApiResp<UserInfoData> =
            serde_json::from_str(&body).context("解析用户信息失败")?;
        let name = parsed
            .data
            .and_then(|d| d.user_name)
            .unwrap_or_else(|| "115 用户".to_string());
        Ok(name)
    }

    /// 整本下载到 raw/ 缓存（有进度）；已缓存则复用。
    pub fn download_to_raw_cache(
        &self,
        pick_code: &str,
        path: &str,
        progress: Option<Arc<DownloadProgress>>,
    ) -> Result<PathBuf> {
        if let Some(p) = raw_cache_path(&self.origin(), path) {
            if let Some(prog) = &progress {
                if let Ok(meta) = std::fs::metadata(&p) {
                    prog.downloaded.store(meta.len(), Ordering::SeqCst);
                    prog.total.store(meta.len(), Ordering::SeqCst);
                }
            }
            return Ok(p);
        }
        let (url, _) = self.downurl(pick_code)?;
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
            .client
            .get(&url)
            .header(USER_AGENT, APP_UA)
            .send()
            .map_err(|e| anyhow!("下载失败:{e}"))?;
        if resp.status() == StatusCode::FORBIDDEN {
            let (url2, _) = self.downurl(pick_code)?;
            resp = self
                .client
                .get(&url2)
                .header(USER_AGENT, APP_UA)
                .send()
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
        let total = resp.content_length().unwrap_or(0);
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

/// 115 远端文件作为 ByteSource：Range 流式读；直链失效重取一次。
pub struct Cloud115File {
    client: Arc<Cloud115Client>,
    pick_code: String,
    len: u64,
    url: Mutex<Option<String>>,
}

impl Cloud115File {
    pub fn new(
        client: Arc<Cloud115Client>,
        pick_code: String,
        len: u64,
        url: String,
    ) -> Self {
        Cloud115File {
            client,
            pick_code,
            len,
            url: Mutex::new(Some(url)),
        }
    }

    fn get_url(&self) -> io::Result<String> {
        if let Some(u) = self.url.lock().unwrap().clone() {
            return Ok(u);
        }
        let (u, _) = self
            .client
            .downurl(&self.pick_code)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("取直链失败:{e}")))?;
        *self.url.lock().unwrap() = Some(u.clone());
        Ok(u)
    }
}

impl ByteSource for Cloud115File {
    fn len(&self) -> u64 {
        self.len
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        let url = self.get_url()?;
        match self.client.read_range_url(&url, offset, buf) {
            Ok(n) => Ok(n),
            Err(e) => {
                *self.url.lock().unwrap() = None;
                let u2 = self.get_url()?;
                self.client
                    .read_range_url(&u2, offset, buf)
                    .map_err(|_| e)
            }
        }
    }
}

/// raw/ 缓存路径。
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_challenge_sha256_base64() {
        let verifier = "abcdefghijklmnopqrstuvwxyz0123456789abcdefghijklmnopqrstuvwxyz0123456789";
        let challenge = code_challenge(verifier);
        // RFC 7636 兼容：base64(sha256(verifier))
        let mut hasher = Sha256::new();
        hasher.update(verifier.as_bytes());
        let expect = B64.encode(hasher.finalize());
        assert_eq!(challenge, expect);
    }

    #[test]
    fn verifier_length_and_charset() {
        let v = gen_code_verifier();
        assert!((43..=128).contains(&v.len()));
        assert!(v
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.' || c == '_' || c == '~'));
    }

    #[test]
    fn parse_files_resp() {
        let json = r#"{
          "state": true, "code": 0, "message": "",
          "data": [
            {"fid": "100", "fc": "0", "fn": "文件夹", "fs": 0, "pc": "", "upt": 1700000000},
            {"fid": "101", "fc": "1", "fn": "漫画.cbz", "fs": 12345, "pc": "abc", "upt": 1700000001}
          ],
          "count": 2
        }"#;
        let p: FilesResp = serde_json::from_str(json).unwrap();
        assert!(p.state);
        assert_eq!(p.count, 2);
        assert_eq!(p.data.len(), 2);
        assert_eq!(p.data[0].fn_, "文件夹");
        assert_eq!(p.data[1].pc, "abc");
        assert_eq!(p.data[1].fs, 12345);
    }

    #[test]
    fn parse_downurl_resp() {
        let json = r#"{"state":true,"code":0,"message":"","data":{"101":{"file_name":"a.cbz","url":{"url":"https://cdn.example.com/a.cbz"},"pick_code":"abc"}}}"#;
        let p: ApiResp<HashMap<String, DownUrlItem>> = serde_json::from_str(json).unwrap();
        let map = p.data.unwrap();
        assert_eq!(
            map.get("101").unwrap().url.url,
            "https://cdn.example.com/a.cbz"
        );
    }

    #[test]
    fn parse_auth_device_code() {
        let json = r#"{"state":1,"code":0,"message":"","data":{"uid":"u1","time":1700000000,"qrcode":"qr://xx","sign":"s1"},"error":"","errno":0}"#;
        let p: AuthResp<DeviceCodeData> = serde_json::from_str(json).unwrap();
        assert_eq!(p.code, 0);
        assert_eq!(p.data.unwrap().qrcode, "qr://xx");
    }

    #[test]
    fn raw_cache_path_smoke() {
        let _ = raw_cache_path("115:app:0", "/101");
        let _ = raw_cache_path("115:app:0", "/101/深");
    }
}
