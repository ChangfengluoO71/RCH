//! 夸克网盘书源：非官方 Web API（Cookie 认证，与 AList Quark 驱动同款契约）。
//!
//! - 认证：浏览器登录 `pan.quark.cn` 后粘贴 Cookie；请求带 `Cookie` + `Referer: https://pan.quark.cn`
//!   + quark-cloud-drive Electron UA，query 固定 `pr=ucpro&fr=pc`；响应 Set-Cookie 中的 `__puus` 回写续期。
//! - 列目录：`GET /file/sort?pdir_fid={fid}`（根目录 `0`），分页拉全，目录在前自然排序。
//! - 下载：`POST /file/download`（body `{"fids":[fid]}`）取直链；直链需带三件套头，Range 支持用
//!   `bytes=0-0` 探测（206 则流式，`Content-Range` 拿总大小；否则整本下载 raw/ 缓存回退）。
//! - 格式探测：fid 仅作 API / 缓存键；文件真实名（download 响应 `file_name` / 列表缓存）用于
//!   `open_document` 扩展名分发，规避 115 用提取码当 path 导致的探测失败隐患。
//!
//! 契约细节见 `.trellis/tasks/08-04-quark-book-source/research/quark-api-contract.md`（步骤 0 冒烟产出）。
use super::singleflight::SingleFlight;
use super::{ByteSource, Entry, RateGate};
use crate::remote_scan::adapter::{classify_range_probe_response, RangeProbe, RemoteScanError};
use crate::source::webdav::DownloadProgress;
use anyhow::{anyhow, bail, Context, Result};
use reqwest::blocking::Client;
use reqwest::header::{COOKIE, RANGE, REFERER, USER_AGENT};
use reqwest::{Method, StatusCode};
use serde::Deserialize;
use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const API_BASE: &str = "https://drive.quark.cn/1/clouddrive";
const API_CONFIG: &str = "/config";
const API_FILE_SORT: &str = "/file/sort";
const API_FILE_DOWNLOAD: &str = "/file/download";
/// 下载直链 / API 请求的 Referer 与 UA 必须与登录域一致（夸克会校验调用方）。
const QUARK_REFERER: &str = "https://pan.quark.cn";
const QUARK_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
(KHTML, like Gecko) quark-cloud-drive/2.5.20 Chrome/100.0.4896.160 \
Electron/18.3.5.4-b478491100 Safari/537.36 Channel/pckk_other_ch";
const PAGE_SIZE: i64 = 100;

/// 直链缓存 TTL。
///
/// **代码里没有任何证据说明夸克直链的有效期**（P0-D 前置审计已确认：全仓库唯一
/// 的直链 TTL 常量是 115 的 300 s，夸克侧只有"403 后重取一次"的反应式处理）。
/// 因此这里取保守值，并配合两道自愈机制：
///
/// - HTTP 403 → 立即失效并重取一次（`QuarkFile::read_at` / `download_to_raw_cache`）；
/// - cookie 代际变化 → 整个缓存对该代际不复用（见 `cookie_epoch`）。
///
/// TTL 偏长是自愈的，偏短只会损失收益；最终值应由真机测量校正。
const QUARK_DLINK_CACHE_TTL: Duration = Duration::from_secs(120);
/// 直链缓存容量上限。超过后按 **LRU** 淘汰（不是 115 那种 HashMap 任意键淘汰）。
const QUARK_DLINK_CACHE_CAPACITY: usize = 256;

/// 列表响应（`data.list[]` + `metadata._total`）。
#[derive(Debug, Deserialize)]
struct SortResp {
    #[serde(default)]
    data: SortData,
    #[serde(default)]
    metadata: SortMetadata,
}

#[derive(Debug, Default, Deserialize)]
struct SortData {
    #[serde(default)]
    list: Vec<QuarkFileItem>,
}

#[derive(Debug, Default, Deserialize)]
struct SortMetadata {
    #[serde(default, rename = "_total")]
    total: i64,
}

#[derive(Debug, Clone, Deserialize)]
struct QuarkFileItem {
    fid: String,
    #[serde(rename = "file_name")]
    file_name: String,
    #[serde(default)]
    size: u64,
    /// true = 文件，false = 文件夹。
    #[serde(default)]
    file: bool,
    #[serde(default)]
    updated_at: Option<i64>,
}

