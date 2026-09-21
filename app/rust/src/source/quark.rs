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
use std::io::{self, Read};
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

    /// 一次大 Range 请求的**并行拆分阈值**（第 81 轮续4）。
    ///
    /// **默认关闭**（`usize::MAX`）：受控 A/B 实测结论 —— 同一批 22–32 MB 的 PDF 封面，
    /// 并行关 1.45–1.73 s、并行开 1.60–1.94 s（**并行略慢 ~8%**）⇒ 瓶颈不是"单连接慢"，
    /// 而是链路/账号总带宽 ⇒ 并行只多占一条并发、没有收益，故默认不启用 ✗。
    /// 代码与开关保留，方便日后换网络/换 provider 时**复现这次 A/B**
    /// （`RCH_RANGE_PARALLEL_MIN=524288` 即为开启）。
    const PARALLEL_RANGE_MIN: usize = usize::MAX;

    /// 并行拆分阈值；可用环境变量 `RCH_RANGE_PARALLEL_MIN` 覆盖，**专门用于受控 A/B**
    /// （同一个桌面进程、同一批书，只改这一个数 ⇒ 才能判定"并行有没有用"）。
    ///
    /// 只解析一次并缓存（`OnceLock`）：第一版每次读都调 `env::var` ⇒ 每次读多一次
    /// 系统调用与锁，直接把 P0 单页延迟基线顶爆（门禁当场挡回）。
    fn parallel_range_min() -> usize {
        static MIN: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
        *MIN.get_or_init(|| {
            std::env::var("RCH_RANGE_PARALLEL_MIN")
                .ok()
                .and_then(|value| value.trim().parse::<usize>().ok())
                .unwrap_or(Self::PARALLEL_RANGE_MIN)
        })
    }

    /// Range 读直链（带三件套头）；403 视为直链失效，由调用方重取一次。
    ///
    /// 第 81 轮续4：大于阈值（默认 [`Self::PARALLEL_RANGE_MIN`]）的读**拆成两半并发**。
    /// 两半都成功才返回合计长度（任何一半失败即整体失败，语义与单请求版一致）。
    pub fn read_range_url(&self, url: &str, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if buf.len() < Self::parallel_range_min() {
            return self.read_range_url_single(url, offset, buf);
        }
        let mid = buf.len() / 2;
        let (left, right) = buf.split_at_mut(mid);
        let second_offset = offset + mid as u64;
        let (first, second) = std::thread::scope(|scope| {
            let handle = scope.spawn(move || self.read_range_url_single(url, offset, left));
            let second = self.read_range_url_single(url, second_offset, right);
            (handle.join(), second)
        });
        let first = first.map_err(|_| io::Error::other("Range 并发读线程 panic"))??;
        let second = second?;
        Ok(first + second)
    }

    /// 单次 Range 请求（原实现，逐字节填满缓冲）。
    fn read_range_url_single(&self, url: &str, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
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
        let mut writer = crate::cache::AtomicCacheFile::create(&file_path)?;
        let mut buf = [0u8; 64 * 1024];
        let mut written: u64 = 0;
        loop {
            let n = resp.read(&mut buf).context("读取下载流失败")?;
            if n == 0 {
                break;
            }
            writer.write_all(&buf[..n])?;
            written += n as u64;
            if let Some(p) = &progress {
                p.downloaded.store(written, Ordering::SeqCst);
            }
        }
        writer.commit()
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
/// raw/ 缓存的**确定性目录**（P1-D-2，Family 2）。
///
/// 目录可由 `(authority, fid)` 精确推导；但**文件名**来自 provider 网络响应
/// （`downlink` 的 `info.name`）且未持久化，因此这里**只给目录**，
/// 不提供文件级 candidate、也不返回任何猜测值。
pub fn raw_cache_dir(origin: &str, fid: &str) -> PathBuf {
    use std::hash::{Hash, Hasher};
    let hash = {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        format!("{}{}", origin, fid).hash(&mut h);
        format!("{:016x}", h.finish())
    };
    crate::cache::CacheDir::Raw.path().join(&hash)
}

pub fn raw_cache_path(origin: &str, fid: &str) -> Option<PathBuf> {
    crate::cache::CacheDir::Raw.ensure().ok()?;
    let dir = raw_cache_dir(origin, fid);
    for entry in std::fs::read_dir(&dir).ok()?.flatten() {
        if let Ok(meta) = entry.metadata() {
            if meta.len() > 0 {
                return Some(entry.path());
            }
        }
    }
    None
}


// ============================================================
// 网页扫码登录（Cookie 模式，免 F12）
// ============================================================
//
// 接口依据（夸克 passport 公开接口，参考实现与 API 手册见 LOG 第 66 轮）：
//   1) GET uop.quark.cn/cas/ajax/getTokenForQrcodeLogin
//        ?client_id=532&v=1.2&request_id=<uuid>      → data.members.token
//   2) GET uop.quark.cn/cas/ajax/getServiceTicketByQrcodeToken
//        同参数 + token
//        status=2000000 + data.members.service_ticket ⇒ 已确认登录
//        50004001 = 等待扫码；50004002/50004003/50004004 = 登录失败/取消
//   3) GET pan.quark.cn/account/info?st=<ticket>&lw=scan&platform=pc
//        响应的 Set-Cookie 即会话 Cookie
// 二维码内容 = https://su.quark.cn/4_eMHBJ?token=<token>&client_id=532&v=1.2

const QR_TOKEN_URL: &str = "https://uop.quark.cn/cas/ajax/getTokenForQrcodeLogin";
const QR_TICKET_URL: &str = "https://uop.quark.cn/cas/ajax/getServiceTicketByQrcodeToken";
const ACCOUNT_INFO_URL: &str = "https://pan.quark.cn/account/info";
const QR_BASE_URL: &str = "https://su.quark.cn/4_eMHBJ";
const QR_CLIENT_ID: &str = "532";
const QR_API_VERSION: &str = "1.2";

/// 扫码载荷：`token`/`request_id` 用于轮询，`qrcode` 用于渲染二维码。
#[derive(Debug, Clone, serde::Serialize)]
pub struct QuarkWebQrPayload {
    pub token: String,
    pub request_id: String,
    pub qrcode: String,
}

/// 轮询语义与 115 对齐：0 等待 / 2 已登录 / -1 失败或过期。
pub const QR_WAITING: i32 = 0;
pub const QR_CONFIRMED: i32 = 2;
pub const QR_FAILED: i32 = -1;

/// 生成 request_id：服务端只要求一个随机标识，不引入新依赖。
fn qr_request_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id() as u128;
    let mix = nanos ^ (pid << 64);
    format!(
        "{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}",
        (mix >> 96) as u32,
        ((mix >> 80) & 0xffff) as u16,
        ((mix >> 68) & 0x0fff) as u16,
        (0x8000 | ((mix >> 52) & 0x3fff)) as u16,
        (mix & 0xffff_ffff_ffff) as u64
    )
}

