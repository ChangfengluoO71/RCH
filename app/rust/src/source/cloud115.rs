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
use reqwest::header::{AUTHORIZATION, COOKIE, RANGE, USER_AGENT};
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
// ---- 网页版扫码（Cookie 模式，无需 APP ID） ----
const API_WEB_QR_TOKEN: &str = "https://qrcodeapi.115.com/api/1.0/web/1.0/token/";
const API_WEB_QR_LOGIN: &str = "https://passportapi.115.com/app/1.0/{app}/1.0/login/qrcode/";
const WEB_API_FILES: &str = "https://webapi.115.com/files";
const WEB_API_FILES_HTTP: &str = "http://web.api.115.com/files";
const WEB_API_NATSORT: &str = "https://aps.115.com/natsort/files.php";
const WEB_API_DOWNURL: &str = "https://proapi.115.com/app/chrome/downurl";
/// 网页接口与 CDN 直链必须带浏览器 UA + Cookie，否则 403/风控。
const WEB_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
(KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";
const WEB_PAGE_SIZE: i64 = 200;
/// 直链取链/下载必须使用完全一致的请求头，否则 CDN 返回 403 "invalid signature"。
/// 与 p115client 一致：UA 显式置空，Referer 为取链接口源站。
const WEB_DOWNLOAD_UA: &str = "";
const WEB_DOWNLOAD_REFERER: &str = "https://proapi.115.com";
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

// ============================================================
// Web 扫码（Cookie 模式）：无需 APP ID，115 App 扫码即可
// ============================================================

/// 115 网页扫码二维码载荷（uid/time/sign 用于轮询，qrcode 用于渲染）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct WebQrPayload {
    pub uid: String,
    pub time: i64,
    pub sign: String,
    pub qrcode: String,
}

/// 可用的扫码设备。Windows/Mac/Linux 客户端已下架不可用；
/// 选不常用设备可避免挤掉网页端/App 的旧登录。
pub const WEB_QR_APPS: &[&str] = &[
    "web", "android", "ios", "tv", "alipaymini", "wechatmini", "qandroid",
];

/// 第一步：获取 115 网页登录二维码（无需 APP ID）。
pub fn web_qr_start() -> Result<WebQrPayload> {
    let client = http_client()?;
    let resp = client
        .get(API_WEB_QR_TOKEN)
        .send()
        .context("请求 115 二维码失败")?;
    let status = resp.status();
    let body: AuthResp<DeviceCodeData> = resp.json().context("解析 115 二维码响应失败")?;
    if !status.is_success() || body.code != 0 {
        bail!("获取 115 二维码失败:{} {}", body.code, body.message);
    }
    let data = body
        .data
        .ok_or_else(|| anyhow!("115 二维码响应缺少 data"))?;
    Ok(WebQrPayload {
        uid: data.uid,
        time: data.time,
        sign: data.sign,
        qrcode: data.qrcode,
    })
}

/// 第二步：轮询扫码状态。0 等待 / 1 已扫 / 2 已登录 / -1 过期 / -2 取消。
pub fn web_qr_poll(uid: &str, time: i64, sign: &str) -> Result<i32> {
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
    let body: AuthResp<QrStatusData> = resp.json().context("解析 115 扫码状态失败")?;
    Ok(body.data.map(|d| d.status).unwrap_or(0))
}

/// 第三步：扫码成功后换取 Cookie（`k=v; k2=v2`，末尾不带 `;`）。
pub fn web_qr_cookie(uid: &str, app: &str) -> Result<String> {
    if !WEB_QR_APPS.contains(&app) {
        bail!(
            "不支持的 115 扫码设备:{app}，可选 {}",
            WEB_QR_APPS.join("/")
        );
    }
    let client = http_client()?;
    let url = API_WEB_QR_LOGIN.replace("{app}", app);
    let resp = client
        .post(&url)
        .form(&[("app", app), ("account", uid)])
        .send()
        .context("换取 115 Cookie 失败")?;
    let status = resp.status();
    let body: serde_json::Value = resp.json().context("解析 115 Cookie 响应失败")?;
    if !status.is_success() {
        bail!("换取 115 Cookie 失败:HTTP {}", status.as_u16());
    }
    let code = body.get("code").and_then(|v| v.as_i64()).unwrap_or(0);
    let message = body
        .get("message")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    if code != 0 {
        bail!("换取 115 Cookie 失败:{code} {message}");
    }
    let cookie_map = body
        .get("data")
        .and_then(|d| d.get("cookie"))
        .and_then(|c| c.as_object())
        .ok_or_else(|| anyhow!("115 Cookie 响应缺少 data.cookie"))?;
    let parts: Vec<String> = cookie_map
        .iter()
        .map(|(k, v)| format!("{k}={}", v.as_str().unwrap_or_default()))
        .collect();
    if parts.is_empty() {
        bail!("115 扫码返回的 Cookie 为空，请重新扫码");
    }
    Ok(parts.join("; "))
}