/// 下载直链响应（`data[0].download_url`；`file_name` / `size` 为可选字段）。
#[derive(Debug, Deserialize)]
struct DownResp {
    #[serde(default)]
    data: Vec<DownItem>,
}

#[derive(Debug, Default, Deserialize)]
struct DownItem {
    #[serde(rename = "download_url", default)]
    download_url: String,
    #[serde(rename = "file_name", default)]
    file_name: Option<String>,
    #[serde(default)]
    size: Option<u64>,
}

/// 下载直链信息。
#[derive(Debug, Clone)]
pub struct DownloadInfo {
    pub url: String,
    pub size: Option<u64>,
    pub name: Option<String>,
}

fn http_client() -> Result<Client> {
    Client::builder()
        .timeout(Duration::from_secs(60))
        .connect_timeout(Duration::from_secs(10))
        .build()
        .context("创建 HTTP 客户端失败")
}

/// 在 cookie 串中新增 / 替换 `name=value`（用于 `__puus` 续期回写）。
fn upsert_cookie(cookie: &str, name: &str, value: &str) -> String {
    let mut parts: Vec<String> = cookie
        .split(';')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let hit = parts.iter_mut().find(|p| {
        p.split('=')
            .next()
            .map(|k| k.trim() == name)
            .unwrap_or(false)
    });
    match hit {
        Some(p) => *p = format!("{name}={value}"),
        None => parts.push(format!("{name}={value}")),
    }
    parts.join("; ")
}

/// 错误码 → 中文提示。具体 code 语义在步骤 0 冒烟时以真实响应为准补全。
fn map_quark_error(code: i64, message: &str) -> String {
    match code {
        401 | 4000 => "登录状态失效，请重新粘贴夸克 Cookie".to_string(),
        _ => {
            if message.is_empty() {
                format!("夸克 API 错误({code})")
            } else {
                format!("夸克 API 错误({code}): {message}")
            }
        }
    }
}

/// 夸克网盘客户端（blocking，配合 `spawn_blocking` 使用）。
pub struct QuarkClient {
    client: Client,
    cookie: Mutex<String>,
    root: String,
    gate: RateGate,
    /// fid -> 真实文件名（列表时填充；用于格式探测与 raw 缓存命名）。
    names: Mutex<HashMap<String, String>>,
    /// fid -> 直链（受 TTL / cookie 代际 / LRU 容量约束）。
    ///
    /// 改造前夸克**完全没有直链缓存**：adapter 的每次 `read_range` 都会重新调用
    /// `file/download`（见 `api/source.rs`）。一次封面提取因此要发 3~6 次取链。
    dlinks: Mutex<DlinkCache>,
    /// 按 fid 合并并发取链；取代"无缓存 + 2 r/s 串行"。
    dlink_flight: SingleFlight<String, std::result::Result<DownloadInfo, String>>,
    /// `__puus` 代际。只有真正发生变化时才自增，避免被无变化的 Set-Cookie 误伤。
    cookie_epoch: AtomicU64,
}

/// 直链缓存（带 LRU 次序戳）。
#[derive(Default)]
struct DlinkCache {
    entries: HashMap<String, CachedDlink>,
    seq: u64,
}

#[derive(Clone)]
struct CachedDlink {
    info: DownloadInfo,
    fetched_at: Instant,
    /// 取链时生效的 `__puus` 代际；不同代际一律不复用。
    cookie_epoch: u64,
    /// LRU 次序戳（单调递增，越大越新）。
    seq: u64,
}

impl DlinkCache {
    fn get(&mut self, fid: &str, epoch: u64) -> Option<DownloadInfo> {
        let expired = {
            let entry = self.entries.get(fid)?;
            entry.cookie_epoch != epoch || entry.fetched_at.elapsed() >= QUARK_DLINK_CACHE_TTL
        };
        if expired {
            self.entries.remove(fid);
            return None;
        }
        let info = self.entries.get(fid)?.info.clone();
        self.seq += 1;
        let seq = self.seq;
        if let Some(entry) = self.entries.get_mut(fid) {
            entry.seq = seq;
        }
        Some(info)
    }