/// 二维码内容（手机夸克 App 认得的格式）。
///
/// 第 67 轮实测修正：**必须**带上 `ssb=weblogin` / `uc_param_str` / `uc_biz_str`
/// 三个参数（逐字对齐参考实现的抓包结论）。少了它们，手机夸克 App 扫到会直接判
/// "二维码已过期"，而服务端轮询永远停在"等待扫码"——本机看不到任何错误。
/// `uc_biz_str` 内含 `|@:`，用 `Url` 构造以保证百分号编码正确。
fn qr_content(token: &str) -> String {
    let mut url = reqwest::Url::parse(QR_BASE_URL).expect("静态 URL 应当合法");
    url.query_pairs_mut()
        .append_pair("token", token)
        .append_pair("client_id", QR_CLIENT_ID)
        .append_pair("ssb", "weblogin")
        .append_pair("uc_param_str", "")
        .append_pair(
            "uc_biz_str",
            "S:custom|OPT:SAREA@0|OPT:IMMERSIVE@1|OPT:BACK_BTN_STYLE@0",
        );
    url.to_string()
}

/// 从响应里取出所有 `Set-Cookie` 的 `k=v`（同名后者覆盖前者）。
fn collect_set_cookies(resp: &reqwest::blocking::Response) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for value in resp.headers().get_all(reqwest::header::SET_COOKIE) {
        let Ok(text) = value.to_str() else { continue };
        let Some(pair) = text.split(';').next() else {
            continue;
        };
        let pair = pair.trim();
        if let Some((name, value)) = pair.split_once('=') {
            if !name.is_empty() {
                out.push((name.to_string(), value.to_string()));
            }
        }
    }
    out
}

/// 合并 cookie（后者覆盖同名）。
fn merge_cookies(into: &mut Vec<(String, String)>, extra: Vec<(String, String)>) {
    for (name, value) in extra {
        if let Some(slot) = into.iter_mut().find(|(existing, _)| *existing == name) {
            slot.1 = value;
        } else {
            into.push((name, value));
        }
    }
}