// ============================================================
// 115 直链加密（proapi chrome/downurl 专用）
// 与 p115client/p115cipher（当前活跃维护）保持一致：
// 数据 XOR 固定 4 字节 key → 字节反转 → XOR 12 字节 client key →
// 前置 16 字节全 0 随机串 → RSA(固定公钥, e=65537) 分块加密。
// 响应同理用固定 key 还原（服务器返回的 data 即本方案密文）。
// ============================================================

const M115_N_HEX: &str = "8686980c0f5a24c4b9d43020cd2c22703ff3f450756529058b1cf88f09b86021\
36477198a6e2683149659bd122c33592fdb5ad47944ad1ea4d36c6b172aad633\
8c3bb6ac6227502d010993ac967d1aef00f0c8e038de2e4d3bc2ec368af2e9f1\
0a6f1eda4f7262f136420c07c331b871bf139f74f3010e3c4fe57df3afb71683";

const M115_XOR_KEY_SEED: [u8; 144] = [
    0xf0, 0xe5, 0x69, 0xae, 0xbf, 0xdc, 0xbf, 0x8a, 0x1a, 0x45, 0xe8, 0xbe, 0x7d, 0xa6, 0x73,
    0xb8, 0xde, 0x8f, 0xe7, 0xc4, 0x45, 0xda, 0x86, 0xc4, 0x9b, 0x64, 0x8b, 0x14, 0x6a, 0xb4,
    0xf1, 0xaa, 0x38, 0x01, 0x35, 0x9e, 0x26, 0x69, 0x2c, 0x86, 0x00, 0x6b, 0x4f, 0xa5, 0x36,
    0x34, 0x62, 0xa6, 0x2a, 0x96, 0x68, 0x18, 0xf2, 0x4a, 0xfd, 0xbd, 0x6b, 0x97, 0x8f, 0x4d,
    0x8f, 0x89, 0x13, 0xb7, 0x6c, 0x8e, 0x93, 0xed, 0x0e, 0x0d, 0x48, 0x3e, 0xd7, 0x2f, 0x88,
    0xd8, 0xfe, 0xfe, 0x7e, 0x86, 0x50, 0x95, 0x4f, 0xd1, 0xeb, 0x83, 0x26, 0x34, 0xdb, 0x66,
    0x7b, 0x9c, 0x7e, 0x9d, 0x7a, 0x81, 0x32, 0xea, 0xb6, 0x33, 0xde, 0x3a, 0xa9, 0x59, 0x34,
    0x66, 0x3b, 0xaa, 0xba, 0x81, 0x60, 0x48, 0xb9, 0xd5, 0x81, 0x9c, 0xf8, 0x6c, 0x84, 0x77,
    0xff, 0x54, 0x78, 0x26, 0x5f, 0xbe, 0xe8, 0x1e, 0x36, 0x9f, 0x34, 0x80, 0x5c, 0x45, 0x2c,
    0x9b, 0x76, 0xd5, 0x1b, 0x8f, 0xcc, 0xc3, 0xb8, 0xf5,
];

const M115_XOR_CLIENT_KEY: [u8; 12] = [
    0x78, 0x06, 0xad, 0x4c, 0x33, 0x86, 0x5d, 0x18, 0x4c, 0x01, 0x3f, 0x46,
];

/// 数据区固定 XOR key（p115cipher.RSA_KEY）。
const M115_FIXED_KEY: [u8; 4] = [0x8d, 0xa5, 0xa5, 0x8d];

fn m115_xor_derive_key(seed: &[u8], size: usize) -> Vec<u8> {
    let mut key = vec![0u8; size];
    for i in 0..size {
        key[i] = seed[i].wrapping_add(M115_XOR_KEY_SEED[size * i]);
        key[i] ^= M115_XOR_KEY_SEED[size * (size - i - 1)];
    }
    key
}

fn m115_xor_transform(data: &mut [u8], key: &[u8]) {
    let (data_size, key_size) = (data.len(), key.len());
    let m = data_size % 4;
    if m > 0 {
        for i in 0..m {
            data[i] ^= key[i % key_size];
        }
    }
    for i in m..data_size {
        data[i] ^= key[(i - m) % key_size];
    }
}