    fn put(&mut self, fid: &str, info: DownloadInfo, epoch: u64) {
        self.seq += 1;
        let seq = self.seq;
        self.entries.insert(
            fid.to_string(),
            CachedDlink {
                info,
                fetched_at: Instant::now(),
                cookie_epoch: epoch,
                seq,
            },
        );
        while self.entries.len() > QUARK_DLINK_CACHE_CAPACITY {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.seq)
                .map(|(fid, _)| fid.clone())
            else {
                break;
            };
            self.entries.remove(&oldest);
        }
    }

    fn invalidate(&mut self, fid: &str) {
        self.entries.remove(fid);
    }
}

impl QuarkClient {
    pub fn new(cookie: &str, root: &str) -> Result<Self> {
        Ok(QuarkClient {
            client: http_client()?,
            cookie: Mutex::new(cookie.trim().to_string()),
            root: if root.is_empty() {
                "0".to_string()
            } else {
                root.to_string()
            },
            gate: RateGate::fixed_interval("quark.api", 2.0),
            names: Mutex::new(HashMap::new()),
            dlinks: Mutex::new(DlinkCache::default()),
            dlink_flight: SingleFlight::new(),
            cookie_epoch: AtomicU64::new(0),
        })
    }

    /// 缓存命名空间前缀（cookie 会轮换，只用 root，保持稳定）。
    pub fn origin(&self) -> String {
        format!("quark:{}", self.root)
    }

    pub fn root(&self) -> &str {
        &self.root
    }

    fn current_cookie(&self) -> String {
        self.cookie.lock().unwrap().clone()
    }

    /// 当前会话 cookie（可能已回写 `__puus` 续期），供 Dart 侧回写 DB。
    pub fn cookie(&self) -> String {
        self.current_cookie()
    }