/// 网页端登录后实际持有的 cookie 名（取自本机一份**曾经可用**的夸克 Cookie，第 67 轮），
/// 仅用于诊断"扫码流程少了哪些"——名字不是机密，值永不记录。
const WEB_LOGIN_COOKIE_NAMES: [&str; 15] = [
    "__kp",
    "__kps",
    "__ktd",
    "__pus",
    "__puus",
    "__sdid",
    "__uid",
    "_c_WBKFRo",
    "_UP_A4A_11_",
    "_UP_D_",
    "b-user-id",
    "CwsSessionId",
    "isg",
    "tfstk",
    "xlly_s",
];

fn cookies_to_string(cookies: &[(String, String)]) -> String {
    cookies
        .iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("; ")
}

/// 扫码流程的 cookie 累积（与参考实现的 `client.cookies.jar` 同义）：
/// 夸克在**每一步**以及**重定向**里都可能下发会话 cookie，只取最后一步会缺 cookie
/// ⇒ drive API 判"登录已过期"（第 67 轮实测用户扫码后就撞到这个）。
fn qr_cookie_jar() -> &'static std::sync::Mutex<std::collections::HashMap<String, Vec<(String, String)>>> {
    static JAR: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, Vec<(String, String)>>>,
    > = std::sync::OnceLock::new();
    JAR.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// 轮询时看到的 `service_ticket` 暂存（按 request_id）。
///
/// 为什么必须存：ticket 是**一次性**凭据。此前第三步为拿 ticket 又轮询了一次，
/// 第二遍就取不到了 ⇒ 用户看到"扫码尚未确认或已过期"（第 67 轮实测）。
fn qr_ticket_jar() -> &'static std::sync::Mutex<std::collections::HashMap<String, String>> {
    static TICKETS: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, String>>> =
        std::sync::OnceLock::new();
    TICKETS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// 从响应体里取 `data.members.service_ticket`（纯函数，便于单测）。
fn qr_ticket_of(body: &serde_json::Value) -> Option<String> {
    body.pointer("/data/members/service_ticket")
        .and_then(|v| v.as_str())
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string())
}

fn ticket_store(request_id: &str, ticket: String) {
    if let Ok(mut jar) = qr_ticket_jar().lock() {
        jar.insert(request_id.to_string(), ticket);
        if jar.len() > 64 {
            if let Some(key) = jar.keys().next().cloned() {
                jar.remove(&key);
            }
        }
    }
}

fn ticket_take(request_id: &str) -> Option<String> {
    qr_ticket_jar().lock().ok().and_then(|mut jar| jar.remove(request_id))
}

fn jar_merge(request_id: &str, cookies: Vec<(String, String)>) {
    if cookies.is_empty() {
        return;
    }
    if let Ok(mut jar) = qr_cookie_jar().lock() {
        let entry = jar.entry(request_id.to_string()).or_default();
        merge_cookies(entry, cookies);
        // 上限保护：异常流程不应无限增长。
        if jar.len() > 64 {
            if let Some(key) = jar.keys().next().cloned() {
                jar.remove(&key);
            }
        }
    }
}

fn jar_take(request_id: &str) -> Vec<(String, String)> {
    qr_cookie_jar()
        .lock()
        .ok()
        .and_then(|mut jar| jar.remove(request_id))
        .unwrap_or_default()
}

/// 第一步：获取二维码 token 与二维码内容。
pub fn web_qr_start() -> Result<QuarkWebQrPayload> {
    let client = http_client()?;
    let request_id = qr_request_id();
    let resp = client
        .get(QR_TOKEN_URL)
        .query(&[
            ("client_id", QR_CLIENT_ID),
            ("v", QR_API_VERSION),
            ("request_id", request_id.as_str()),
        ])
        .send()
        .context("请求夸克二维码失败")?;
    jar_merge(&request_id, collect_set_cookies(&resp));
    let body: serde_json::Value = resp.json().context("解析夸克二维码响应失败")?;
    let token = body
        .pointer("/data/members/token")
        .and_then(|v| v.as_str())
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string());
    let Some(token) = token else {
        let status = body.get("status").and_then(|v| v.as_i64()).unwrap_or(-1);
        let message = body.get("message").and_then(|v| v.as_str()).unwrap_or("");
        bail!("获取夸克二维码失败:status={status} {message}");
    };
    let qrcode = qr_content(&token);
    Ok(QuarkWebQrPayload {
        token,
        request_id,
        qrcode,
    })
}