/// RSA 加密（公钥 e=65537，PKCS#1 v1.5 type2 风格填充，按 128 字节分块）。
/// `pad` 非空时用其字节生成填充（测试向量用），否则使用随机填充。
fn m115_rsa_encrypt(input: &[u8], pad: &[u8]) -> Vec<u8> {
    use num_bigint::BigUint;
    let n = BigUint::parse_bytes(M115_N_HEX.as_bytes(), 16).expect("m115 modulus");
    let e = BigUint::from(0x10001u32);
    let key_len = ((n.bits() as usize) + 7) / 8; // 1024 位 → 128 字节
    let mut out = Vec::new();
    let mut remain = input;
    let mut pad_idx = 0usize;
    while !remain.is_empty() {
        let slice_size = (key_len - 11).min(remain.len());
        let slice = &remain[..slice_size];
        remain = &remain[slice_size..];
        let pad_size = key_len - slice.len() - 3;
        let mut msg = vec![0u8; key_len];
        msg[0] = 0;
        msg[1] = 2;
        for b in msg.iter_mut().take(2 + pad_size).skip(2) {
            *b = if pad.is_empty() {
                rand::random::<u8>() % 0xff + 1 // 1..=255，非零填充
            } else {
                let v = pad[pad_idx % pad.len()];
                pad_idx += 1;
                v % 0xff + 1
            };
        }
        msg[2 + pad_size] = 0;
        msg[pad_size + 3..].copy_from_slice(slice);
        let m = BigUint::from_bytes_be(&msg);
        let c = m.modpow(&e, &n);
        let bytes = c.to_bytes_be();
        out.extend(std::iter::repeat(0).take(key_len - bytes.len()));
        out.extend(bytes);
    }
    out
}

/// 与 115driver 的 rsaDecrypt 一致：对密文分块做同公钥变换，
/// 每块取第一个 `0x00`（位置非 0）之后的明文（跳过 PKCS#1 填充头）。
fn m115_rsa_transform(input: &[u8]) -> Vec<u8> {
    use num_bigint::BigUint;
    let n = BigUint::parse_bytes(M115_N_HEX.as_bytes(), 16).expect("m115 modulus");
    let e = BigUint::from(0x10001u32);
    let key_len = ((n.bits() as usize) + 7) / 8; // 1024 位 → 128 字节
    let mut out = Vec::new();
    for chunk in input.chunks(key_len) {
        let m = BigUint::from_bytes_be(chunk);
        let ret = m.modpow(&e, &n).to_bytes_be();
        for (i, b) in ret.iter().enumerate() {
            if *b == 0 && i != 0 {
                out.extend_from_slice(&ret[i + 1..]);
                break;
            }
        }
    }
    out
}

/// 加密直链请求体核心：固定 key XOR + 字节反转 + RSA 分块。
/// `pad` 非空时用其字节生成填充（测试向量用），否则使用随机填充。
fn m115_encode_with(json: &str, pad: &[u8]) -> String {
    let mut buf = Vec::with_capacity(16 + json.len());
    buf.extend_from_slice(&[0u8; 16]); // 随机串固定全 0（与 p115client 一致）
    let mut body = json.as_bytes().to_vec();
    m115_xor_transform(&mut body, &M115_FIXED_KEY);
    body.reverse();
    m115_xor_transform(&mut body, &M115_XOR_CLIENT_KEY);
    buf.extend_from_slice(&body);
    B64.encode(m115_rsa_encrypt(&buf, pad))
}

/// 加密直链请求体，返回 base64。
fn m115_encode(json: &str) -> String {
    m115_encode_with(json, &[])
}

/// 解密直链响应（固定 key 还原）。
fn m115_decode(b64: &str) -> Result<Vec<u8>> {
    let data = B64.decode(b64).context("解码 115 直链响应失败")?;
    let raw = m115_rsa_transform(&data);
    if raw.len() <= 16 {
        bail!("115 直链响应解密失败（数据过短）");
    }
    let k = &raw[..16]; // 响应携带的随机串，派生 12 字节 key
    let mut out = raw[16..].to_vec();
    m115_xor_transform(&mut out, &m115_xor_derive_key(k, 12));
    out.reverse();
    m115_xor_transform(&mut out, &M115_FIXED_KEY);
    Ok(out)
}

// ============================================================
// Cloud115WebClient：Cookie 模式（webapi 列表 + proapi 直链）
// ============================================================

/// 115 网页接口错误码 → 用户可读文案。
fn map_115_web_errno(errno: i64, message: &str) -> String {
    match errno {
        990001 | 40101032 | 40101033 => {
            "115 登录状态已失效（Cookie 过期或被顶下线），请重新扫码获取 Cookie".to_string()
        }
        990002 => "115 请求过于频繁，请稍后再试".to_string(),
        990004 => "115 账号被风控，请稍后重试或更换网络".to_string(),
        // 同族"文件不存在或已删除"（p115client 同款契约）。
        20013 | 20018 | 31003 | 50015 | 70005 | 70008 | 90008 | 430004 => {
            "115 文件不存在或已删除（可能已被移动或重新上传，请重新浏览文件夹打开）".to_string()
        }
        20004 => "115 目录名称已存在".to_string(),
        20009 => "115 父目录不存在".to_string(),
        50003 => "115 文件提取码不存在".to_string(),
        50028 => "115 文件大小超出限制，请使用 115 电脑端下载".to_string(),
        50038 => "115 下载失败（含违规内容）".to_string(),
        51011 => "115 不允许转存空文件夹".to_string(),
        51012 => "115 已有文件正在解压中，请稍后再试".to_string(),
        _ => {
            if message.is_empty() {
                format!("115 网页接口错误({errno})")
            } else {
                format!("115 网页接口错误({errno}): {message}")
            }
        }
    }
}