    /// 统一请求封装：GET/POST + 三件套头 + `pr/fr` 参数；`code != 0` 报错；
    /// 响应 Set-Cookie 中的 `__puus` 回写续期。
    fn request(
        &self,
        path: &str,
        method: Method,
        query: &[(&str, String)],
        body: Option<serde_json::Value>,
    ) -> Result<String> {
        let gate_guard = self.gate.enter();
        crate::source::record_gate_wait(self.gate.channel(), gate_guard.waited_us());
        let cookie = self.current_cookie();
        let url = format!("{API_BASE}{path}");
        let mut builder = match method {
            Method::GET => self.client.get(&url),
            Method::POST => self.client.post(&url),
            _ => bail!("夸克 API 不支持的方法"),
        };
        builder = builder
            .header(COOKIE, &cookie)
            .header(REFERER, QUARK_REFERER)
            .header(USER_AGENT, QUARK_UA)
            .query(&[("pr", "ucpro"), ("fr", "pc")])
            .query(query);
        if let Some(b) = body {
            builder = builder.json(&b);
        }
        let resp = builder.send().context("夸克 API 请求失败")?;
        let status = resp.status();
        let set_cookie: Vec<String> = resp
            .headers()
            .get_all(reqwest::header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .map(|s| s.to_string())
            .collect();
        let text = resp.text().unwrap_or_default();
        if let Some(puus) = set_cookie.iter().find_map(|s| {
            s.split(';').next().and_then(|kv| {
                let (k, v) = kv.split_once('=')?;
                if k.trim() == "__puus" && !v.trim().is_empty() {
                    Some(v.trim().to_string())
                } else {
                    None
                }
            })
        }) {
            let mut c = self.cookie.lock().unwrap();
            let updated = upsert_cookie(&c, "__puus", &puus);
            // 只有 `__puus` 真的变了才推进代际：无变化的 Set-Cookie 不该让整个
            // 直链缓存作废（否则缓存几乎永远不命中）。
            if updated != *c {
                self.cookie_epoch.fetch_add(1, Ordering::SeqCst);
            }
            *c = updated;
        }
        if !status.is_success() {
            bail!(
                "夸克 API HTTP {}: {}",
                status.as_u16(),
                text.chars().take(200).collect::<String>()
            );
        }
        let parsed: serde_json::Value =
            serde_json::from_str(&text).context("解析夸克 API 响应失败")?;
        let code = parsed.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
        if code != 0 {
            let message = parsed
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            bail!(map_quark_error(code, message));
        }
        Ok(text)
    }

    /// 连通性测试：`/config` + 根目录首屏 list。
    pub fn check(&self) -> Result<()> {
        let _ = self.request(API_CONFIG, Method::GET, &[], None)?;
        let _ = self.list(&self.root)?;
        Ok(())
    }

    /// 列目录（按 fid；根目录 `0`），分页拉全，目录在前自然排序；同时缓存 fid → 文件名。
    pub fn list(&self, fid: &str) -> Result<Vec<Entry>> {
        let mut all = Vec::new();
        let mut page = 1i64;
        loop {
            let body = self.request(
                API_FILE_SORT,
                Method::GET,
                &[
                    ("pdir_fid", fid.to_string()),
                    ("_page", page.to_string()),
                    ("_size", PAGE_SIZE.to_string()),
                    ("_fetch_total", "1".to_string()),
                    ("fetch_all_file", "1".to_string()),
                    ("fetch_risk_file_name", "1".to_string()),
                    ("_sort", "file_type:asc,file_name:asc".to_string()),
                ],
                None,
            )?;
            let parsed: SortResp = serde_json::from_str(&body).context("解析夸克文件列表失败")?;
            let n = parsed.data.list.len();
            {
                let mut names = self.names.lock().unwrap();
                for it in &parsed.data.list {
                    if it.file {
                        names.insert(it.fid.clone(), it.file_name.clone());
                    }
                }
            }
            all.extend(parsed.data.list.into_iter().map(|it| Entry {
                name: it.file_name,
                path: it.fid,
                is_dir: !it.file,
                size: it.size,
                mtime: it.updated_at.unwrap_or(0) / 1000,
            }));
            if page * PAGE_SIZE >= parsed.metadata.total || n < PAGE_SIZE as usize {
                break;
            }
            page += 1;
        }
        all.sort_by(|a, b| {
            b.is_dir
                .cmp(&a.is_dir)
                .then_with(|| crate::util::natural_cmp(&a.name, &b.name))
        });
        Ok(all)
    }

    /// 直链缓存查询（受 TTL 与 cookie 代际约束）。
    pub fn cached_dlink(&self, fid: &str) -> Option<DownloadInfo> {
        let epoch = self.cookie_epoch.load(Ordering::SeqCst);
        self.dlinks.lock().unwrap().get(fid, epoch)
    }

    /// 主动失效某个 fid 的直链（403 后调用）。
    pub fn invalidate_dlink(&self, fid: &str) {
        self.dlinks.lock().unwrap().invalidate(fid);
    }

    /// 取下载直链：命中缓存直接返回，否则按 fid 合并并发请求。
    ///
    /// 合并粒度是**文件**：不同 fid 互不阻塞；账号级的 2 r/s 门控仍然共享。
    pub fn downlink(&self, fid: &str) -> Result<DownloadInfo> {
        if let Some(info) = self.cached_dlink(fid) {
            return Ok(info);
        }
        let outcome = self.dlink_flight.run(fid.to_string(), || {
            // leader 的二次检查：登记的瞬间可能已经有别人填好缓存。
            if let Some(info) = self.cached_dlink(fid) {
                return Ok(info);
            }
            self.fetch_downlink(fid).map_err(|error| error.to_string())
        });
        // 只缓存**成功**结果：把瞬时错误（超时/429）缓存下来会把整个会话的该
        // fid 钉死（P0-D 前置审计风险 3）。
        match outcome.value {
            Ok(info) => Ok(info),
            Err(message) => anyhow::bail!("{message}"),
        }
    }

    /// 真正发起取链请求，并在成功后写入缓存。
    fn fetch_downlink(&self, fid: &str) -> Result<DownloadInfo> {
        let body = self.request(
            API_FILE_DOWNLOAD,
            Method::POST,
            &[],
            Some(serde_json::json!({ "fids": [fid] })),
        )?;
        let parsed: DownResp = serde_json::from_str(&body).context("解析夸克下载直链失败")?;
        let item = parsed
            .data
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("夸克下载接口未返回直链"))?;
        if item.download_url.is_empty() {
            bail!("夸克下载接口未返回直链");
        }
        if let Some(name) = &item.file_name {
            if !name.is_empty() {
                self.names
                    .lock()
                    .unwrap()
                    .insert(fid.to_string(), name.clone());
            }
        }
        let info = DownloadInfo {
            url: item.download_url,
            size: item.size,
            name: item.file_name,
        };
        // 只缓存成功结果。瞬时错误（网络超时 / 风控）绝不进缓存。
        let epoch = self.cookie_epoch.load(Ordering::SeqCst);
        self.dlinks
            .lock()
            .unwrap()
            .put(fid, info.clone(), epoch);
        Ok(info)
    }