fn qr_ticket_body(token: &str, request_id: &str) -> Result<serde_json::Value> {
    let client = http_client()?;
    let resp = client
        .get(QR_TICKET_URL)
        .query(&[
            ("client_id", QR_CLIENT_ID),
            ("v", QR_API_VERSION),
            ("token", token),
            ("request_id", request_id),
        ])
        .send()
        .context("查询夸克扫码状态失败")?;
    jar_merge(request_id, collect_set_cookies(&resp));
    resp.json().context("解析夸克扫码状态失败")
}

/// 状态映射（纯函数，便于单测）。
fn qr_status_of(body: &serde_json::Value) -> i32 {
    if qr_ticket_of(body).is_some() {
        return QR_CONFIRMED;
    }
    // 第 67 轮修正：**只有官方文档明示的失败码才算失败**，其余（含未知/缺字段）
    // 一律按"等待"处理。此前用 `_ => QR_FAILED` 把未枚举的等待码判成失败，
    // 界面立刻报"过期"并停掉轮询 ⇒ 用户即便在手机上确认也永远换不到 Cookie。
    match body.get("status").and_then(|v| v.as_i64()) {
        Some(50004002) | Some(50004003) | Some(50004004) => QR_FAILED,
        _ => QR_WAITING,
    }
}

/// 第二步：轮询扫码状态（0 等待 / 2 已登录 / -1 失败或过期）。
pub fn web_qr_poll(token: &str, request_id: &str) -> Result<i32> {
    let body = qr_ticket_body(token, request_id)?;
    if let Some(ticket) = qr_ticket_of(&body) {
        ticket_store(request_id, ticket);
    }
    let status = qr_status_of(&body);
    if status != QR_WAITING {
        crate::remote_scan::diag::note(&format!(
            "quark_qr_poll mapped={} raw_status={} message={}",
            status,
            body.get("status").and_then(|v| v.as_i64()).unwrap_or(-1),
            body.get("message").and_then(|v| v.as_str()).unwrap_or("")
        ));
    }
    Ok(status)
}