/// 从 Cookie 中提取 user_id：`UID=1234567890_xxxx` 取 `_` 前的数字。
/// chrome/downurl 的 payload 需要 user_id（p115client 同款）。
fn user_id_from_cookie(cookie: &str) -> Option<String> {
    for part in cookie.split(';') {
        let p = part.trim();
        if let Some(v) = p.strip_prefix("UID=") {
            let id = v.split('_').next().unwrap_or("");
            if !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()) {
                return Some(id.to_string());
            }
        }
    }
    None
}

/// 解析 115 网页文件项。目录项 fid 为空（用 cid 进入下一层），
/// 文件项用 `pc`(pickcode) 取直链；`s` 可能为数字或字符串。
#[derive(Debug, Deserialize)]
struct WebFileItem {
    #[serde(default)]
    fid: String,
    #[serde(default)]
    cid: String,
    #[serde(default, rename = "n")]
    name: String,
    #[serde(default)]
    s: StringInt,
    #[serde(default)]
    t: String,
    #[serde(default, rename = "pc")]
    pc: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(untagged)]
enum StringInt {
    S(String),
    I(i64),
    #[default]
    Empty,
}

impl StringInt {
    fn as_u64(&self) -> u64 {
        match self {
            StringInt::S(s) => s.parse().unwrap_or(0),
            StringInt::I(i) => (*i).max(0) as u64,
            StringInt::Empty => 0,
        }
    }
}

#[derive(Debug, Deserialize)]
struct WebFilesResp {
    #[serde(default)]
    state: bool,
    #[serde(default)]
    code: i64,
    #[serde(default)]
    message: String,
    #[serde(default)]
    data: Vec<WebFileItem>,
    #[serde(default)]
    count: i64,
    #[serde(default, alias = "errNo")]
    errno: i64,
}

/// 115 网页客户端（Cookie 认证，与 AList/p115client 同款契约）。
pub struct Cloud115WebClient {
    client: Client,
    cookie: Mutex<String>,
    root: String,
    gate: RateGate,
    /// pick_code -> 真实文件名（列表时缓存；下载/封面/历史打开都需要）。
    names: Mutex<HashMap<String, String>>,
}

impl Cloud115WebClient {
    pub fn new(cookie: &str, root: &str) -> Result<Self> {
        Ok(Cloud115WebClient {
            client: http_client()?,
            cookie: Mutex::new(cookie.trim().to_string()),
            root: if root.is_empty() { "0".to_string() } else { root.to_string() },
            gate: RateGate::new(1.5),
            names: Mutex::new(HashMap::new()),
        })
    }

    /// 缓存命名空间前缀：`115web:{root}`（Cookie 会变，只用 root 保持稳定）。
    pub fn origin(&self) -> String {
        format!("115web:{}", self.root)
    }

    pub fn root(&self) -> &str {
        &self.root
    }

    fn current_cookie(&self) -> String {
        self.cookie.lock().unwrap().clone()
    }

    /// 当前会话 Cookie（供 Dart 侧回写 DB）。
    pub fn cookie(&self) -> String {
        self.current_cookie()
    }

    /// 统一 GET：带 Cookie + 浏览器 UA；返回 (HTTP 状态码, 文本)。
    fn get_with_cookie(&self, url: &str, query: &[(&str, String)]) -> Result<(u16, String)> {
        self.gate.wait();
        let resp = self
            .client
            .get(url)
            .query(query)
            .header(COOKIE, self.current_cookie())
            .header(USER_AGENT, WEB_UA)
            .send()
            .context("115 网页接口请求失败")?;
        let status = resp.status().as_u16();
        let text = resp.text().unwrap_or_default();
        Ok((status, text))
    }

    /// 连通性测试：列表根目录成功即视为可用（登录失效会在这里暴露）。
    pub fn check(&self) -> Result<()> {
        let _ = self.list(&self.root)?;
        Ok(())
    }