    /// 解析 fid 对应的真实文件名：列表缓存 → download 响应 → 报错。
    pub fn resolve_name(&self, fid: &str) -> Result<String> {
        if let Some(n) = self.names.lock().unwrap().get(fid).cloned() {
            return Ok(n);
        }
        let info = self.downlink(fid)?;
        info.name
            .filter(|n| !n.trim().is_empty())
            .ok_or_else(|| anyhow!("无法获取夸克文件名，请从书源浏览打开"))
    }

    /// 探测直链 Range 支持并返回总大小（206 + Content-Range / Content-Length）。
    pub fn probe(&self, url: &str) -> (bool, u64) {
        match self.probe_checked(url) {
            Ok(probe) => (probe.supported, probe.total_size.unwrap_or(0)),
            Err(_) => (false, 0),
        }
    }

    /// 带错误语义的直链 Range 探测。
    pub fn probe_checked(&self, url: &str) -> Result<RangeProbe, RemoteScanError> {
        let resp = self
            .client
            .get(url)
            .header(RANGE, "bytes=0-0")
            .header(COOKIE, self.current_cookie())
            .header(REFERER, QUARK_REFERER)
            .header(USER_AGENT, QUARK_UA)
            .send()
            .map_err(|_| RemoteScanError::TransientNetwork("range_probe_network".into()))?;
        let status = resp.status().as_u16();
        let content_range = resp
            .headers()
            .get(reqwest::header::CONTENT_RANGE)
            .and_then(|value| value.to_str().ok());
        let content_length = resp
            .headers()
            .get(reqwest::header::CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok());
        classify_range_probe_response(status, content_range, content_length)
    }