/// 第三步：扫码确认后换取 Cookie（`k=v; k2=v2`，末尾不带 `;`）。
pub fn web_qr_cookie(token: &str, request_id: &str) -> Result<String> {
    // 优先用轮询阶段已经拿到的 ticket（一次性凭据，不能重复去问）。
    let ticket = match ticket_take(request_id) {
        Some(ticket) => ticket,
        None => {
            let body = qr_ticket_body(token, request_id)?;
            qr_ticket_of(&body)
                .ok_or_else(|| anyhow!("扫码尚未确认或已过期，请重新扫码"))?
        }
    };
    // 第 67 轮修正：Cookie 必须**整条流程累积**（含各跳重定向），只取最后一步会缺
    // cookie ⇒ drive API 判"登录已过期"。这里手动跟重定向，逐跳收集 Set-Cookie。
    let client = reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .context("创建夸克登录客户端失败")?;
    let mut cookies = jar_take(request_id);
    let mut url = format!("{ACCOUNT_INFO_URL}?st={ticket}&lw=scan&platform=pc");
    for _ in 0..5 {
        let header = cookies_to_string(&cookies);
        let mut request = client.get(&url);
        if !header.is_empty() {
            request = request.header(COOKIE, header);
        }
        let resp = request.send().context("夸克登录换取 Cookie 失败")?;
        let hop_names: Vec<String> = collect_set_cookies(&resp)
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        crate::remote_scan::diag::note(&format!(
            "quark_qr_hop status={} set_cookie={}",
            resp.status().as_u16(),
            if hop_names.is_empty() {
                "(none)".to_string()
            } else {
                hop_names.join(",")
            }
        ));
        merge_cookies(&mut cookies, collect_set_cookies(&resp));
        let next = resp
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .map(|value| value.to_string());
        match next {
            Some(location) => {
                url = if location.starts_with("http") {
                    location
                } else if let Some(scheme_end) = url.find("://") {
                    let host_start = scheme_end + 3;
                    match url[host_start..].find('/') {
                        Some(relative) => {
                            format!("{}{}", &url[..host_start + relative], location)
                        }
                        None => format!("{url}{location}"),
                    }
                } else {
                    location
                };
            }
            None => break,
        }
    }
    // 脱敏诊断（第 67 轮）：只记录 **cookie 名字**与"相对网页端参考集缺哪些"，
    // 绝不记录值；用于定位"扫码成功但仍报登录已过期"。
    let mut names: Vec<String> = cookies.iter().map(|(name, _)| name.clone()).collect();
    names.sort();
    let missing: Vec<&str> = WEB_LOGIN_COOKIE_NAMES
        .iter()
        .copied()
        .filter(|name| !names.iter().any(|have| have == name))
        .collect();
    crate::remote_scan::diag::note(&format!(
        "quark_qr_cookies count={} names={} missing={}",
        names.len(),
        names.join(","),
        if missing.is_empty() {
            "(none)".to_string()
        } else {
            missing.join(",")
        }
    ));
    if cookies.is_empty() {
        bail!("夸克未返回会话 Cookie，请重新扫码");
    }
    Ok(cookies_to_string(&cookies))
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

    #[test]
    fn qr_status_maps_waiting_confirmed_and_failed() {
        let waiting: serde_json::Value =
            serde_json::json!({"status": 2000000, "data": {"members": {}}});
        assert_eq!(super::qr_status_of(&waiting), super::QR_WAITING);
        let scanning: serde_json::Value = serde_json::json!({"status": 50004001});
        assert_eq!(super::qr_status_of(&scanning), super::QR_WAITING);
        let confirmed: serde_json::Value = serde_json::json!({
            "status": 2000000,
            "data": {"members": {"service_ticket": "st-123"}}
        });
        assert_eq!(super::qr_status_of(&confirmed), super::QR_CONFIRMED);
        let failed: serde_json::Value = serde_json::json!({"status": 50004003});
        assert_eq!(super::qr_status_of(&failed), super::QR_FAILED);
        let expired: serde_json::Value = serde_json::json!({"status": 50004004});
        assert_eq!(super::qr_status_of(&expired), super::QR_FAILED);
        // 未枚举的状态/缺字段必须按"等待"处理（第 67 轮修正：错判成失败会停掉轮询）
        let unknown: serde_json::Value = serde_json::json!({"status": 50004009});
        assert_eq!(super::qr_status_of(&unknown), super::QR_WAITING);
        let no_status: serde_json::Value = serde_json::json!({"message": "ok"});
        assert_eq!(super::qr_status_of(&no_status), super::QR_WAITING);
    }

    /// 二维码内容必须是手机夸克 App 认得的格式（第 66 轮从参考实现核对）：
    /// `https://su.quark.cn/4_eMHBJ?token=<token>&client_id=532&v=1.2`
    #[test]
    fn qr_content_matches_the_quark_app_format() {
        let url = super::qr_content("tok-abc");
        let parsed = reqwest::Url::parse(&url).unwrap();
        assert_eq!(parsed.scheme(), "https");
        assert_eq!(parsed.host_str().unwrap(), "su.quark.cn");
        assert_eq!(parsed.path(), "/4_eMHBJ");
        let pairs: std::collections::HashMap<String, String> =
            parsed.query_pairs().into_owned().collect();
        assert_eq!(pairs.get("token").map(String::as_str), Some("tok-abc"));
        assert_eq!(pairs.get("client_id").map(String::as_str), Some("532"));
        // 第 67 轮：这三个参数缺一个，手机端就判"二维码已过期"
        assert_eq!(pairs.get("ssb").map(String::as_str), Some("weblogin"));
        assert_eq!(pairs.get("uc_param_str").map(String::as_str), Some(""));
        assert_eq!(
            pairs.get("uc_biz_str").map(String::as_str),
            Some("S:custom|OPT:SAREA@0|OPT:IMMERSIVE@1|OPT:BACK_BTN_STYLE@0")
        );
    }

    #[test]
    fn qr_ticket_is_extracted_only_when_present() {
        let with: serde_json::Value =
            serde_json::json!({"data": {"members": {"service_ticket": "st-1"}}});
        assert_eq!(super::qr_ticket_of(&with).as_deref(), Some("st-1"));
        let empty: serde_json::Value =
            serde_json::json!({"data": {"members": {"service_ticket": ""}}});
        assert_eq!(super::qr_ticket_of(&empty), None);
        let missing: serde_json::Value = serde_json::json!({"data": {"members": {}}});
        assert_eq!(super::qr_ticket_of(&missing), None);
    }

    #[test]
    fn cookie_merge_prefers_the_latest_value_and_keeps_order() {
        let mut cookies = vec![("a".to_string(), "1".to_string())];
        super::merge_cookies(
            &mut cookies,
            vec![("b".to_string(), "2".to_string()), ("a".to_string(), "9".to_string())],
        );
        assert_eq!(super::cookies_to_string(&cookies), "a=9; b=2");
    }

    #[test]
    fn qr_request_id_looks_like_a_uuid() {
        let id = super::qr_request_id();
        assert_eq!(id.len(), 36, "uuid 形态: {id}");
        assert_eq!(id.chars().filter(|c| *c == '-').count(), 4);
    }
}