    /// 列表一页（多域名 fallback：webapi 可能被 WAF 405，换备用域名）。
    fn list_page(&self, cid: &str, offset: i64) -> Result<WebFilesResp> {
        let query: Vec<(&str, String)> = vec![
            ("aid", "1".to_string()),
            ("cid", cid.to_string()),
            ("o", "user_ptime".to_string()),
            ("asc", "0".to_string()),
            ("offset", offset.to_string()),
            ("show_dir", "1".to_string()),
            ("limit", WEB_PAGE_SIZE.to_string()),
            ("snap", "0".to_string()),
            ("natsort", "0".to_string()),
            ("record_open_time", "1".to_string()),
            ("format", "json".to_string()),
            ("fc_mix", "0".to_string()),
        ];
        let mut last_err: Option<anyhow::Error> = None;
        for url in [WEB_API_FILES, WEB_API_FILES_HTTP, WEB_API_NATSORT] {
            let (status, text) = match self.get_with_cookie(url, &query) {
                Ok(v) => v,
                Err(e) => {
                    last_err = Some(e);
                    continue;
                }
            };
            if status == 405 {
                last_err = Some(anyhow!(
                    "115 列表接口被风控拦截(HTTP 405)，已尝试备用接口"
                ));
                continue;
            }
            if !(200..300).contains(&status) {
                last_err = Some(anyhow!(
                    "115 列表接口 HTTP {status}: {}",
                    text.chars().take(200).collect::<String>()
                ));
                continue;
            }
            match serde_json::from_str::<WebFilesResp>(&text) {
                Ok(parsed) => {
                    if !parsed.state || parsed.code != 0 {
                        bail!(map_115_web_errno(
                            parsed.errno.max(parsed.code),
                            &parsed.message
                        ));
                    }
                    return Ok(parsed);
                }
                Err(e) => {
                    last_err = Some(anyhow!("解析 115 列表响应失败:{e}"));
                    continue;
                }
            }
        }
        Err(last_err.unwrap_or_else(|| anyhow!("115 列表接口全部不可用")))
    }

    /// 列出目录（path 为文件夹 ID，根目录 `0`），分页拉全，目录在前自然排序。
    pub fn list(&self, cid: &str) -> Result<Vec<Entry>> {
        let mut all = Vec::new();
        let mut offset = 0i64;
        loop {
            let parsed = self.list_page(cid, offset)?;
            let n = parsed.data.len();
            for it in parsed.data.into_iter() {
                let is_dir = it.fid.is_empty();
                let path = if is_dir { it.cid } else { it.pc };
                if path.is_empty() {
                    // 115 偶发返回空 pickcode/cid 的条目（无法浏览/下载），
                    // 直接跳过，避免空路径在下游（如 .zip 扩展名处理）触发越界。
                    tracing::warn!("115 列表返回空路径条目，已跳过: {}", it.name);
                    continue;
                }
                if !is_dir && !it.name.is_empty() {
                    self.names
                        .lock()
                        .unwrap()
                        .insert(path.clone(), it.name.clone());
                }
                all.push(Entry {
                    name: it.name,
                    path,
                    is_dir,
                    size: it.s.as_u64(),
                    mtime: parse_115_time(&it.t),
                });
            }
            if parsed.count <= offset + n as i64 {
                break;
            }
            offset += WEB_PAGE_SIZE;
        }
        all.sort_by(|a, b| {
            b.is_dir
                .cmp(&a.is_dir)
                .then_with(|| crate::util::natural_cmp(&a.name, &b.name))
        });
        Ok(all)
    }