    /// Range 读直链（带三件套头）；403 视为直链失效，由调用方重取一次。
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
            .header(REFERER, QUARK_REFERER)
            .header(USER_AGENT, QUARK_UA)
            .send()
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("Range 请求失败:{e}")))?;
        if resp.status() == StatusCode::FORBIDDEN {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "夸克直链失效，请重试",
            ));
        }
        if resp.status() != StatusCode::PARTIAL_CONTENT {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                format!("夸克直链未支持 Range(HTTP {})", resp.status().as_u16()),
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

    /// 整本下载到 raw/ 缓存（有进度）；已缓存（目录内存在非空文件）则复用。
    pub fn download_to_raw_cache(
        &self,
        fid: &str,
        progress: Option<Arc<DownloadProgress>>,
    ) -> Result<PathBuf> {
        if let Some(p) = raw_cache_path(&self.origin(), fid) {
            if let Some(prog) = &progress {
                if let Ok(meta) = std::fs::metadata(&p) {
                    prog.downloaded.store(meta.len(), Ordering::SeqCst);
                    prog.total.store(meta.len(), Ordering::SeqCst);
                }
            }
            return Ok(p);
        }
        let info = self.downlink(fid)?;
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
            format!("{}{}", self.origin(), fid).hash(&mut h);
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
            .header(REFERER, QUARK_REFERER)
            .header(USER_AGENT, QUARK_UA)
            .send()
            .map_err(|e| anyhow!("下载失败:{e}"))?;
        if resp.status() == StatusCode::FORBIDDEN {
            // 同上：先失效共享缓存，否则这里会拿回同一条已失效的直链。
            self.invalidate_dlink(fid);
            let info2 = self.downlink(fid)?;
            resp = self
                .client
                .get(&info2.url)
                .header(COOKIE, self.current_cookie())
                .header(REFERER, QUARK_REFERER)
                .header(USER_AGENT, QUARK_UA)
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

/// 夸克远端文件作为 ByteSource：Range 流式读；直链失效重取一次。
pub struct QuarkFile {
    client: Arc<QuarkClient>,
    fid: String,
    len: u64,
    dlink: Mutex<Option<String>>,
}

impl QuarkFile {
    pub fn new(client: Arc<QuarkClient>, fid: String, len: u64, dlink: String) -> Self {
        QuarkFile {
            client,
            fid,
            len,
            dlink: Mutex::new(Some(dlink)),
        }
    }

    fn get_dlink(&self) -> io::Result<String> {
        if let Some(d) = self.dlink.lock().unwrap().clone() {
            return Ok(d);
        }
        let info = self
            .client
            .downlink(&self.fid)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("取直链失败:{e}")))?;
        *self.dlink.lock().unwrap() = Some(info.url.clone());
        Ok(info.url)
    }
}

impl ByteSource for QuarkFile {
    fn len(&self) -> u64 {
        self.len
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        let url = self.get_dlink()?;
        match self.client.read_range_url(&url, offset, buf) {
            Ok(n) => Ok(n),
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
                // 直链失效：先失效**共享**缓存再重取一次。
                // 只清本实例的 `dlink` 是不够的 —— 扫描/封面走的是 adapter 路径，
                // 它们读的是 `QuarkClient` 上的共享缓存（前置审计风险 9）。
                self.client.invalidate_dlink(&self.fid);
                *self.dlink.lock().unwrap() = None;
                let url2 = self.get_dlink()?;
                self.client
                    .read_range_url(&url2, offset, buf)
                    .map_err(|_| e)
            }
            Err(e) => Err(e),
        }
    }
}