    /// 取下载直链（chrome/downurl，无 200MB 上限；需 m115 加密）。
    pub fn downurl(&self, pick_code: &str) -> Result<super::quark::DownloadInfo> {
        let mut payload = serde_json::Map::new();
        payload.insert("pickcode".to_string(), pick_code.into());
        if let Some(uid) = user_id_from_cookie(&self.current_cookie()) {
            if let Ok(n) = uid.parse::<i64>() {
                payload.insert("user_id".to_string(), n.into());
            }
        }
        let json = serde_json::Value::Object(payload).to_string();
        let enc = m115_encode(&json);
        self.gate.wait();
        let resp = self
            .client
            .post(WEB_API_DOWNURL)
            .form(&[("data", enc)])
            .header(COOKIE, self.current_cookie())
            .header(USER_AGENT, WEB_DOWNLOAD_UA)
            .header(reqwest::header::REFERER, WEB_DOWNLOAD_REFERER)
            .send()
            .context("请求 115 下载直链失败")?;
        let status = resp.status().as_u16();
        let text = resp.text().unwrap_or_default();
        if !(200..300).contains(&status) {
            bail!(
                "115 下载直链接口 HTTP {status}: {}",
                text.chars().take(200).collect::<String>()
            );
        }
        let parsed: serde_json::Value =
            serde_json::from_str(&text).context("解析 115 直链响应失败")?;
        let state = parsed.get("state").cloned().unwrap_or(serde_json::Value::Null);
        let state_ok = match state {
            serde_json::Value::Bool(b) => b,
            serde_json::Value::Number(n) => n.as_i64() == Some(1),
            _ => true,
        };
        let errno = parsed
            .get("errno")
            .and_then(|v| v.as_i64())
            .or_else(|| parsed.get("errNo").and_then(|v| v.as_i64()))
            .unwrap_or(0);
        if !state_ok {
            let msg = parsed
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            bail!(map_115_web_errno(errno, msg));
        }
        let data = parsed
            .get("data")
            .and_then(|d| d.as_str())
            .ok_or_else(|| anyhow!("115 直链响应缺少 data"))?;
        let dec = m115_decode(data).with_context(|| {
            format!(
                "解密 115 直链响应失败，data 完整内容: {}",
                data
            )
        })?;
        let decoded: serde_json::Value = serde_json::from_slice(&dec).with_context(|| {
            format!(
                "解析 115 直链数据失败，解密结果前 96 字符: {}",
                String::from_utf8_lossy(&dec).chars().take(96).collect::<String>()
            )
        })?;
        let obj = decoded
            .as_object()
            .ok_or_else(|| anyhow!("115 直链数据格式错误"))?;
        let item = obj
            .values()
            .next()
            .ok_or_else(|| anyhow!("115 直链数据为空"))?;
        let url = item
            .get("url")
            .and_then(|u| u.get("url"))
            .and_then(|u| u.as_str())
            .unwrap_or_default()
            .to_string();
        if url.is_empty() {
            bail!("115 未返回下载直链");
        }
        let name = item.get("file_name").and_then(|v| v.as_str()).map(str::to_string);
        let size = item
            .get("file_size")
            .and_then(|v| v.as_i64())
            .map(|v| v.max(0) as u64)
            .or_else(|| {
                item.get("file_size")
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse().ok())
            });
        if let Some(n) = &name {
            if !n.is_empty() {
                self.names
                    .lock()
                    .unwrap()
                    .insert(pick_code.to_string(), n.clone());
            }
        }
        Ok(super::quark::DownloadInfo { url, size, name })
    }

    /// 解析 pick_code 对应的真实文件名：列表缓存 → 直链响应 → 报错。
    pub fn resolve_name(&self, pick_code: &str) -> Result<String> {
        if let Some(n) = self.names.lock().unwrap().get(pick_code).cloned() {
            return Ok(n);
        }
        let info = self.downurl(pick_code)?;
        info.name
            .filter(|n| !n.trim().is_empty())
            .ok_or_else(|| anyhow!("无法获取 115 文件名，请从书源浏览打开"))
    }

    /// 探测直链 Range 支持并返回总大小。
    pub fn probe(&self, url: &str) -> (bool, u64) {
        let resp = self
            .client
            .get(url)
            .header(RANGE, "bytes=0-0")
            .header(COOKIE, self.current_cookie())
            .header(USER_AGENT, WEB_DOWNLOAD_UA)
            .header(reqwest::header::REFERER, WEB_DOWNLOAD_REFERER)
            .send();
        let resp = match resp {
            Ok(r) => r,
            Err(_) => return (false, 0),
        };
        let content_length = || {
            resp.headers()
                .get(reqwest::header::CONTENT_LENGTH)
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(0)
        };
        if resp.status() == StatusCode::PARTIAL_CONTENT {
            let total = resp
                .headers()
                .get(reqwest::header::CONTENT_RANGE)
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.rsplit('/').next())
                .and_then(|x| x.trim().parse::<u64>().ok())
                .filter(|n| *n > 0);
            (true, total.unwrap_or_else(content_length))
        } else {
            (false, content_length())
        }
    }

    /// Range 读直链（带 Cookie + UA）；403 视为直链失效，由调用方重取一次。
    pub fn read_range_url(&self, url: &str, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let end = offset + buf.len() as u64 - 1;
        let mut resp = self
            .client
            .get(url)
            .header(RANGE, format!("bytes={}-{}", offset, end))
            .header(COOKIE, self.current_cookie())
            .header(USER_AGENT, WEB_DOWNLOAD_UA)
            .header(reqwest::header::REFERER, WEB_DOWNLOAD_REFERER)
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
                format!("115 直链不支持 Range(HTTP {})", resp.status().as_u16()),
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
        pick_code: &str,
        progress: Option<Arc<DownloadProgress>>,
    ) -> Result<PathBuf> {
        if let Some(p) = web_raw_cache_path(&self.origin(), pick_code) {
            if let Some(prog) = &progress {
                if let Ok(meta) = std::fs::metadata(&p) {
                    prog.downloaded.store(meta.len(), Ordering::SeqCst);
                    prog.total.store(meta.len(), Ordering::SeqCst);
                }
            }
            return Ok(p);
        }
        let info = self.downurl(pick_code)?;
        let name = info
            .name
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| "file.cbz".to_string());
        let name = name
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or("file.cbz")
            .to_string();
        let raw_dir = crate::cache::CacheDir::Raw
            .ensure()
            .context("创建 raw/ 缓存目录失败")?;
        let hash = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            format!("{}{}", self.origin(), pick_code).hash(&mut h);
            format!("{:016x}", h.finish())
        };
        let dir = raw_dir.join(&hash);
        std::fs::create_dir_all(&dir).ok();
        let file_path = dir.join(&name);
        if let Ok(meta) = std::fs::metadata(&file_path) {
            if meta.len() > 0 {
                return Ok(file_path);
            }
        }
        let mut resp = self
            .client
            .get(&info.url)
            .header(COOKIE, self.current_cookie())
            .header(USER_AGENT, WEB_DOWNLOAD_UA)
            .header(reqwest::header::REFERER, WEB_DOWNLOAD_REFERER)
            .send()
            .map_err(|e| anyhow!("下载失败:{e}"))?;
        if resp.status() == StatusCode::FORBIDDEN {
            let info2 = self.downurl(pick_code)?;
            resp = self
                .client
                .get(&info2.url)
                .header(COOKIE, self.current_cookie())
                .header(USER_AGENT, WEB_DOWNLOAD_UA)
                .header(reqwest::header::REFERER, WEB_DOWNLOAD_REFERER)
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
        let total = resp.content_length().unwrap_or(info.size.unwrap_or(0));
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

/// 115 网页版远端文件作为 ByteSource：Range 流式读，直链失效重取一次。
pub struct Cloud115WebFile {
    client: Arc<Cloud115WebClient>,
    pick_code: String,
    len: u64,
    url: Mutex<Option<String>>,
}

impl Cloud115WebFile {
    pub fn new(
        client: Arc<Cloud115WebClient>,
        pick_code: String,
        len: u64,
        url: String,
    ) -> Self {
        Cloud115WebFile {
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
        let info = self
            .client
            .downurl(&self.pick_code)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("取直链失败:{e}")))?;
        *self.url.lock().unwrap() = Some(info.url.clone());
        Ok(info.url)
    }
}

impl ByteSource for Cloud115WebFile {
    fn len(&self) -> u64 {
        self.len
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        let url = self.get_url()?;
        match self.client.read_range_url(&url, offset, buf) {
            Ok(n) => Ok(n),
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
                *self.url.lock().unwrap() = None;
                let u2 = self.get_url()?;
                self.client.read_range_url(&u2, offset, buf).map_err(|_| e)
            }
            Err(e) => Err(e),
        }
    }
}

/// 115 网页版 raw/ 缓存路径（`115web:{root}:{pick_code}` hash 目录）。
pub fn web_raw_cache_path(origin: &str, pick_code: &str) -> Option<PathBuf> {
    use std::hash::{Hash, Hasher};
    let hash = {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        format!("{}{}", origin, pick_code).hash(&mut h);
        format!("{:016x}", h.finish())
    };
    let dir = crate::cache::CacheDir::Raw.ensure().ok()?.join(&hash);
    for entry in std::fs::read_dir(&dir).ok()?.flatten() {
        if let Ok(meta) = entry.metadata() {
            if meta.len() > 0 {
                return Some(entry.path());
            }
        }
    }
    None
}

/// 解析 115 时间：目录是 unix 秒，文件是 "2006-01-02 15:04"（无时区，按 UTC+8）。
fn parse_115_time(t: &str) -> i64 {
    if let Ok(secs) = t.parse::<i64>() {
        return secs;
    }
    let b = t.as_bytes();
    if b.len() < 16 || b[4] != b'-' || b[7] != b'-' || b[10] != b' ' || b[13] != b':' {
        return 0;
    }
    let num2 = |i: usize| -> i64 {
        (b[i] - b'0') as i64 * 10 + (b[i + 1] - b'0') as i64
    };
    let num4 = |i: usize| -> i64 {
        (b[i] - b'0') as i64 * 1000
            + (b[i + 1] - b'0') as i64 * 100
            + (b[i + 2] - b'0') as i64 * 10
            + (b[i + 3] - b'0') as i64
    };
    let (y, mo, d) = (num4(0), num2(5), num2(8));
    let (h, mi) = (num2(11), num2(14));
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 {
        return 0;
    }
    // 天数转 unix（UTC 基准）+ 本地时刻 - UTC+8
    let y = if mo <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (mo + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    days * 86400 + h * 3600 + mi * 60 - 8 * 3600
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

    #[test]
    fn m115_encode_matches_reference_vector() {
        // 与 p115client/p115cipher 实现交叉验证：固定 key + 固定 padding 的参考向量，
        // 由 Python 按 p115cipher 算法逐字节生成。
        let json = r#"{"pickcode":"abc123"}"#;
        let pad: Vec<u8> = (1..40).collect();
        let enc = m115_encode_with(json, &pad);
        assert_eq!(
            enc,
            "RySYP54+MjnjYFI3tgW8vUVsFUWGIFBRMHmRINUyK8HEPCsLCOXo7c+vs0wTHPWqf5v5TufX2VQdI3G20cJSYqcj+fnSMTW8wwmmDD0Y0mSpLYLHXGdI9khICv69r+CTVtRB83xomisC/8nQW4Zv7qeQ9477TFcFU/Gr0aNs3oI="
        );
    }

    #[test]
    fn m115_xor_transform_is_self_inverse() {
        let mut data: Vec<u8> = (0..40).map(|i| (i * 7) as u8).collect();
        let key = m115_xor_derive_key(&[3u8; 16], 4);
        let orig = data.clone();
        m115_xor_transform(&mut data, &key);
        m115_xor_transform(&mut data, &key);
        assert_eq!(data, orig);
    }

    #[test]
    fn m115_encode_is_std_base64_multiple() {
        let enc = m115_encode(r#"{"pickcode":"x"}"#);
        // RSA 输出 128 字节块 → base64 长度为 4 的倍数
        assert!(enc.len() % 4 == 0);
        assert!(B64.decode(&enc).is_ok());
    }

    #[test]
    fn m115_rsa_key_len_is_128() {
        // 115 直链 RSA 模数为 1024 位 → 分块 128 字节。
        // 曾误写为 256 导致响应解密乱码，此测试防止回退。
        use num_bigint::BigUint;
        let n = BigUint::parse_bytes(M115_N_HEX.as_bytes(), 16).unwrap();
        assert_eq!(n.bits(), 1024);
        assert_eq!(((n.bits() as usize) + 7) / 8, 128);
        // 加密与解密必须使用相同分块
        let mut dummy = vec![0u8; 256];
        dummy[0] = 0;
        dummy[1] = 2;
        dummy[255] = 0;
        let enc = m115_rsa_encrypt(&dummy, &[]);
        assert_eq!(enc.len() % 128, 0);
        let _ = m115_rsa_transform(&enc);
    }

    #[test]
    fn user_id_extraction_from_cookie() {
        assert_eq!(
            user_id_from_cookie("UID=1234567890_abc; CID=1; SEID=2"),
            Some("1234567890".to_string())
        );
        assert_eq!(user_id_from_cookie("UID=9876543210; CID=1"), Some("9876543210".to_string()));
        assert_eq!(user_id_from_cookie("CID=1; SEID=2"), None);
        assert_eq!(user_id_from_cookie(""), None);
    }

    #[test]
    fn parse_115_time_both_kinds() {
        // 目录：unix 秒
        assert_eq!(parse_115_time("1700000000"), 1700000000);
        // 文件："2006-01-02 15:04"（无时区，按 UTC+8）
        assert_eq!(parse_115_time("2006-01-02 00:00"), 1136131200);
        assert_eq!(parse_115_time("2006-01-02 03:04"), 1136142240);
        // 垃圾输入
        assert_eq!(parse_115_time("?"), 0);
        assert_eq!(parse_115_time(""), 0);
    }

    #[test]
    fn parse_web_files_resp() {
        let json = r#"{
          "state": true, "code": 0, "message": "", "count": 2, "offset": 0,
          "data": [
            {"fid": "", "cid": "100", "n": "漫画文件夹", "s": 0, "t": "1700000000", "pc": ""},
            {"fid": "101", "cid": "100", "n": "漫画.cbz", "s": "12345", "t": "2024-01-02 03:04", "pc": "pc_abc"}
          ]
        }"#;
        let p: WebFilesResp = serde_json::from_str(json).unwrap();
        assert!(p.state);
        assert_eq!(p.count, 2);
        assert_eq!(p.data.len(), 2);
        assert!(p.data[0].fid.is_empty());
        assert_eq!(p.data[0].cid, "100");
        assert_eq!(p.data[1].fid, "101");
        assert_eq!(p.data[1].s.as_u64(), 12345);
        assert_eq!(p.data[1].pc, "pc_abc");
    }

    #[test]
    fn web_qr_app_whitelist() {
        assert!(WEB_QR_APPS.contains(&"web"));
        assert!(!WEB_QR_APPS.contains(&"windows"));
        assert!(!WEB_QR_APPS.contains(&"linux"));
        // 非法设备直接报错，不发请求
        assert!(web_qr_cookie("u", "windows").is_err());
    }
}