/// raw/ 缓存路径：hash 目录（`quark:{root}:{fid}`）内的任意非空文件即命中。
pub fn raw_cache_path(origin: &str, fid: &str) -> Option<PathBuf> {
    use std::hash::{Hash, Hasher};
    let hash = {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        format!("{}{}", origin, fid).hash(&mut h);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_sort_resp() {
        let json = r#"{
          "status": 200, "code": 0, "message": "",
          "data": {
            "list": [
              {"fid": "f1", "file_name": "漫画文件夹", "file": false, "size": 0, "updated_at": 1700000000000},
              {"fid": "f2", "file_name": "漫画.cbz", "file": true, "size": 12345, "updated_at": 1700000001000}
            ]
          },
          "metadata": {"_total": 2, "_page": 1, "_count": 2, "_size": 100, "way": "list"}
        }"#;
        let p: SortResp = serde_json::from_str(json).unwrap();
        assert_eq!(p.metadata.total, 2);
        assert_eq!(p.data.list.len(), 2);
        assert!(!p.data.list[0].file);
        assert_eq!(p.data.list[1].file_name, "漫画.cbz");
        assert_eq!(p.data.list[1].size, 12345);
    }

    #[test]
    fn parse_down_resp() {
        let json = r#"{
          "status": 200, "code": 0, "message": "",
          "data": [{
            "fid": "f2", "file_name": "漫画.cbz", "size": 12345,
            "download_url": "https://quark-download.example.com/xxx?sign=1"
          }],
          "metadata": {"acc2": "1", "acc1": "1"}
        }"#;
        let p: DownResp = serde_json::from_str(json).unwrap();
        assert_eq!(p.data.len(), 1);
        assert_eq!(
            p.data[0].download_url,
            "https://quark-download.example.com/xxx?sign=1"
        );
        assert_eq!(p.data[0].file_name.as_deref(), Some("漫画.cbz"));
        assert_eq!(p.data[0].size, Some(12345));
    }

    #[test]
    fn parse_error_resp() {
        let json = r#"{"status": 401, "code": 4000, "message": "登录已过期"}"#;
        let v: serde_json::Value = serde_json::from_str(json).unwrap();
        let code = v.get("code").and_then(|x| x.as_i64()).unwrap_or(-1);
        assert_eq!(code, 4000);
        assert!(map_quark_error(code, "登录已过期").contains("登录状态失效"));
        assert!(map_quark_error(5000, "内部错误").contains("5000"));
    }

    #[test]
    fn upsert_cookie_sets_and_replaces() {
        let c = upsert_cookie("a=1; b=2", "__puus", "x");
        assert!(c.contains("__puus=x"));
        assert!(c.contains("a=1"));
        let c2 = upsert_cookie(&c, "__puus", "y");
        assert!(c2.contains("__puus=y"));
        assert!(!c2.contains("__puus=x"));
    }

    fn sample(fid: &str) -> DownloadInfo {
        DownloadInfo {
            url: format!("https://cdn.example.test/{fid}"),
            size: Some(1),
            name: Some("f.cbz".to_string()),
        }
    }

    /// P0-D：直链缓存必须命中、必须过期、必须可主动失效。
    #[test]
    fn dlink_cache_hits_expires_and_invalidates() {
        let client = QuarkClient::new("__puus=a", "0").unwrap();
        let epoch = client.cookie_epoch.load(Ordering::SeqCst);

        // 命中
        client.dlinks.lock().unwrap().put("fid-a", sample("fid-a"), epoch);
        assert_eq!(
            client.cached_dlink("fid-a").map(|value| value.url),
            Some("https://cdn.example.test/fid-a".to_string())
        );

        // 主动失效（403 路径会走这里）
        client.invalidate_dlink("fid-a");
        assert!(client.cached_dlink("fid-a").is_none());

        // 过期：直接写入一条超出 TTL 的记录，保持用例确定性。
        client.dlinks.lock().unwrap().entries.insert(
            "fid-old".to_string(),
            CachedDlink {
                info: sample("fid-old"),
                fetched_at: Instant::now() - QUARK_DLINK_CACHE_TTL - Duration::from_secs(1),
                cookie_epoch: epoch,
                seq: 1,
            },
        );
        assert!(
            client.cached_dlink("fid-old").is_none(),
            "an entry older than the TTL must not be served"
        );
    }

    /// `__puus` 轮换后旧直链必须不复用。
    ///
    /// 审计已确认"直链签名究竟绑定 cookie 的哪一部分"在代码里**没有依据**，因此
    /// 这里采取保守策略：代际变了就不复用。也正因为保守，必须验证"没有变化的
    /// Set-Cookie 不会推进代际"，否则缓存几乎永远不命中。
    #[test]
    fn dlink_cache_is_discarded_when_the_cookie_epoch_changes() {
        let client = QuarkClient::new("__puus=a", "0").unwrap();
        let epoch = client.cookie_epoch.load(Ordering::SeqCst);
        client.dlinks.lock().unwrap().put("fid-a", sample("fid-a"), epoch);
        assert!(client.cached_dlink("fid-a").is_some());

        // 代际推进（等价于 `__puus` 真的变了）。
        client.cookie_epoch.fetch_add(1, Ordering::SeqCst);
        assert!(
            client.cached_dlink("fid-a").is_none(),
            "a link fetched under another cookie generation must not be reused"
        );

        // 无变化的 Set-Cookie 不得推进代际：这里模拟 upsert_cookie 返回同值。
        let before = client.cookie_epoch.load(Ordering::SeqCst);
        {
            let mut cookie = client.cookie.lock().unwrap();
            let updated = upsert_cookie(&cookie, "__puus", "a");
            if updated != *cookie {
                client.cookie_epoch.fetch_add(1, Ordering::SeqCst);
            }
            *cookie = updated;
        }
        assert_eq!(
            client.cookie_epoch.load(Ordering::SeqCst),
            before,
            "an unchanged Set-Cookie must not invalidate the whole cache"
        );
    }

    /// 容量必须有界，且按 LRU 淘汰最旧而不是任意键。
    #[test]
    fn dlink_cache_is_lru_bounded() {
        let client = QuarkClient::new("__puus=a", "0").unwrap();
        let epoch = client.cookie_epoch.load(Ordering::SeqCst);
        for index in 0..QUARK_DLINK_CACHE_CAPACITY + 10 {
            client
                .dlinks
                .lock()
                .unwrap()
                .put(&format!("fid-{index}"), sample("x"), epoch);
        }
        assert_eq!(
            client.dlinks.lock().unwrap().entries.len(),
            QUARK_DLINK_CACHE_CAPACITY,
            "the cache must stay bounded"
        );
        assert!(
            client.cached_dlink("fid-0").is_none(),
            "the oldest entry must be evicted first"
        );
        assert!(
            client
                .cached_dlink(&format!("fid-{}", QUARK_DLINK_CACHE_CAPACITY + 9))
                .is_some(),
            "the newest entry must survive"
        );
    }

    /// 同 fid 并发只发一次取链；不同 fid 互不阻塞。
    #[test]
    fn dlink_coalesces_same_fid_without_serialising_other_fids() {
        use std::sync::atomic::AtomicUsize;
        let client = Arc::new(QuarkClient::new("__puus=a", "0").unwrap());
        let calls = Arc::new(AtomicUsize::new(0));

        let mut same = Vec::new();
        for _ in 0..6 {
            let client = Arc::clone(&client);
            let calls = Arc::clone(&calls);
            same.push(std::thread::spawn(move || {
                client
                    .dlink_flight
                    .run("fid-shared".to_string(), || {
                        calls.fetch_add(1, Ordering::SeqCst);
                        std::thread::sleep(Duration::from_millis(120));
                        Ok(sample("fid-shared"))
                    })
                    .value
            }));
        }
        for handle in same {
            assert!(handle.join().unwrap().is_ok());
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        let calls = Arc::new(AtomicUsize::new(0));
        let started = Instant::now();
        let mut different = Vec::new();
        for index in 0..4_u32 {
            let client = Arc::clone(&client);
            let calls = Arc::clone(&calls);
            different.push(std::thread::spawn(move || {
                client
                    .dlink_flight
                    .run(format!("fid-{index}"), || {
                        calls.fetch_add(1, Ordering::SeqCst);
                        std::thread::sleep(Duration::from_millis(150));
                        Ok(sample("x"))
                    })
                    .leader
            }));
        }
        let leaders = different
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .filter(|leader| *leader)
            .count();
        let elapsed = started.elapsed();
        assert_eq!(leaders, 4);
        assert!(
            elapsed < Duration::from_millis(500),
            "different fids must not serialise behind one another; took {elapsed:?}"
        );
    }

    /// leader 失败：错误一致传播、不重试成风暴、**不写缓存**。
    #[test]
    fn failed_dlink_leader_propagates_without_stampede_or_poisoning() {
        use std::sync::atomic::AtomicUsize;
        let client = Arc::new(QuarkClient::new("__puus=a", "0").unwrap());
        let calls = Arc::new(AtomicUsize::new(0));
        let mut handles = Vec::new();
        for _ in 0..5 {
            let client = Arc::clone(&client);
            let calls = Arc::clone(&calls);
            handles.push(std::thread::spawn(move || {
                client
                    .dlink_flight
                    .run("fid-bad".to_string(), || {
                        calls.fetch_add(1, Ordering::SeqCst);
                        std::thread::sleep(Duration::from_millis(60));
                        Err("夸克 API 错误(41017): 请求过于频繁".to_string())
                    })
                    .value
            }));
        }
        let results: Vec<std::result::Result<DownloadInfo, String>> = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect();
        assert!(
            results.iter().all(|value| value.is_err()),
            "every caller must observe the leader failure"
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "a failing leader must not cause followers to retry"
        );
        assert!(
            client.cached_dlink("fid-bad").is_none(),
            "a failed fetch must never be cached"
        );
    }

    #[test]
    fn raw_cache_path_smoke() {
        let _ = raw_cache_path("quark:0", "f2");
        let _ = raw_cache_path("quark:0", "文件夹/f3");
    }

    #[test]
    fn download_info_name_fallback() {
        let info = DownloadInfo {
            url: "https://x".to_string(),
            size: None,
            name: None,
        };
        assert!(info.name.is_none());
        assert_eq!(info.size, None);
    }
}
