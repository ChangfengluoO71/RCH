//! EH 订阅（可选插件）：按可编辑规则筛选 E-Hentai 画廊种子并保存到指定文件夹。
//!
//! 设计边界（与 SPEC §12「不做在线漫画站聚合」并存的插件边界，见
//! `docs/research/eh-subscription-115-feasibility.md` §5.1）：
//! * **只读来源**：仅访问 EH 元数据接口与 `.torrent` 文件；绝不读取图片、不解析压缩包内容。
//! * **资源输出边界**：种子与 manifest 只写入用户指定的 `<out_dir>`；运行设置与可选 Daily
//!   状态保存在既有应用支持目录 `eh_subscription_rules.json` 配置中，不触碰阅读数据。
//! * **默认关闭**：手动点击收集；Daily 仅在用户显式启用后、Windows 应用运行期间调度。
//!
//! 所有接口契约均来自 2026-09-22 的实测（探针 `examples/eh_subscription_probe.rs`）：
//! * gdata 必须 **POST JSON**：`{"method":"gdata","gidlist":[[gid,token],...],"namespace":1}`；
//!   响应外层是 `{"gmetadata":[...]}`。
//! * 数字字段类型不统一：`rating` / `torrentcount` / `filecount` / `posted` 是**字符串**，
//!   `filesize` 是数字 → 统一走 [`int_field`] / [`text_field`] 解析。
//! * `title` 是罗马字，`title_jpn` 才是日文原名，也才是与种子名可比对的字段。
//! * 搜索分页游标在页面脚本变量 `nexturl`（`next=<本页最后一个 gid>`），**不是** `?page=N`。
//! * 种子页 `gallerytorrents.php?gid=&t=` 内文本含 `Seeds: n Peers: n Downloads: n`。
//! * 种子直链 `https://ehtracker.org/get/<gid>/<infohash>.torrent`，无需 Referer。

use std::collections::HashSet;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
#[cfg(windows)]
use sha2::{Digest as _, Sha256};

use crate::eh_tag_translation;

pub const DEFAULT_HOST: &str = "e-hentai.org";
/// 页数安全上限。单查询总量约 440 页；这里给足够大的上限，避免误操作把一轮跑成几小时。
pub const MAX_PAGES: usize = 500;
const MINIMUM_RATING: f64 = 4.4;
const MINIMUM_STABLE_AGE_SECS: i64 = 3 * 24 * 60 * 60;
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                  (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";

// ---------------------------------------------------------------------------
// 规则（与前端 JSON 同构，字段名即用户可编辑的键）
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AgeTier {
    /// 画龄下限（年）。按此值从大到小匹配，取第一个满足的档位。
    pub min_age_years: f64,
    /// 该档位要求的种子下载数下限。
    pub min_downloads: i64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EhProxyMode {
    System,
    Direct,
    Manual,
}

impl Default for EhProxyMode {
    fn default() -> Self {
        Self::System
    }
}

impl EhProxyMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Direct => "direct",
            Self::Manual => "manual",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct EhNetworkSettings {
    pub mode: EhProxyMode,
    pub proxy_url: String,
}

impl Default for EhNetworkSettings {
    fn default() -> Self {
        Self {
            mode: EhProxyMode::System,
            proxy_url: String::new(),
        }
    }
}

impl EhNetworkSettings {
    fn validate(&self) -> Result<(), String> {
        if self.mode != EhProxyMode::Manual {
            return Ok(());
        }

        let raw = self.proxy_url.trim();
        let url = reqwest::Url::parse(raw).map_err(|_| "手动代理 URL 无效".to_string())?;
        if !matches!(url.scheme(), "http" | "https" | "socks5" | "socks5h")
            || url.host_str().is_none()
            || url.port().is_none_or(|port| port == 0)
            || (url.path() != "" && url.path() != "/")
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err("手动代理 URL 无效：需要 HTTP(S) 或 SOCKS5 代理及有效端口".into());
        }
        reqwest::Proxy::all(raw).map_err(|_| "手动代理 URL 无效".to_string())?;
        Ok(())
    }
}

/// EH 插件的可选 Daily 设置与最近一次运行记录，随既有规则配置文件保存。
/// 旧规则文件缺少此字段时保持关闭，Rust 规则往返序列化也会保留该状态。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct EhAutomationSettings {
    pub daily_enabled: bool,
    pub last_auto_attempt_date: Option<String>,
    pub last_run: Option<EhAutomationRun>,
}

impl Default for EhAutomationSettings {
    fn default() -> Self {
        Self {
            daily_enabled: false,
            last_auto_attempt_date: None,
            last_run: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct EhAutomationRun {
    /// Local calendar date captured when this run was claimed.
    pub date: String,
    /// `manual` or `daily_auto`.
    pub source: String,
    /// `claimed`, `running`, `completed`, `failed`, `cancelled`, or `interrupted_unknown`.
    pub status: String,
    pub saved: usize,
    pub error: Option<String>,
}

impl Default for EhAutomationRun {
    fn default() -> Self {
        Self {
            date: String::new(),
            source: "manual".into(),
            status: "failed".into(),
            saved: 0,
            error: None,
        }
    }
}

impl Default for AgeTier {
    fn default() -> Self {
        Self {
            min_age_years: 0.0,
            min_downloads: 0,
        }
    }
}

/// 订阅筛选规则。整体可序列化为 JSON 供用户直接编辑。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct EhRules {
    /// One-time migration marker for the rating and age-tier policy.
    pub filter_policy_version: u32,
    /// EH 搜索语法，例如 `language:chinese$ uncensored`、`other:"full color"$`。
    pub search: String,
    /// 评分下限。注意 EH 搜索**不支持** `rating>=4`，只能在本地过滤。
    pub min_rating: f64,
    /// 主标题标记白名单（`|` 分隔；`any` = 不筛）。
    /// 实测 `[Digital]` / `[DL版]` 出现在主标题里而不是标签里。
    pub title_markers: String,
    /// 命中即排除的标题标记（例如 AI 生成作品）。
    pub exclude_markers: Vec<String>,
    /// 分时间段下载数要求：时间越久要求越高。
    pub age_tiers: Vec<AgeTier>,
    /// 种子保存目录。
    pub out_dir: String,
    /// 每轮扫描的搜索页数（每页 25 条；跟随 `nexturl` 翻页）。
    ///
    /// 上限只受站点与时间成本约束：单查询总量约 1.1 万条 ≈ 440 页。
    /// 每页成本 ≈ 1 次搜索 + 1 次 gdata 批量 + 每个通过者 1 次种子页，按默认间隔 2.5s，
    /// **每页约 40~60 秒**，因此页数是"覆盖面 ↔ 耗时"的直接权衡。
    /// 代码侧仅做安全上限（[`MAX_PAGES`]），不设人为的小上限。
    pub pages: usize,
    /// 主站域名。默认 `e-hentai.org`；某些网络环境下需要配合代理或改用可访问的镜像域名。
    /// 注意：种子直链固定在 `ehtracker.org`，改这里不影响种子下载。
    pub host: String,
    /// 请求间隔秒数（下限 0.5）。EH 对高频请求有限流，不要调得过小。
    pub request_interval_secs: f64,
    /// 仅用于验证分档逻辑：把"当前时间"往后偏移 N 天再算画龄。正常使用保持 0。
    pub now_offset_days: i64,
    /// EH main-site and Tracker requests share this explicit network policy.
    pub network: EhNetworkSettings,
    /// Opt-in scheduler state; excluded from EH filtering decisions.
    pub automation: EhAutomationSettings,
}

impl Default for EhRules {
    fn default() -> Self {
        Self {
            filter_policy_version: 1,
            search: "language:chinese$ uncensored".into(),
            min_rating: MINIMUM_RATING,
            title_markers: "Digital|DL版|DL".into(),
            exclude_markers: vec!["AI Generated".into()],
            // Trial ladder: items up to and including 3 days old are rejected
            // before this ladder is consulted. Just over 3 days starts at 20.
            age_tiers: vec![
                AgeTier {
                    min_age_years: 5.0,
                    min_downloads: 2000,
                },
                AgeTier {
                    min_age_years: 2.0,
                    min_downloads: 900,
                },
                AgeTier {
                    min_age_years: 0.5,
                    min_downloads: 500,
                },
                AgeTier {
                    min_age_years: 30.0 / 365.25,
                    min_downloads: 300,
                },
                AgeTier {
                    min_age_years: 14.0 / 365.25,
                    min_downloads: 200,
                },
                AgeTier {
                    min_age_years: 7.0 / 365.25,
                    min_downloads: 100,
                },
                AgeTier {
                    min_age_years: 3.0 / 365.25,
                    min_downloads: 20,
                },
            ],
            out_dir: String::new(),
            pages: 10,
            host: DEFAULT_HOST.into(),
            request_interval_secs: 2.5,
            now_offset_days: 0,
            network: EhNetworkSettings::default(),
            automation: EhAutomationSettings::default(),
        }
    }
}

impl EhRules {
    pub fn from_json(text: &str) -> Result<Self, String> {
        let rules: Self =
            serde_json::from_str(text).map_err(|e| format!("规则 JSON 解析失败：{e}"))?;
        rules.network.validate()?;
        Ok(rules)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".into())
    }

    /// 按画龄选出该档要求的下载数下限。
    pub fn required_dl(&self, age_years: f64) -> i64 {
        let mut tiers = self.age_tiers.clone();
        tiers.sort_by(|a, b| {
            b.min_age_years
                .partial_cmp(&a.min_age_years)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        tiers
            .iter()
            .find(|t| age_years >= t.min_age_years)
            .or_else(|| tiers.last())
            .map(|t| t.min_downloads)
            .unwrap_or(0)
    }

    /// 标题标记判定：先看排除名单，再看白名单。
    pub fn marker_ok(&self, title: &str, title_jpn: &str) -> bool {
        let hay = format!("{title} {title_jpn}").to_lowercase();
        if self
            .exclude_markers
            .iter()
            .filter(|m| !m.trim().is_empty())
            .any(|m| hay.contains(&m.to_lowercase()))
        {
            return false;
        }
        if self.title_markers.trim().is_empty()
            || self.title_markers.trim().eq_ignore_ascii_case("any")
        {
            return true;
        }
        has_marker(title, title_jpn, &self.title_markers)
    }

    fn interval(&self) -> Duration {
        Duration::from_secs_f64(self.request_interval_secs.clamp(0.5, 60.0))
    }
}

/// 主标题标记判定（`[marker]` 形式，大小写不敏感）。
pub fn has_marker(title: &str, title_jpn: &str, markers: &str) -> bool {
    let hay = format!("{title} {title_jpn}").to_lowercase();
    markers
        .split('|')
        .map(|m| m.trim().to_lowercase())
        .filter(|m| !m.is_empty())
        .any(|m| hay.contains(&format!("[{m}]")))
}

// ---------------------------------------------------------------------------
// 运行状态（供 UI 轮询进度）
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize)]
pub struct EhProgress {
    pub running: bool,
    /// `idle` | `searching` | `metadata` | `filtering` | `probing` | `done` | `failed`
    pub stage: String,
    pub message: String,
    pub page: usize,
    pub pages: usize,
    pub candidates: usize,
    /// Metadata candidates visited so far (includes the item currently being processed).
    pub processed: usize,
    /// Number of metadata rows available for local filtering once each batch returns.
    pub work_total: usize,
    pub checked: usize,
    pub saved: usize,
    /// 拒绝分项（规则可编辑，必须能看出是哪条规则挡住的）
    pub no_rating: usize,
    pub no_marker: usize,
    pub no_torrent: usize,
    pub no_downloads: usize,
    /// Rejected before any torrent-stat request because the gallery is <= 3 days old.
    pub too_recent: usize,
    pub unmapped: usize,
    /// Unix milliseconds when the currently displayed activity began.
    pub activity_started_at_unix_ms: u128,
    pub error: Option<String>,
}

static RUNNING: AtomicBool = AtomicBool::new(false);
static PROGRESS: std::sync::Mutex<Option<EhProgress>> = std::sync::Mutex::new(None);

fn set_progress(p: EhProgress) {
    if let Ok(mut slot) = PROGRESS.lock() {
        *slot = Some(p);
    }
}

pub fn progress() -> EhProgress {
    PROGRESS
        .lock()
        .ok()
        .and_then(|s| s.clone())
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// HTTP / HTML 小工具
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct Client {
    http: reqwest::blocking::Client,
    host: String,
    interval: Duration,
}

impl Client {
    fn new(host: &str, interval: Duration, network: &EhNetworkSettings) -> Result<Self, String> {
        Self::new_with_timeout(host, interval, network, Duration::from_secs(30))
    }

    fn new_with_timeout(
        host: &str,
        interval: Duration,
        network: &EhNetworkSettings,
        timeout: Duration,
    ) -> Result<Self, String> {
        let mut builder = reqwest::blocking::Client::builder()
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::none());
        network.validate()?;
        builder = match network.mode {
            EhProxyMode::System => builder,
            EhProxyMode::Direct => builder.no_proxy(),
            EhProxyMode::Manual => {
                let proxy = reqwest::Proxy::all(network.proxy_url.trim())
                    .map_err(|_| "手动代理配置无效".to_string())?;
                builder.proxy(proxy)
            }
        };
        let http = builder
            .build()
            .map_err(|_| "创建 EH HTTP 客户端失败".to_string())?;
        Ok(Self {
            http,
            host: host.to_string(),
            interval,
        })
    }

    fn get(&self, url: &str) -> Result<String, String> {
        let resp = self
            .http
            .get(url)
            .header("User-Agent", UA)
            .send()
            .map_err(|e| safe_network_error("EH 请求", &e))?;
        let status = resp.status().as_u16();
        let body = resp
            .text()
            .map_err(|e| safe_network_error("EH 响应读取", &e))?;
        if status == 403 || status == 503 || is_challenge(&body) {
            return Err(format!("EH 返回 {status}（可能被限流或需要 Cookie）"));
        }
        if status >= 400 {
            return Err(format!("EH 返回 {status}"));
        }
        Ok(body)
    }

    fn get_bytes(&self, url: &str) -> Result<Vec<u8>, String> {
        let resp = self
            .http
            .get(url)
            .header("User-Agent", UA)
            .send()
            .map_err(|e| safe_network_error("Torrent 请求", &e))?;
        let status = resp.status().as_u16();
        if status >= 400 {
            return Err(format!("下载返回 {status}"));
        }
        resp.bytes()
            .map(|b| b.to_vec())
            .map_err(|e| safe_network_error("Torrent 响应读取", &e))
    }

    fn get_probe_response(&self, url: &str) -> Result<ProbeResponse, String> {
        use std::io::Read;

        let response = self
            .http
            .get(url)
            .header("User-Agent", UA)
            .send()
            .map_err(|e| safe_network_error("诊断请求", &e))?;
        let status = response.status().as_u16();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_string();
        let mut limited = response.take(1024 * 1024);
        let mut bytes = Vec::new();
        limited
            .read_to_end(&mut bytes)
            .map_err(|_| "诊断响应读取失败".to_string())?;
        Ok(ProbeResponse {
            status,
            content_type,
            body: String::from_utf8_lossy(&bytes).into_owned(),
        })
    }

    /// 实测契约：POST JSON，`gidlist` 为 `[[gid, token], ...]`，一次最多 25 项。
    fn gdata(&self, pairs: &[(String, String)]) -> Result<Vec<serde_json::Value>, String> {
        if pairs.is_empty() {
            return Ok(Vec::new());
        }
        let list: Vec<Vec<String>> = pairs
            .iter()
            .map(|(g, t)| vec![g.clone(), t.clone()])
            .collect();
        let payload = serde_json::json!({
            "method": "gdata",
            "gidlist": list,
            "namespace": 1,
        })
        .to_string();
        let resp = self
            .http
            .post(format!("https://{}/api.php", self.host))
            .header("User-Agent", UA)
            .header("Content-Type", "application/json")
            .body(payload)
            .send()
            .map_err(|e| safe_network_error("gdata 请求", &e))?;
        let body = resp
            .text()
            .map_err(|e| safe_network_error("gdata 响应读取", &e))?;
        let value: serde_json::Value =
            serde_json::from_str(&body).map_err(|e| format!("gdata 非 JSON：{e}"))?;
        if let Some(err) = value.get("error").and_then(|e| e.as_str()) {
            return Err(format!("gdata 返回错误：{err}"));
        }
        Ok(value
            .get("gmetadata")
            .and_then(|m| m.as_array())
            .cloned()
            .unwrap_or_default())
    }
}

fn safe_network_error(action: &str, error: &reqwest::Error) -> String {
    if error.is_timeout() {
        format!("{action}超时")
    } else if error.is_connect() {
        format!("{action}连接失败（DNS、代理、TCP 或 TLS）")
    } else {
        format!("{action}失败")
    }
}

fn is_challenge(body: &str) -> bool {
    let body = body.to_ascii_lowercase();
    body.contains("cf-challenge")
        || body.contains("just a moment")
        || body.contains("<title>attention required")
}

/// 多词标签必须写成 `other:"high resolution"$`；裸词不带 `$` 实测会返回无关结果。
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// gdata 的数字字段类型不统一（字符串/数字皆有），统一在此兼容。
pub fn int_field(v: &serde_json::Value, key: &str) -> Option<i64> {
    match v.get(key)? {
        serde_json::Value::String(s) => s.trim().parse::<i64>().ok(),
        serde_json::Value::Number(n) => n.as_i64(),
        _ => None,
    }
}

pub fn text_field<'a>(v: &'a serde_json::Value, key: &str) -> &'a str {
    v.get(key).and_then(|x| x.as_str()).unwrap_or("")
}

pub fn rating_of(v: &serde_json::Value) -> f64 {
    match v.get("rating") {
        Some(serde_json::Value::String(s)) => s.parse().unwrap_or(-1.0),
        Some(serde_json::Value::Number(n)) => n.as_f64().unwrap_or(-1.0),
        _ => -1.0,
    }
}

/// `posted` 实测是字符串；解析失败返回 None，**绝不静默当作 0**
/// （那会让画龄恒为 0，分档阈值全部失效——这是探针阶段踩过的坑）。
pub fn posted_of(v: &serde_json::Value) -> Option<i64> {
    int_field(v, "posted").filter(|t| *t > 0)
}

fn required_downloads_for_posted(rules: &EhRules, posted: Option<i64>, now: i64) -> Option<i64> {
    let posted = posted.filter(|timestamp| *timestamp > 0)?;
    let age_seconds = now.saturating_sub(posted);
    if age_seconds <= MINIMUM_STABLE_AGE_SECS {
        return None;
    }
    let age_years = age_seconds as f64 / 365.25 / 86400.0;
    Some(rules.required_dl(age_years))
}

/// unix 秒 → `YYYY-MM-DD HH:MM UTC`（手算 civil date，不引额外依赖）。
pub fn fmt_utc(secs: i64) -> String {
    if secs <= 0 {
        return "未知".into();
    }
    let days = secs / 86400;
    let rem = secs % 86400;
    let (h, mi) = (rem / 3600, (rem % 3600) / 60);
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02} {h:02}:{mi:02} UTC")
}

// ---------------------------------------------------------------------------
// 页面解析
// ---------------------------------------------------------------------------

/// 从搜索结果页取 `(画廊绝对 URL, 标题)` 去重列表。
pub fn parse_gallery_pairs(html: &str, host: &str) -> Vec<(String, String)> {
    let needle = format!("https://{host}/g/");
    let mut out: Vec<(String, String)> = Vec::new();
    let mut rest = html;
    while let Some(i) = rest.find(&needle) {
        rest = &rest[i..];
        let raw: String = rest
            .chars()
            .take_while(|c| *c != '"' && *c != '\'' && !c.is_whitespace())
            .collect();
        if parse_gid_token(&raw).is_some() {
            let after = &rest[raw.len().min(rest.len())..];
            let title = between(after, ">", "</a>")
                .map(|s| strip_tags(&s))
                .unwrap_or_default();
            let url = raw.replace("&amp;", "&");
            if !out.iter().any(|(u, _)| u == &url) {
                out.push((url, title));
            }
        }
        if raw.is_empty() {
            break;
        }
        rest = &rest[raw.len().min(rest.len())..];
    }
    out
}

/// 分页游标（实测在 JS 变量 `nexturl` 里，页面上没有 `<a>Next</a>`）。
pub fn parse_next_page(html: &str, host: &str) -> Option<String> {
    let href = extract_next_href(html)?;
    let href = href.replace("&amp;", "&").replace("\\/", "/");
    if href.is_empty() {
        None
    } else if href.starts_with("http") {
        Some(href)
    } else {
        Some(format!("https://{host}{href}"))
    }
}

fn extract_next_href(html: &str) -> Option<String> {
    if let Some(after) = html.split("nexturl=\"").nth(1) {
        if let Some(end) = after.find('"') {
            let url = after[..end].trim();
            if !url.is_empty() {
                return Some(url.to_string());
            }
        }
    }
    if let Some(seg) = between(html, "id=\"next\"", "</a>") {
        if let Some(href) = between(&seg, "href=\"", "\"") {
            return Some(href);
        }
    }
    let lower = html.to_lowercase();
    let pos = lower.find(">next<")?;
    let start = html[..pos].rfind("href=\"")?;
    let rest = &html[start + 6..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// 解析 `gid` 与 `token`（URL 形如 `/g/<gid>/<token>-<title>/`）。
pub fn parse_gid_token(url: &str) -> Option<(String, String)> {
    let i = url.find("/g/")? + 3;
    let rest = &url[i..];
    let mut parts = rest.split('/');
    let gid = parts.next()?.to_string();
    let hash = parts.next()?;
    let token = hash.split('-').next()?.to_string();
    if gid.chars().all(|c| c.is_ascii_digit()) && token.len() >= 8 {
        Some((gid, token))
    } else {
        None
    }
}

/// 种子页统计：实测文本形如 `Seeds: 124 Peers: 4 Downloads: 924`。
pub fn parse_torrent_stats(html: &str) -> Vec<Option<(i64, i64, i64)>> {
    let flat = strip_tags(html);
    let mut out = Vec::new();
    let mut rest = flat.as_str();
    while let Some(i) = rest.find("Seeds:") {
        rest = &rest[i..];
        let block_end = rest[6..]
            .find("Seeds:")
            .map(|next| next + 6)
            .unwrap_or(rest.len());
        let block = &rest[..block_end];
        let stats = parse_stat_value(block, "Seeds:")
            .zip(parse_stat_value(block, "Peers:"))
            .zip(parse_stat_value(block, "Downloads:"))
            .map(|((seeds, peers), downloads)| (seeds, peers, downloads));
        out.push(stats);
        rest = &rest[block_end..];
    }
    out
}

fn parse_stat_value(block: &str, label: &str) -> Option<i64> {
    let value = block.split_once(label)?.1.trim_start();
    let digits: String = value.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        None
    } else {
        digits.parse().ok()
    }
}

fn between(hay: &str, start: &str, end: &str) -> Option<String> {
    let i = hay.find(start)? + start.len();
    let rest = &hay[i..];
    let j = rest.find(end)?;
    Some(rest[..j].to_string())
}

fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#039;", "'")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&nbsp;", " ")
        .trim()
        .to_string()
}

/// 归一化：只留字母数字并小写（用于 `title_jpn` 与种子名比对）。
pub fn normalize_for_match(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// 映射判定：**必须用 `title_jpn`**（罗马字 `title` 实测全部失败）。
pub fn maps_to_gallery(title_jpn: &str, torrent_name: &str, files: &[String]) -> bool {
    let needle = normalize_for_match(title_jpn);
    if needle.is_empty() {
        return false;
    }
    let mut hay = normalize_for_match(torrent_name);
    for f in files {
        hay.push_str(&normalize_for_match(f));
    }
    let strong: String = needle.chars().take(24).collect();
    let weak: String = needle.chars().take(12).collect();
    hay.contains(&strong) || (weak.chars().count() >= 8 && hay.contains(&weak))
}

/// 文件名安全化：先解 HTML 实体（gdata 标题里带 `&#039;`），再替换非法字符。
pub fn sanitize_filename(s: &str) -> String {
    let decoded = s
        .replace("&#039;", "'")
        .replace("&#39;", "'")
        .replace("&quot;", "\"")
        .replace("&amp;", "&")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">");
    let cleaned: String = decoded
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if (c as u32) < 0x20 => '_',
            c => c,
        })
        .collect();
    let trimmed = cleaned.trim().trim_end_matches('.').to_string();
    if trimmed.chars().count() > 80 {
        trimmed.chars().take(80).collect()
    } else if trimmed.is_empty() {
        "untitled".into()
    } else {
        trimmed
    }
}

// ---------------------------------------------------------------------------
// 落盘清单
// ---------------------------------------------------------------------------

/// 创作者条目（与刮削的 `CreatorCandidate` 同构：角色 + 名字）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EhCreator {
    /// `artist` | `group`
    pub role: String,
    pub name: String,
}

/// 与刮削产出对齐的语义层。
///
/// 为什么单列一层而不是平铺到 `EhSavedItem`：
/// 刮削的同类字段在 `proposal.semantic` 子对象里（`work_title` / `creators` /
/// `source_series` / `resource_language` / `censorship` / `color_state` …），
/// 这里保持**同构嵌套**，未来做元数据导入时两边字段可直接对齐，不需要再写一层翻译。
/// 全部字段 `#[serde(default)]`，旧 manifest 缺这一层也能正常反序列化。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct EhSemantic {
    /// 作品名（取 gdata 的 `title_jpn`；罗马字 `title` 进 `title_aliases`）。
    pub work_title: String,
    /// 别名（罗马字标题等），供匹配时兜底。
    pub title_aliases: Vec<String>,
    /// 创作者：由 `artist:*` / `group:*` 命名空间拆出。
    pub creators: Vec<EhCreator>,
    /// 系列/原作：`parody:*`。
    pub source_series: Vec<String>,
    /// 角色：`character:*`。
    pub characters: Vec<String>,
    /// 资源语言：`language:*` 中第一个非 `translated` 的值（如 `chinese`）。
    pub resource_language: Option<String>,
    /// 翻译状态：命中 `language:translated` 时为 `translated`。
    pub translation_state: Option<String>,
    /// 修正状态：`other:uncensored` → `uncensored`。
    pub censorship: Option<String>,
    /// 彩色状态：`other:full color` → `full color`。
    pub color_state: Option<String>,
    /// 其余内容/资源标签（`male:*` / `female:*` / `mixed:*` / `other:*` / `reclass:*`）。
    pub resource_tags: Vec<String>,
    /// 对应 `resource_tags` 的中文译名（数量与顺序一致；无译名时该位为空串）。
    pub resource_tags_zh: Vec<String>,
}

/// 拆分 gdata 标签：解析 `命名空间:值`。
fn split_tag(tag: &str) -> (&str, &str) {
    match tag.split_once(':') {
        Some((ns, v)) => (ns.trim(), v.trim()),
        None => ("", tag.trim()),
    }
}

/// 从 gdata 条目推导语义层。
///
/// `cache_dir` 用于翻译层的按需更新缓存；`allow_network_update` 为真时，
/// 遇到基线里没有的标签会尝试拉一次上游（见 `eh_tag_translation`）。
pub fn derive_semantic(
    item: &serde_json::Value,
    allow_network_update: bool,
    cache_dir: &str,
) -> EhSemantic {
    let title_jpn = text_field(item, "title_jpn").to_string();
    let title = text_field(item, "title").to_string();
    let mut sem = EhSemantic {
        work_title: if title_jpn.is_empty() {
            title.clone()
        } else {
            title_jpn.clone()
        },
        ..Default::default()
    };
    if !title.is_empty() && title != sem.work_title {
        sem.title_aliases.push(title);
    }

    let tags: Vec<String> = item
        .get("tags")
        .and_then(|t| t.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    for tag in &tags {
        let (ns, value) = split_tag(tag);
        match ns {
            "artist" | "group" => sem.creators.push(EhCreator {
                role: ns.to_string(),
                name: value.to_string(),
            }),
            "parody" => sem.source_series.push(value.to_string()),
            "character" => sem.characters.push(value.to_string()),
            "language" => {
                let lower = value.to_lowercase();
                if lower == "translated" {
                    sem.translation_state = Some("translated".into());
                } else if sem.resource_language.is_none() {
                    sem.resource_language = Some(lower);
                }
            }
            "other" => match value.to_lowercase().as_str() {
                "uncensored" => sem.censorship = Some("uncensored".into()),
                "full color" => sem.color_state = Some("full color".into()),
                _ => sem.resource_tags.push(tag.clone()),
            },
            "male" | "female" | "mixed" | "reclass" => sem.resource_tags.push(tag.clone()),
            _ => {
                if !value.is_empty() {
                    sem.resource_tags.push(tag.clone());
                }
            }
        }
    }

    // 中文译名：与 resource_tags 一一对应，缺译名留空串（保持下标对齐）。
    for tag in &sem.resource_tags {
        let (ns, value) = split_tag(tag);
        let zh =
            eh_tag_translation::translate_with_update(ns, value, allow_network_update, cache_dir)
                .unwrap_or_default();
        sem.resource_tags_zh.push(zh);
    }

    sem
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EhSavedItem {
    pub infohash: String,
    pub gid: String,
    pub title: String,
    pub title_jpn: String,
    pub rating: f64,
    pub downloads: i64,
    pub required_dl: i64,
    pub posted: Option<i64>,
    pub posted_utc: String,
    pub age_years: Option<f64>,
    pub tags: Vec<String>,
    pub category: String,
    pub filecount: Option<i64>,
    pub filesize: Option<i64>,
    pub uploader: String,
    pub torrent_name: String,
    pub file: String,
    pub bytes: i64,
    pub source_url: String,
    pub saved_at_utc: String,
    /// 与刮削产出对齐的语义层（v2 新增；旧 manifest 无此字段时取默认值）。
    #[serde(default)]
    pub semantic: EhSemantic,
}

pub fn manifest_path(out_dir: &str) -> PathBuf {
    Path::new(out_dir).join("manifest.json")
}

/// 读取已保存清单（文件不存在或损坏时返回空列表，不报错）。
pub fn read_manifest(out_dir: &str) -> Vec<EhSavedItem> {
    if out_dir.trim().is_empty() {
        return Vec::new();
    }
    std::fs::read_to_string(manifest_path(out_dir))
        .ok()
        .and_then(|s| serde_json::from_str::<Vec<EhSavedItem>>(&s).ok())
        .unwrap_or_default()
}

struct ManifestSnapshot {
    items: Vec<EhSavedItem>,
    /// Keep unknown fields from older/future compatible records intact on rewrite.
    rows: Vec<serde_json::Value>,
}

impl ManifestSnapshot {
    fn empty() -> Self {
        Self {
            items: Vec::new(),
            rows: Vec::new(),
        }
    }
}

#[derive(Debug)]
enum ManifestReadError {
    Unreadable(std::io::Error),
    Damaged(serde_json::Error),
}

impl std::fmt::Display for ManifestReadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreadable(error) => write!(formatter, "manifest unreadable: {error}"),
            Self::Damaged(error) => write!(formatter, "manifest damaged: {error}"),
        }
    }
}

fn load_manifest_for_collection(out_dir: &Path) -> Result<ManifestSnapshot, ManifestReadError> {
    let root = out_dir
        .canonicalize()
        .map_err(ManifestReadError::Unreadable)?;
    let path = out_dir.join("manifest.json");
    match std::fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ManifestSnapshot::empty())
        }
        Err(error) => return Err(ManifestReadError::Unreadable(error)),
        Ok(_) => {}
    }

    let canonical = path.canonicalize().map_err(ManifestReadError::Unreadable)?;
    if !canonical.starts_with(&root) {
        return Err(ManifestReadError::Unreadable(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "manifest path escapes output directory",
        )));
    }
    match std::fs::metadata(&canonical) {
        Ok(metadata) if metadata.is_file() => {}
        Ok(_) => {
            return Err(ManifestReadError::Unreadable(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "manifest is not a regular file",
            )))
        }
        Err(error) => return Err(ManifestReadError::Unreadable(error)),
    }

    load_manifest_from_reader(&canonical, |path| std::fs::read(path))
}

fn load_manifest_from_reader<F>(path: &Path, read: F) -> Result<ManifestSnapshot, ManifestReadError>
where
    F: FnOnce(&Path) -> std::io::Result<Vec<u8>>,
{
    let bytes = match read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ManifestSnapshot::empty())
        }
        Err(error) => return Err(ManifestReadError::Unreadable(error)),
    };
    let rows: Vec<serde_json::Value> =
        serde_json::from_slice(&bytes).map_err(ManifestReadError::Damaged)?;
    let items = rows
        .iter()
        .cloned()
        .map(serde_json::from_value)
        .collect::<Result<Vec<EhSavedItem>, _>>()
        .map_err(ManifestReadError::Damaged)?;
    Ok(ManifestSnapshot { items, rows })
}

#[derive(Debug)]
enum ManifestCommitError {
    Read(ManifestReadError),
    Lock(std::io::Error),
    Conflict,
    Write(std::io::Error),
}

impl std::fmt::Display for ManifestCommitError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Read(error) => write!(formatter, "{error}"),
            Self::Lock(error) => write!(formatter, "manifest write lock failed: {error}"),
            Self::Conflict => write!(formatter, "manifest concurrent modification conflict"),
            Self::Write(error) => write!(formatter, "manifest write failed: {error}"),
        }
    }
}

#[cfg(windows)]
struct ManifestCommitLock {
    handle: *mut std::ffi::c_void,
}

#[cfg(unix)]
struct ManifestCommitLock {
    _directory: std::fs::File,
}

#[cfg(not(any(windows, unix)))]
struct ManifestCommitLock;

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateMutexW(
        attributes: *mut std::ffi::c_void,
        initial_owner: i32,
        name: *const u16,
    ) -> *mut std::ffi::c_void;
    fn WaitForSingleObject(handle: *mut std::ffi::c_void, milliseconds: u32) -> u32;
    fn ReleaseMutex(handle: *mut std::ffi::c_void) -> i32;
    fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
}

#[cfg(windows)]
impl Drop for ManifestCommitLock {
    fn drop(&mut self) {
        unsafe {
            let _ = ReleaseMutex(self.handle);
            let _ = CloseHandle(self.handle);
        }
    }
}

#[cfg(windows)]
fn acquire_manifest_commit_lock(out_dir: &Path) -> Result<ManifestCommitLock, ManifestCommitError> {
    use std::os::windows::ffi::OsStrExt as _;

    const WAIT_OBJECT_0: u32 = 0;
    const WAIT_ABANDONED: u32 = 0x80;
    const WAIT_TIMEOUT: u32 = 258;

    let canonical = out_dir.canonicalize().map_err(ManifestCommitError::Lock)?;
    let identity = canonical.to_string_lossy().to_lowercase();
    let name = format!(
        "Local\\RCH-EH-Manifest-{:x}",
        Sha256::digest(identity.as_bytes())
    );
    let wide_name: Vec<u16> = std::ffi::OsStr::new(&name)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let handle = unsafe { CreateMutexW(std::ptr::null_mut(), 0, wide_name.as_ptr()) };
    if handle.is_null() {
        return Err(ManifestCommitError::Lock(std::io::Error::last_os_error()));
    }

    match unsafe { WaitForSingleObject(handle, 2_000) } {
        WAIT_OBJECT_0 | WAIT_ABANDONED => Ok(ManifestCommitLock { handle }),
        WAIT_TIMEOUT => {
            unsafe {
                let _ = CloseHandle(handle);
            }
            Err(ManifestCommitError::Conflict)
        }
        _ => {
            let error = std::io::Error::last_os_error();
            unsafe {
                let _ = CloseHandle(handle);
            }
            Err(ManifestCommitError::Lock(error))
        }
    }
}

#[cfg(unix)]
fn acquire_manifest_commit_lock(out_dir: &Path) -> Result<ManifestCommitLock, ManifestCommitError> {
    let directory = std::fs::File::open(out_dir).map_err(ManifestCommitError::Lock)?;
    for _ in 0..200 {
        match fs2::FileExt::try_lock_exclusive(&directory) {
            Ok(()) => {
                return Ok(ManifestCommitLock {
                    _directory: directory,
                })
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(ManifestCommitError::Lock(error)),
        }
    }
    Err(ManifestCommitError::Conflict)
}

#[cfg(not(any(windows, unix)))]
fn acquire_manifest_commit_lock(
    _out_dir: &Path,
) -> Result<ManifestCommitLock, ManifestCommitError> {
    Err(ManifestCommitError::Lock(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "manifest writer locking is unsupported on this platform",
    )))
}

fn commit_manifest_additions(
    out_dir: &Path,
    additions: &[EhSavedItem],
) -> Result<usize, ManifestCommitError> {
    if additions.is_empty() {
        return Ok(0);
    }

    let _lock = acquire_manifest_commit_lock(out_dir)?;
    let mut latest = load_manifest_for_collection(out_dir).map_err(ManifestCommitError::Read)?;
    let mut known: HashSet<String> = latest
        .items
        .iter()
        .map(|item| item.infohash.to_ascii_lowercase())
        .collect();
    let mut added = 0usize;

    for item in additions {
        if !known.insert(item.infohash.to_ascii_lowercase()) {
            continue;
        }
        let row = serde_json::to_value(item).map_err(|error| {
            ManifestCommitError::Write(std::io::Error::new(std::io::ErrorKind::InvalidData, error))
        })?;
        latest.items.push(item.clone());
        latest.rows.push(row);
        added += 1;
    }
    if added == 0 {
        return Ok(0);
    }

    let path = manifest_path(&out_dir.to_string_lossy());
    atomic_write_manifest(&path, &latest.rows).map_err(ManifestCommitError::Write)?;
    let verified = load_manifest_for_collection(out_dir).map_err(ManifestCommitError::Read)?;
    if verified.rows != latest.rows || verified.items.len() != latest.items.len() {
        return Err(ManifestCommitError::Write(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "manifest read-back did not match the committed history",
        )));
    }
    Ok(added)
}

static MANIFEST_TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct ManifestTempGuard {
    path: PathBuf,
    committed: bool,
}

impl Drop for ManifestTempGuard {
    fn drop(&mut self) {
        if !self.committed {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

fn create_manifest_temp(target: &Path) -> std::io::Result<(PathBuf, std::fs::File)> {
    let parent = rules_config_parent(target);
    let base = target
        .file_name()
        .unwrap_or_else(|| std::ffi::OsStr::new("manifest.json"))
        .to_string_lossy();
    for _ in 0..128 {
        let sequence = MANIFEST_TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!(".{base}.rch-tmp-{}-{sequence}", std::process::id()));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "unable to allocate unique manifest temp file",
    ))
}

fn atomic_write_manifest(target: &Path, rows: &[serde_json::Value]) -> std::io::Result<()> {
    let bytes = serde_json::to_vec_pretty(rows)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    atomic_write_manifest_with(
        target,
        rows,
        &bytes,
        |file, bytes| {
            file.write_all(bytes)?;
            file.flush()?;
            file.sync_all()
        },
        |source, destination| std::fs::rename(source, destination),
    )
}

fn atomic_write_manifest_with<W, R>(
    target: &Path,
    expected_rows: &[serde_json::Value],
    bytes: &[u8],
    write_temp: W,
    replace: R,
) -> std::io::Result<()>
where
    W: FnOnce(&mut std::fs::File, &[u8]) -> std::io::Result<()>,
    R: FnOnce(&Path, &Path) -> std::io::Result<()>,
{
    let encoded_rows: Vec<serde_json::Value> = serde_json::from_slice(bytes)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    if encoded_rows != expected_rows {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "serialized manifest differs from intended rows",
        ));
    }

    let (temp_path, mut temp_file) = create_manifest_temp(target)?;
    let mut guard = ManifestTempGuard {
        path: temp_path.clone(),
        committed: false,
    };
    write_temp(&mut temp_file, bytes)?;
    drop(temp_file);

    let temp_bytes = std::fs::read(&temp_path)?;
    let written_rows: Vec<serde_json::Value> = serde_json::from_slice(&temp_bytes)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    if written_rows != expected_rows {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "temporary manifest validation failed",
        ));
    }

    // Both paths are in the same directory. The destination is never removed first.
    replace(&temp_path, target)?;
    guard.committed = true;

    let installed_bytes = std::fs::read(target)?;
    let installed_rows: Vec<serde_json::Value> = serde_json::from_slice(&installed_bytes)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    if installed_rows != expected_rows {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "committed manifest validation failed",
        ));
    }
    sync_manifest_parent(target)?;
    Ok(())
}

#[cfg(unix)]
fn sync_manifest_parent(target: &Path) -> std::io::Result<()> {
    let parent = target.parent().unwrap_or_else(|| Path::new("."));
    std::fs::File::open(parent)?.sync_all()
}

#[cfg(not(unix))]
fn sync_manifest_parent(_target: &Path) -> std::io::Result<()> {
    // Windows has no portable std API for opening and syncing a directory handle.
    Ok(())
}

/// Persist EH rules through the existing config path. The temporary file stays
/// beside the target, and the old config is never removed first.
pub fn save_rules_atomic(target: &Path, rules: &EhRules) -> Result<(), String> {
    let _lock = acquire_rules_config_lock(target)?;
    save_rules_atomic_unlocked(target, rules)
}

fn save_rules_atomic_unlocked(target: &Path, rules: &EhRules) -> Result<(), String> {
    rules.network.validate()?;
    let bytes =
        serde_json::to_vec_pretty(rules).map_err(|error| format!("序列化 EH 规则失败：{error}"))?;
    let (temp_path, mut temp_file) =
        create_manifest_temp(target).map_err(|error| format!("创建规则临时文件失败：{error}"))?;
    let mut guard = ManifestTempGuard {
        path: temp_path.clone(),
        committed: false,
    };

    let write_result = temp_file
        .write_all(&bytes)
        .and_then(|()| temp_file.flush())
        .and_then(|()| temp_file.sync_all());
    drop(temp_file);
    write_result.map_err(|error| format!("写入 EH 规则失败：{error}"))?;

    let validate = |path: &Path| -> Result<(), String> {
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("读取待提交 EH 规则失败：{error}"))?;
        let saved = EhRules::from_json(&text)?;
        if saved != *rules {
            return Err("EH 规则读回校验不一致".into());
        }
        Ok(())
    };
    validate(&temp_path)?;

    // Windows replacement behavior is covered by the R0 Windows regression;
    // on Unix, same-directory rename atomically replaces the prior file.
    std::fs::rename(&temp_path, target).map_err(|error| format!("替换 EH 规则失败：{error}"))?;
    guard.committed = true;
    validate(target)?;
    sync_manifest_parent(target).map_err(|error| format!("同步 EH 规则目录失败：{error}"))?;
    Ok(())
}

#[cfg(windows)]
struct RulesConfigLock {
    handle: *mut std::ffi::c_void,
}

#[cfg(unix)]
struct RulesConfigLock {
    _directory: std::fs::File,
}

#[cfg(not(any(windows, unix)))]
struct RulesConfigLock;

#[cfg(windows)]
impl Drop for RulesConfigLock {
    fn drop(&mut self) {
        unsafe {
            let _ = ReleaseMutex(self.handle);
            let _ = CloseHandle(self.handle);
        }
    }
}

#[cfg(windows)]
fn rules_mutex_name(target: &Path, purpose: &str) -> Result<Vec<u16>, String> {
    use std::os::windows::ffi::OsStrExt as _;

    let parent = target.parent().unwrap_or_else(|| Path::new("."));
    let canonical_parent = parent
        .canonicalize()
        .map_err(|error| format!("解析 EH 配置目录失败：{error}"))?;
    let file_name = target
        .file_name()
        .ok_or_else(|| "EH 配置路径缺少文件名".to_string())?;
    let identity = canonical_parent
        .join(file_name)
        .to_string_lossy()
        .replace('/', "\\")
        .to_lowercase();
    let name = format!(
        "Local\\RCH-EH-{purpose}-{:x}",
        Sha256::digest(identity.as_bytes())
    );
    Ok(std::ffi::OsStr::new(&name)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect())
}

#[cfg(windows)]
fn acquire_named_rules_mutex(
    target: &Path,
    purpose: &str,
    timeout_ms: u32,
) -> Result<Option<*mut std::ffi::c_void>, String> {
    const WAIT_OBJECT_0: u32 = 0;
    const WAIT_ABANDONED: u32 = 0x80;
    const WAIT_TIMEOUT: u32 = 258;

    let name = rules_mutex_name(target, purpose)?;
    let handle = unsafe { CreateMutexW(std::ptr::null_mut(), 0, name.as_ptr()) };
    if handle.is_null() {
        return Err(format!(
            "创建 EH {purpose} 互斥锁失败：{}",
            std::io::Error::last_os_error()
        ));
    }
    match unsafe { WaitForSingleObject(handle, timeout_ms) } {
        WAIT_OBJECT_0 | WAIT_ABANDONED => Ok(Some(handle)),
        WAIT_TIMEOUT => {
            unsafe {
                let _ = CloseHandle(handle);
            }
            Ok(None)
        }
        _ => {
            let error = std::io::Error::last_os_error();
            unsafe {
                let _ = CloseHandle(handle);
            }
            Err(format!("等待 EH {purpose} 互斥锁失败：{error}"))
        }
    }
}

fn acquire_rules_config_lock(target: &Path) -> Result<RulesConfigLock, String> {
    // The first rules read also needs this lock, but the rules loader creates
    // the file's parent only after acquiring it. Create the app-owned config
    // directory first so a clean Android install can acquire the lock.
    let parent = rules_config_parent(target);
    std::fs::create_dir_all(parent).map_err(|error| format!("创建 EH 配置目录失败：{error}"))?;

    #[cfg(windows)]
    {
        let handle = acquire_named_rules_mutex(target, "Rules", 5_000)?
            .ok_or_else(|| "EH 规则仍被另一个 RCH 进程修改，已安全取消本次写入".to_string())?;
        return Ok(RulesConfigLock { handle });
    }

    #[cfg(unix)]
    {
        use fs2::FileExt as _;
        let directory = std::fs::File::open(parent)
            .map_err(|error| format!("打开 EH 配置目录互斥锁失败：{error}"))?;
        for _ in 0..500 {
            match directory.try_lock_exclusive() {
                Ok(()) => {
                    return Ok(RulesConfigLock {
                        _directory: directory,
                    })
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => return Err(format!("获取 EH 规则互斥锁失败：{error}")),
            }
        }
        return Err("EH 规则仍被另一个进程修改，已安全取消本次写入".into());
    }

    #[cfg(not(any(windows, unix)))]
    {
        let _ = target;
        Err("当前平台不支持 EH 规则跨进程互斥".into())
    }
}

fn rules_config_parent(target: &Path) -> &Path {
    target
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

pub fn with_rules_config_lock<T>(
    target: &Path,
    operation: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let _lock = acquire_rules_config_lock(target)?;
    operation()
}

#[cfg(windows)]
pub struct EhCollectionLock {
    handle: *mut std::ffi::c_void,
}

#[cfg(windows)]
impl Drop for EhCollectionLock {
    fn drop(&mut self) {
        unsafe {
            let _ = ReleaseMutex(self.handle);
            let _ = CloseHandle(self.handle);
        }
    }
}

#[cfg(not(windows))]
pub struct EhCollectionLock;

/// Try to hold a configuration-scoped Windows lease for the duration of one
/// EH collection. Settings and reading do not acquire this lease.
pub fn try_acquire_collection_lock(target: &Path) -> Result<Option<EhCollectionLock>, String> {
    #[cfg(windows)]
    {
        return Ok(acquire_named_rules_mutex(target, "Collection", 0)?
            .map(|handle| EhCollectionLock { handle }));
    }

    #[cfg(not(windows))]
    {
        let _ = target;
        Ok(Some(EhCollectionLock))
    }
}

/// Returns whether another process currently owns this config's collection
/// lease. An abandoned mutex means its former owner exited and is not active.
pub fn collection_is_active(target: &Path) -> Result<bool, String> {
    Ok(try_acquire_collection_lock(target)?.is_none())
}

fn read_rules_for_update(target: &Path) -> Result<EhRules, String> {
    let text = std::fs::read_to_string(target)
        .map_err(|error| format!("读取 EH 规则失败，已停止状态更新：{error}"))?;
    EhRules::from_json(&text)
}

/// Save changed user rule fields against the latest disk version. Daily state
/// is owned by the dedicated operations below and can never be overwritten by
/// a stale whole-file snapshot from another process.
pub fn save_rules_merged(
    target: &Path,
    base_json: &str,
    desired_json: &str,
) -> Result<String, String> {
    let _lock = acquire_rules_config_lock(target)?;
    let base_value: serde_json::Value = serde_json::from_str(base_json)
        .map_err(|error| format!("EH 规则基线 JSON 无效：{error}"))?;
    let desired_value: serde_json::Value = serde_json::from_str(desired_json)
        .map_err(|error| format!("EH 规则 JSON 无效：{error}"))?;
    EhRules::from_json(base_json)?;
    EhRules::from_json(desired_json)?;

    let latest = read_rules_for_update(target)?;
    let mut merged =
        serde_json::to_value(latest).map_err(|error| format!("序列化现有 EH 规则失败：{error}"))?;
    let desired = desired_value
        .as_object()
        .ok_or_else(|| "EH 规则必须是 JSON 对象".to_string())?;
    let base = base_value
        .as_object()
        .ok_or_else(|| "EH 规则基线必须是 JSON 对象".to_string())?;
    let merged_object = merged
        .as_object_mut()
        .ok_or_else(|| "现有 EH 规则必须是 JSON 对象".to_string())?;

    for key in base.keys().chain(desired.keys()) {
        if key == "automation" || base.get(key) == desired.get(key) {
            continue;
        }
        match desired.get(key) {
            Some(value) => {
                merged_object.insert(key.clone(), value.clone());
            }
            None => {
                merged_object.remove(key);
            }
        }
    }

    let rules = EhRules::from_json(&merged.to_string())?;
    save_rules_atomic_unlocked(target, &rules)?;
    Ok(rules.to_json())
}

pub fn set_daily_enabled(target: &Path, enabled: bool) -> Result<String, String> {
    let _lock = acquire_rules_config_lock(target)?;
    let mut rules = read_rules_for_update(target)?;
    rules.automation.daily_enabled = enabled;
    save_rules_atomic_unlocked(target, &rules)?;
    Ok(rules.to_json())
}

fn is_valid_local_date_key(value: &str) -> bool {
    let parts: Vec<_> = value.split('-').collect();
    if parts.len() != 3 || parts[0].len() != 4 || parts[1].len() != 2 || parts[2].len() != 2 {
        return false;
    }
    let (Ok(year), Ok(month), Ok(day)) = (
        parts[0].parse::<u32>(),
        parts[1].parse::<u32>(),
        parts[2].parse::<u32>(),
    ) else {
        return false;
    };
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days_in_month = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    day >= 1 && day <= days_in_month
}

pub fn claim_daily(target: &Path, date: &str) -> Result<(bool, String), String> {
    if !is_valid_local_date_key(date) {
        return Err("EH Daily 日期格式无效，已停止领取".into());
    }
    let _lock = acquire_rules_config_lock(target)?;
    let mut rules = read_rules_for_update(target)?;
    let active_run = rules
        .automation
        .last_run
        .as_ref()
        .is_some_and(|run| matches!(run.status.as_str(), "claimed" | "running"));
    if active_run || collection_is_active(target)? {
        return Ok((false, rules.to_json()));
    }
    if !rules.automation.daily_enabled {
        return Ok((false, rules.to_json()));
    }
    if let Some(last_attempt) = rules.automation.last_auto_attempt_date.as_deref() {
        if !is_valid_local_date_key(last_attempt) {
            return Err("EH Daily 已保存日期损坏，已停止领取".into());
        }
        if date <= last_attempt {
            return Ok((false, rules.to_json()));
        }
    }

    rules.automation.last_auto_attempt_date = Some(date.to_string());
    rules.automation.last_run = Some(EhAutomationRun {
        date: date.to_string(),
        source: "daily_auto".into(),
        status: "claimed".into(),
        saved: 0,
        error: None,
    });
    save_rules_atomic_unlocked(target, &rules)?;
    Ok((true, rules.to_json()))
}

pub fn update_daily_run(target: &Path, requested: EhAutomationRun) -> Result<String, String> {
    if !is_valid_local_date_key(&requested.date)
        || !matches!(requested.source.as_str(), "manual" | "daily_auto")
        || !matches!(
            requested.status.as_str(),
            "running" | "completed" | "failed" | "cancelled" | "interrupted_unknown"
        )
    {
        return Err("EH Daily 运行状态无效，已停止写入".into());
    }

    let _lock = acquire_rules_config_lock(target)?;
    let mut rules = read_rules_for_update(target)?;
    let current = rules.automation.last_run.as_ref();
    let collection_active = collection_is_active(target)?;
    let allowed = match requested.status.as_str() {
        "running" if requested.source == "daily_auto" => current.is_some_and(|run| {
            rules.automation.daily_enabled
                && !collection_active
                && run.date == requested.date
                && run.source == requested.source
                && run.status == "claimed"
        }),
        "running" => {
            requested.source == "manual"
                && !collection_active
                && current.is_none_or(|run| !matches!(run.status.as_str(), "claimed" | "running"))
        }
        "completed" | "failed" | "cancelled" | "interrupted_unknown" => {
            current.is_some_and(|run| {
                run.date == requested.date
                    && run.source == requested.source
                    && matches!(run.status.as_str(), "claimed" | "running")
            })
        }
        _ => false,
    };
    if !allowed {
        return Err("EH Daily 状态迁移不适用、已被更新或重复，已停止覆盖".into());
    }
    rules.automation.last_run = Some(requested);
    save_rules_atomic_unlocked(target, &rules)?;
    Ok(rules.to_json())
}

pub fn validate_daily_collection_start(
    target: &Path,
    date: &str,
    source: &str,
) -> Result<(), String> {
    if source == "manual" {
        return Ok(());
    }
    if source != "daily_auto" || !is_valid_local_date_key(date) {
        return Err("EH 自动收集启动参数无效，已停止请求".into());
    }

    let _lock = acquire_rules_config_lock(target)?;
    let rules = read_rules_for_update(target)?;
    let matches_running_claim = rules.automation.last_run.as_ref().is_some_and(|run| {
        run.date == date && run.source == "daily_auto" && run.status == "running"
    });
    if !rules.automation.daily_enabled
        || rules.automation.last_auto_attempt_date.as_deref() != Some(date)
        || !matches_running_claim
    {
        return Err("EH Daily 已关闭或领取状态已变化，未发起扫描。".into());
    }
    Ok(())
}

/// Recover stale claimed/running state only when no process still owns the
/// configuration-scoped collection lease.
pub fn recover_daily_run(target: &Path) -> Result<String, String> {
    let _lock = acquire_rules_config_lock(target)?;
    let mut rules = read_rules_for_update(target)?;
    let Some(current) = rules.automation.last_run.as_ref() else {
        return Ok(rules.to_json());
    };
    if !matches!(current.status.as_str(), "claimed" | "running") || collection_is_active(target)? {
        return Ok(rules.to_json());
    }

    let mut recovered = current.clone();
    recovered.status = "interrupted_unknown".into();
    recovered.error = Some("上次进程退出时任务尚未结束，远端结果未知；本轮不会自动重试。".into());
    rules.automation.last_run = Some(recovered);
    save_rules_atomic_unlocked(target, &rules)?;
    Ok(rules.to_json())
}

const MAX_TORRENT_METADATA_BYTES: u64 = 16 * 1024 * 1024;
const MAX_BENCODE_DEPTH: usize = 128;
const MAX_BENCODE_VALUES: usize = 1_000_000;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EhManifestReadStatus {
    Unconfigured,
    Missing,
    Empty,
    Ready,
    Damaged,
    Unreadable,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EhSavedFileStatus {
    Present,
    Missing,
    UnsafePath,
    NotRegularFile,
    Unreadable,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EhTorrentValidationStatus {
    Valid,
    SizeMismatch,
    InvalidTorrent,
    InfohashMismatch,
    TooLarge,
    Unreadable,
    NotChecked,
}

#[derive(Debug, Clone, Serialize)]
pub struct EhCheckedManifestEntry {
    pub item: EhSavedItem,
    pub file_status: EhSavedFileStatus,
    pub validation_status: EhTorrentValidationStatus,
    /// Available only for a relative path confirmed to remain inside `out_dir`.
    pub full_path: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EhManifestCheck {
    pub status: EhManifestReadStatus,
    pub entries: Vec<EhCheckedManifestEntry>,
    pub invalid_entries: usize,
    /// Fixed internal category only; never includes paths or filesystem error text.
    pub error_code: Option<String>,
}

impl EhManifestCheck {
    fn new(status: EhManifestReadStatus) -> Self {
        Self {
            status,
            entries: Vec::new(),
            invalid_entries: 0,
            error_code: None,
        }
    }
}

/// Read the local manifest without changing the legacy `read_manifest` contract.
/// Only the configured directory, its `manifest.json`, and files referenced by
/// that manifest are touched; the directory is never enumerated.
pub fn read_manifest_checked(out_dir: &Path) -> EhManifestCheck {
    if out_dir.as_os_str().is_empty() {
        return EhManifestCheck::new(EhManifestReadStatus::Unconfigured);
    }

    let root = match out_dir.canonicalize() {
        Ok(root) if root.is_dir() => root,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return EhManifestCheck::new(EhManifestReadStatus::Missing);
        }
        _ => {
            let mut report = EhManifestCheck::new(EhManifestReadStatus::Unreadable);
            report.error_code = Some("output_directory_unreadable".into());
            return report;
        }
    };

    let manifest = match out_dir.join("manifest.json").canonicalize() {
        Ok(manifest) if manifest.starts_with(&root) => manifest,
        Ok(_) => {
            let mut report = EhManifestCheck::new(EhManifestReadStatus::Unreadable);
            report.error_code = Some("manifest_path_escapes_output_directory".into());
            return report;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return EhManifestCheck::new(EhManifestReadStatus::Missing);
        }
        Err(_) => {
            let mut report = EhManifestCheck::new(EhManifestReadStatus::Unreadable);
            report.error_code = Some("manifest_unreadable".into());
            return report;
        }
    };

    match std::fs::metadata(&manifest) {
        Ok(metadata) if metadata.is_file() => {}
        Ok(_) => {
            let mut report = EhManifestCheck::new(EhManifestReadStatus::Unreadable);
            report.error_code = Some("manifest_not_regular_file".into());
            return report;
        }
        Err(_) => {
            let mut report = EhManifestCheck::new(EhManifestReadStatus::Unreadable);
            report.error_code = Some("manifest_unreadable".into());
            return report;
        }
    }

    let bytes = match std::fs::read(&manifest) {
        Ok(bytes) => bytes,
        Err(_) => {
            let mut report = EhManifestCheck::new(EhManifestReadStatus::Unreadable);
            report.error_code = Some("manifest_unreadable".into());
            return report;
        }
    };
    let value: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(_) => {
            let mut report = EhManifestCheck::new(EhManifestReadStatus::Damaged);
            report.error_code = Some("manifest_json_invalid".into());
            return report;
        }
    };
    let Some(rows) = value.as_array() else {
        let mut report = EhManifestCheck::new(EhManifestReadStatus::Damaged);
        report.error_code = Some("manifest_root_not_array".into());
        return report;
    };

    let mut report = EhManifestCheck::new(EhManifestReadStatus::Ready);
    for row in rows {
        match serde_json::from_value::<EhSavedItem>(row.clone()) {
            Ok(item) => report.entries.push(check_saved_item(&root, item)),
            Err(_) => report.invalid_entries += 1,
        }
    }

    if report.invalid_entries > 0 {
        report.status = EhManifestReadStatus::Damaged;
        report.error_code = Some("manifest_entries_invalid".into());
    } else if rows.is_empty() {
        report.status = EhManifestReadStatus::Empty;
    }
    report
}

/// Enumerate and validate only immediate `.torrent` files in the configured
/// output directory. This is intentionally independent of `manifest.json` so
/// files saved before an interrupted manifest commit remain discoverable.
/// Subdirectories are never traversed and symlinks are never followed.
pub fn read_torrent_directory_checked(out_dir: &Path) -> EhManifestCheck {
    if out_dir.as_os_str().is_empty() {
        return EhManifestCheck::new(EhManifestReadStatus::Unconfigured);
    }

    let root = match out_dir.canonicalize() {
        Ok(root) if root.is_dir() => root,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return EhManifestCheck::new(EhManifestReadStatus::Missing);
        }
        _ => {
            let mut report = EhManifestCheck::new(EhManifestReadStatus::Unreadable);
            report.error_code = Some("output_directory_unreadable".into());
            return report;
        }
    };

    let directory = match std::fs::read_dir(&root) {
        Ok(directory) => directory,
        Err(_) => {
            let mut report = EhManifestCheck::new(EhManifestReadStatus::Unreadable);
            report.error_code = Some("output_directory_unreadable".into());
            return report;
        }
    };

    let mut report = EhManifestCheck::new(EhManifestReadStatus::Empty);
    for item in directory {
        let item = match item {
            Ok(item) => item,
            Err(_) => {
                report.status = EhManifestReadStatus::Unreadable;
                report.error_code = Some("output_directory_entry_unreadable".into());
                report.entries.clear();
                return report;
            }
        };
        let path = item.path();
        if !path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("torrent"))
        {
            continue;
        }

        let file_name = match item.file_name().into_string() {
            Ok(file_name) => file_name,
            Err(_) => {
                report.status = EhManifestReadStatus::Unreadable;
                report.error_code = Some("torrent_filename_not_unicode".into());
                report.entries.clear();
                return report;
            }
        };
        let file_type = match item.file_type() {
            Ok(file_type) => file_type,
            Err(_) => {
                report.status = EhManifestReadStatus::Unreadable;
                report.error_code = Some("torrent_file_metadata_unreadable".into());
                report.entries.clear();
                return report;
            }
        };

        let title = Path::new(&file_name)
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or(&file_name)
            .to_string();

        if file_type.is_symlink() {
            report.entries.push(EhCheckedManifestEntry {
                item: directory_torrent_item(&file_name, 0, "", &title, "未知".into()),
                file_status: EhSavedFileStatus::UnsafePath,
                validation_status: EhTorrentValidationStatus::NotChecked,
                full_path: None,
            });
            continue;
        }
        if !file_type.is_file() {
            report.entries.push(EhCheckedManifestEntry {
                item: directory_torrent_item(&file_name, 0, "", &title, "未知".into()),
                file_status: EhSavedFileStatus::NotRegularFile,
                validation_status: EhTorrentValidationStatus::NotChecked,
                full_path: None,
            });
            continue;
        }

        let canonical = match path.canonicalize() {
            Ok(canonical) if canonical.starts_with(&root) && canonical.parent() == Some(&root) => {
                canonical
            }
            _ => {
                report.entries.push(EhCheckedManifestEntry {
                    item: directory_torrent_item(&file_name, 0, "", &title, "未知".into()),
                    file_status: EhSavedFileStatus::UnsafePath,
                    validation_status: EhTorrentValidationStatus::NotChecked,
                    full_path: None,
                });
                continue;
            }
        };
        let metadata = match std::fs::metadata(&canonical) {
            Ok(metadata) if metadata.is_file() => metadata,
            _ => {
                report.entries.push(EhCheckedManifestEntry {
                    item: directory_torrent_item(&file_name, 0, "", &title, "未知".into()),
                    file_status: EhSavedFileStatus::Unreadable,
                    validation_status: EhTorrentValidationStatus::Unreadable,
                    full_path: None,
                });
                continue;
            }
        };
        let modified_at = metadata
            .modified()
            .ok()
            .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
            .and_then(|duration| i64::try_from(duration.as_secs()).ok())
            .map(fmt_utc)
            .unwrap_or_else(|| "未知".into());
        let bytes = if metadata.len() > MAX_TORRENT_METADATA_BYTES {
            None
        } else {
            std::fs::read(&canonical).ok()
        };
        let hash = bytes.as_deref().and_then(torrent_v1_infohash);
        let validation_status = match (&bytes, &hash) {
            (None, _) => {
                if metadata.len() > MAX_TORRENT_METADATA_BYTES {
                    EhTorrentValidationStatus::TooLarge
                } else {
                    EhTorrentValidationStatus::Unreadable
                }
            }
            (Some(_), None) => EhTorrentValidationStatus::InvalidTorrent,
            (Some(_), Some(_)) => EhTorrentValidationStatus::Valid,
        };
        report.entries.push(EhCheckedManifestEntry {
            item: directory_torrent_item(
                &file_name,
                i64::try_from(metadata.len()).unwrap_or(i64::MAX),
                hash.as_deref().unwrap_or_default(),
                &title,
                modified_at,
            ),
            file_status: EhSavedFileStatus::Present,
            validation_status,
            full_path: Some(canonical.to_string_lossy().into_owned()),
        });
    }

    report
        .entries
        .sort_by(|left, right| left.item.file.cmp(&right.item.file));
    if !report.entries.is_empty() {
        report.status = EhManifestReadStatus::Ready;
    }
    report
}

fn directory_torrent_item(
    file: &str,
    bytes: i64,
    infohash: &str,
    title: &str,
    saved_at_utc: String,
) -> EhSavedItem {
    EhSavedItem {
        infohash: infohash.into(),
        gid: String::new(),
        title: title.into(),
        title_jpn: title.into(),
        rating: 0.0,
        downloads: 0,
        required_dl: 0,
        posted: None,
        posted_utc: String::new(),
        age_years: None,
        tags: Vec::new(),
        category: String::new(),
        filecount: None,
        filesize: None,
        uploader: String::new(),
        torrent_name: file.into(),
        file: file.into(),
        bytes,
        source_url: String::new(),
        saved_at_utc,
        semantic: EhSemantic::default(),
    }
}

fn check_saved_item(root: &Path, item: EhSavedItem) -> EhCheckedManifestEntry {
    let (file_status, path) = resolve_manifest_file(root, &item.file);
    let validation_status = if file_status == EhSavedFileStatus::Present {
        match path.as_deref() {
            Some(path) => validate_torrent_file(path, &item),
            None => EhTorrentValidationStatus::NotChecked,
        }
    } else {
        EhTorrentValidationStatus::NotChecked
    };

    EhCheckedManifestEntry {
        item,
        file_status,
        validation_status,
        full_path: path.map(|path| path.to_string_lossy().into_owned()),
    }
}

fn resolve_manifest_file(root: &Path, relative: &str) -> (EhSavedFileStatus, Option<PathBuf>) {
    let path = Path::new(relative);
    let components: Vec<&str> = relative
        .split(|character| character == '/' || character == '\\')
        .collect();
    let has_drive_prefix = components.first().is_some_and(|component| {
        let bytes = component.as_bytes();
        bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
    });

    if relative.is_empty()
        || relative.contains('\0')
        || path.is_absolute()
        || relative.starts_with('/')
        || relative.starts_with('\\')
        || has_drive_prefix
        || components
            .iter()
            .any(|component| component.is_empty() || *component == "." || *component == "..")
    {
        return (EhSavedFileStatus::UnsafePath, None);
    }

    let mut resolved = root.to_path_buf();
    for (index, component) in components.iter().enumerate() {
        resolved.push(component);
        match std::fs::symlink_metadata(&resolved) {
            Ok(_) => {
                let canonical = match resolved.canonicalize() {
                    Ok(canonical) if canonical.starts_with(root) => canonical,
                    Ok(_) | Err(_) => return (EhSavedFileStatus::UnsafePath, None),
                };
                if index + 1 < components.len() {
                    if !canonical.is_dir() {
                        return (EhSavedFileStatus::NotRegularFile, Some(canonical));
                    }
                } else if !canonical.is_file() {
                    return (EhSavedFileStatus::NotRegularFile, Some(canonical));
                }
                resolved = canonical;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                for remaining in components.iter().skip(index + 1) {
                    resolved.push(remaining);
                }
                return (EhSavedFileStatus::Missing, Some(resolved));
            }
            Err(_) => return (EhSavedFileStatus::Unreadable, None),
        }
    }

    (EhSavedFileStatus::Present, Some(resolved))
}

fn validate_torrent_file(path: &Path, item: &EhSavedItem) -> EhTorrentValidationStatus {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(_) => return EhTorrentValidationStatus::Unreadable,
    };
    let metadata = match file.metadata() {
        Ok(metadata) if metadata.is_file() => metadata,
        _ => return EhTorrentValidationStatus::Unreadable,
    };
    if item.bytes < 0 || metadata.len() != item.bytes as u64 {
        return EhTorrentValidationStatus::SizeMismatch;
    }
    if metadata.len() > MAX_TORRENT_METADATA_BYTES {
        return EhTorrentValidationStatus::TooLarge;
    }

    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    let mut limited = file.take(MAX_TORRENT_METADATA_BYTES + 1);
    if limited.read_to_end(&mut bytes).is_err() {
        return EhTorrentValidationStatus::Unreadable;
    }
    if bytes.len() as u64 > MAX_TORRENT_METADATA_BYTES {
        return EhTorrentValidationStatus::TooLarge;
    }

    match torrent_v1_infohash(&bytes) {
        Some(infohash) if infohash.eq_ignore_ascii_case(&item.infohash) => {
            EhTorrentValidationStatus::Valid
        }
        Some(_) => EhTorrentValidationStatus::InfohashMismatch,
        None => EhTorrentValidationStatus::InvalidTorrent,
    }
}

fn torrent_v1_infohash(bytes: &[u8]) -> Option<String> {
    let mut parser = BencodeParser::new(bytes);
    if parser.bytes.first().copied()? != b'd' {
        return None;
    }
    parser.position = 1;
    let mut previous_key: Option<&[u8]> = None;
    let mut info_range = None;

    loop {
        if parser.bytes.get(parser.position).copied()? == b'e' {
            parser.position += 1;
            break;
        }
        let key = parser.parse_bytes()?;
        if previous_key.is_some_and(|previous| previous >= key) {
            return None;
        }
        previous_key = Some(key);
        let start = parser.position;
        let kind = parser.parse_value(1)?;
        if key == b"info" {
            if kind != BencodeKind::Dictionary || info_range.is_some() {
                return None;
            }
            info_range = Some((start, parser.position));
        }
    }

    if parser.position != bytes.len() {
        return None;
    }
    let (start, end) = info_range?;
    let digest = ring::digest::digest(&ring::digest::SHA1_FOR_LEGACY_USE_ONLY, &bytes[start..end]);
    let hex = b"0123456789abcdef";
    let mut infohash = String::with_capacity(digest.as_ref().len() * 2);
    for byte in digest.as_ref() {
        infohash.push(hex[(byte >> 4) as usize] as char);
        infohash.push(hex[(byte & 0x0f) as usize] as char);
    }
    Some(infohash)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BencodeKind {
    Dictionary,
    List,
    Integer,
    Bytes,
}

struct BencodeParser<'a> {
    bytes: &'a [u8],
    position: usize,
    values: usize,
}

impl<'a> BencodeParser<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            position: 0,
            values: 0,
        }
    }

    fn parse_value(&mut self, depth: usize) -> Option<BencodeKind> {
        if depth > MAX_BENCODE_DEPTH || self.values >= MAX_BENCODE_VALUES {
            return None;
        }
        self.values += 1;
        match self.bytes.get(self.position).copied()? {
            b'i' => {
                self.parse_integer()?;
                Some(BencodeKind::Integer)
            }
            b'l' => {
                self.position += 1;
                while self.bytes.get(self.position).copied()? != b'e' {
                    self.parse_value(depth + 1)?;
                }
                self.position += 1;
                Some(BencodeKind::List)
            }
            b'd' => {
                self.position += 1;
                let mut previous_key: Option<&[u8]> = None;
                while self.bytes.get(self.position).copied()? != b'e' {
                    let key = self.parse_bytes()?;
                    if previous_key.is_some_and(|previous| previous >= key) {
                        return None;
                    }
                    previous_key = Some(key);
                    self.parse_value(depth + 1)?;
                }
                self.position += 1;
                Some(BencodeKind::Dictionary)
            }
            b'0'..=b'9' => {
                self.parse_bytes()?;
                Some(BencodeKind::Bytes)
            }
            _ => None,
        }
    }

    fn parse_bytes(&mut self) -> Option<&'a [u8]> {
        let start = self.position;
        while self
            .bytes
            .get(self.position)
            .is_some_and(u8::is_ascii_digit)
        {
            self.position += 1;
        }
        if self.position == start || self.bytes.get(self.position).copied()? != b':' {
            return None;
        }
        let digits = &self.bytes[start..self.position];
        if digits.len() > 1 && digits[0] == b'0' {
            return None;
        }
        let length = std::str::from_utf8(digits).ok()?.parse::<usize>().ok()?;
        self.position += 1;
        let end = self.position.checked_add(length)?;
        let value = self.bytes.get(self.position..end)?;
        self.position = end;
        Some(value)
    }

    fn parse_integer(&mut self) -> Option<()> {
        self.position += 1;
        let start = self.position;
        while self.bytes.get(self.position).copied()? != b'e' {
            self.position += 1;
        }
        let digits = self.bytes.get(start..self.position)?;
        let number = std::str::from_utf8(digits).ok()?;
        let unsigned = number.strip_prefix('-').unwrap_or(number);
        if unsigned.is_empty()
            || !unsigned.bytes().all(|byte| byte.is_ascii_digit())
            || (unsigned.len() > 1 && unsigned.starts_with('0'))
            || number == "-0"
        {
            return None;
        }
        self.position += 1;
        Some(())
    }
}

// ---------------------------------------------------------------------------
// 主流程
// ---------------------------------------------------------------------------

/// 按锚点顺序检索并判定：作品名优先，其次创作者名（见 `eh_match::search_anchors`）。
///
/// 只要某轮锚点得到 `Matched` / `Editions` 就返回；都没命中但有 `Ambiguous` 则返回
/// 需人工确认的结果；全无 → `Unmatched`（**不猜**）。
pub fn match_gallery(
    rules: &EhRules,
    work_title: &str,
    creators: &[String],
) -> Result<crate::eh_match::MatchDecision, String> {
    match_gallery_full(rules, work_title, creators, "").map(|(d, _, _)| d)
}

/// 与 [`match_gallery`] 相同，但**同时返回命中画廊的语义层**（标签/作者/系列/语言…）。
///
/// 导入流程需要标签，而 `MatchHit` 只带标题与分数，所以这里把 gdata 原始条目一并带出，
/// 用 [`derive_semantic`] 现场推导（含中文译名，缺失时按需联网更新）。
pub fn match_gallery_full(
    rules: &EhRules,
    work_title: &str,
    creators: &[String],
    number: &str,
) -> Result<(crate::eh_match::MatchDecision, EhSemantic, String), String> {
    use crate::eh_match::{self, MatchDecision};

    let host = if rules.host.trim().is_empty() {
        DEFAULT_HOST
    } else {
        rules.host.trim()
    };
    let client = Client::new(host, rules.interval(), &rules.network)?;
    // 放宽策略：先把"作品名 + 卷/话号"作为首个锚点（便于命中对应卷/话），
    // 再退回纯作品名与创作者锚点。
    let anchors = crate::eh_match::search_anchors_with_number(work_title, creators, number);
    if anchors.is_empty() {
        return Ok((
            MatchDecision::Unmatched,
            EhSemantic::default(),
            String::new(),
        ));
    }

    let mut pending_ambiguous: Option<MatchDecision> = None;
    // 第一个锚点是作品名，其余是创作者兜底——命中依据必须如实带出，
    // 否则"创作者兜底"会被上层当成"作品名命中"而误自动写库。
    for (idx, anchor) in anchors.into_iter().enumerate() {
        let anchor_kind = if idx == 0 { "title" } else { "creator" };
        // 实测：命名空间过滤不可用，必须用裸词
        let url = format!("https://{}/?f_search={}", client.host, urlencode(&anchor));
        let html = match client.get(&url) {
            Ok(h) => h,
            Err(_) => continue, // 单个锚点失败不致命，继续下一个
        };
        let pairs = parse_gallery_pairs(&html, &client.host);
        if pairs.is_empty() {
            continue;
        }
        let reqs: Vec<(String, String)> = pairs
            .iter()
            .filter_map(|(u, _)| parse_gid_token(u))
            .take(25)
            .collect();
        if reqs.is_empty() {
            continue;
        }
        std::thread::sleep(client.interval);
        let items = match client.gdata(&reqs) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let candidates: Vec<(String, String, String)> = items
            .iter()
            .map(|it| {
                let gid = it.get("gid").map(|g| g.to_string()).unwrap_or_default();
                (
                    gid,
                    text_field(it, "title").to_string(),
                    text_field(it, "title_jpn").to_string(),
                )
            })
            .collect();
        let semantic_of = |gid: &str| -> EhSemantic {
            items
                .iter()
                .find(|it| it.get("gid").map(|g| g.to_string()).unwrap_or_default() == gid)
                .map(|it| derive_semantic(it, true, &rules.out_dir))
                .unwrap_or_default()
        };
        match eh_match::decide_with_number(work_title, number, &candidates) {
            MatchDecision::Matched(h) => {
                let sem = semantic_of(&h.gid);
                return Ok((MatchDecision::Matched(h), sem, anchor_kind.to_string()));
            }
            MatchDecision::Editions(v) => {
                let sem = v.first().map(|h| semantic_of(&h.gid)).unwrap_or_default();
                return Ok((MatchDecision::Editions(v), sem, anchor_kind.to_string()));
            }
            MatchDecision::Ambiguous(v) => {
                if pending_ambiguous.is_none() {
                    pending_ambiguous = Some(MatchDecision::Ambiguous(v));
                }
            }
            MatchDecision::Unmatched => {}
        }
    }
    Ok((
        pending_ambiguous.unwrap_or(MatchDecision::Unmatched),
        EhSemantic::default(),
        String::new(),
    ))
}

/// 纯逻辑抽出，便于离线测试锚点顺序与去重。
pub fn match_gallery_anchors(work_title: &str, creators: &[String]) -> Vec<String> {
    crate::eh_match::search_anchors(work_title, creators)
}

/// 连通性预检结果（供 UI 显示；不抛错，让调用方决定是否继续）。
#[derive(Debug, Clone, Serialize)]
pub struct EhProbe {
    pub mode: String,
    pub checked_at_unix_ms: u128,
    pub host: String,
    pub host_ok: bool,
    pub host_detail: String,
    pub host_elapsed_ms: u128,
    pub host_challenge_suspected: bool,
    /// 种子直链所在的 tracker 域名（固定 ehtracker.org，与主站分开）
    pub tracker: String,
    /// Tracker 根路径的 HTTP 可达性；不代表已验证 Torrent 数据。
    pub tracker_ok: bool,
    pub tracker_detail: String,
    pub tracker_elapsed_ms: u128,
    pub tracker_challenge_suspected: bool,
    pub tracker_torrent_validated: bool,
}

#[derive(Debug)]
struct ProbeResponse {
    status: u16,
    content_type: String,
    body: String,
}

fn unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis())
        .unwrap_or_default()
}

fn challenge_suspected(status: u16, body: &str) -> bool {
    status == 403 || is_challenge(body)
}

fn effective_eh_page(response: &ProbeResponse) -> bool {
    let body = response.body.to_lowercase();
    response.status == 200
        && response
            .content_type
            .to_ascii_lowercase()
            .contains("text/html")
        && body.contains("<html")
        && body.len() > 500
        && !is_challenge(&body)
}

/// 探测主站与 tracker 是否可达。
///
/// 存在的意义：部分网络环境下主站需要代理才能访问（DNS 污染 / SNI 阻断），
/// 用户看到的现象只会是"扫描卡住然后失败"。这里把**到底是哪一段不通**讲清楚，
/// 并提示可以改主站域名或走代理。
pub fn probe_connectivity(rules: &EhRules) -> EhProbe {
    let host = if rules.host.trim().is_empty() {
        DEFAULT_HOST.to_string()
    } else {
        rules.host.trim().to_string()
    };
    let mode = rules.network.mode.as_str().to_string();
    let checked_at_unix_ms = unix_ms();
    let mut probe = EhProbe {
        mode,
        checked_at_unix_ms,
        host: host.clone(),
        host_ok: false,
        host_detail: "未探测".into(),
        host_elapsed_ms: 0,
        host_challenge_suspected: false,
        tracker: "ehtracker.org".into(),
        tracker_ok: false,
        tracker_detail: "未探测".into(),
        tracker_elapsed_ms: 0,
        tracker_challenge_suspected: false,
        tracker_torrent_validated: false,
    };
    let client = match Client::new_with_timeout(
        &host,
        Duration::from_millis(500),
        &rules.network,
        Duration::from_secs(8),
    ) {
        Ok(c) => c,
        Err(e) => {
            probe.host_detail = e;
            probe.tracker_detail = "网络配置无效，未探测".into();
            return probe;
        }
    };

    let started = Instant::now();
    match client.get_probe_response(&format!("https://{host}/")) {
        Ok(response) => {
            probe.host_elapsed_ms = started.elapsed().as_millis();
            probe.host_challenge_suspected = challenge_suspected(response.status, &response.body);
            probe.host_ok = effective_eh_page(&response);
            let page_result = if probe.host_challenge_suspected {
                "疑似挑战或访问限制"
            } else if probe.host_ok {
                "有效 HTML 页面"
            } else {
                "HTTP 响应不是有效 EH 页面"
            };
            probe.host_detail = format!(
                "HTTP {}; {}; {} ms；DNS/TCP/TLS 分段由 reqwest blocking 未暴露",
                response.status, page_result, probe.host_elapsed_ms
            );
        }
        Err(error) => {
            probe.host_elapsed_ms = started.elapsed().as_millis();
            probe.host_detail = format!("{error}；{} ms", probe.host_elapsed_ms);
        }
    }

    let started = Instant::now();
    match client.get_probe_response("https://ehtracker.org/") {
        Ok(response) => {
            probe.tracker_elapsed_ms = started.elapsed().as_millis();
            probe.tracker_challenge_suspected =
                challenge_suspected(response.status, &response.body);
            probe.tracker_ok =
                (200..400).contains(&response.status) && !probe.tracker_challenge_suspected;
            probe.tracker_detail = format!(
                "HTTP {}; {}；{} ms；仅验证域名响应，未验证 Torrent 数据",
                response.status,
                if probe.tracker_ok {
                    "主机有响应"
                } else {
                    "不可达或受限"
                },
                probe.tracker_elapsed_ms
            );
        }
        Err(error) => {
            probe.tracker_elapsed_ms = started.elapsed().as_millis();
            probe.tracker_detail = format!("{error}；{} ms", probe.tracker_elapsed_ms);
        }
    }
    probe.checked_at_unix_ms = unix_ms();
    probe
}

/// 执行一轮订阅扫描：搜索 → 元数据 → 本地筛选 → 查下载数 → 映射确认 → 保存。
///
/// `terminal` 为 true 时把进度打到 stderr（CLI 用）；UI 通过 [`progress`] 轮询。
pub fn collect(rules: &EhRules, terminal: bool) -> Result<EhProgress, String> {
    if RUNNING.swap(true, Ordering::SeqCst) {
        return Err("已有一次扫描正在进行".into());
    }
    set_progress(EhProgress {
        running: true,
        stage: "preparing".into(),
        message: "正在准备 EH 扫描".into(),
        activity_started_at_unix_ms: unix_ms(),
        ..Default::default()
    });
    let result = collect_inner(rules, terminal);
    RUNNING.store(false, Ordering::SeqCst);
    match &result {
        Ok(p) => set_progress(p.clone()),
        Err(e) => {
            let mut p = progress();
            p.running = false;
            p.stage = "failed".into();
            p.message = e.clone();
            p.error = Some(e.clone());
            set_progress(p);
        }
    }
    result
}

fn emit(terminal: bool, stage: &str, message: String, p: &EhProgress) {
    if terminal {
        eprintln!("[eh] {message}");
    }
    let mut snapshot = p.clone();
    snapshot.stage = stage.to_string();
    snapshot.message = message;
    snapshot.activity_started_at_unix_ms = unix_ms();
    set_progress(snapshot);
}

fn collect_inner(rules: &EhRules, terminal: bool) -> Result<EhProgress, String> {
    if rules.out_dir.trim().is_empty() {
        return Err("请先选择保存目录".into());
    }
    std::fs::create_dir_all(&rules.out_dir).map_err(|e| format!("创建目录失败：{e}"))?;
    let initial_manifest = load_manifest_for_collection(Path::new(&rules.out_dir))
        .map_err(|error| error.to_string())?;
    let mut known: HashSet<String> = initial_manifest
        .items
        .iter()
        .map(|item| item.infohash.to_ascii_lowercase())
        .collect();

    let host = if rules.host.trim().is_empty() {
        DEFAULT_HOST.to_string()
    } else {
        rules.host.trim().to_string()
    };
    let client = Client::new(&host, rules.interval(), &rules.network)?;
    let mut progress = EhProgress {
        running: true,
        stage: "searching".into(),
        pages: rules.pages.clamp(1, MAX_PAGES),
        activity_started_at_unix_ms: unix_ms(),
        ..Default::default()
    };
    set_progress(progress.clone());

    // 1) 候选：跟随 nexturl 翻页（实测 ?page=N 无效）
    let mut pairs: Vec<(String, String)> = Vec::new();
    let mut url = format!(
        "https://{}/?f_search={}",
        client.host,
        urlencode(&rules.search)
    );
    for page in 1..=rules.pages.clamp(1, MAX_PAGES) {
        if !RUNNING.load(Ordering::SeqCst) {
            return Err("已取消".into());
        }
        if page > 1 {
            emit(
                terminal,
                "searching",
                format!("等待请求间隔后查询搜索第 {page}/{} 页", progress.pages),
                &progress,
            );
            std::thread::sleep(client.interval);
        }
        emit(
            terminal,
            "searching",
            format!("正在请求搜索第 {page}/{} 页", progress.pages),
            &progress,
        );
        let html = client.get(&url)?;
        for p in parse_gallery_pairs(&html, &client.host) {
            if !pairs.iter().any(|(u, _)| u == &p.0) {
                pairs.push(p);
            }
        }
        progress.page = page;
        progress.candidates = pairs.len();
        emit(
            terminal,
            "searching",
            format!("搜索第 {page} 页：累计候选 {} 条", pairs.len()),
            &progress,
        );
        match parse_next_page(&html, &client.host) {
            Some(next) => url = next,
            None => break,
        }
    }
    if pairs.is_empty() {
        progress.stage = "done".into();
        progress.running = false;
        progress.message = "没有搜索到候选画廊".into();
        return Ok(progress);
    }

    // 2) 元数据（每次 ≤25 个 gid）
    progress.stage = "metadata".into();
    emit(
        terminal,
        "metadata",
        format!("拉取 {} 条元数据", pairs.len()),
        &progress,
    );
    let mut items: Vec<serde_json::Value> = Vec::new();
    let metadata_batches = pairs.chunks(25).len();
    for (batch_index, chunk) in pairs.chunks(25).enumerate() {
        if !RUNNING.load(Ordering::SeqCst) {
            return Err("已取消".into());
        }
        emit(
            terminal,
            "metadata",
            format!(
                "正在拉取元数据：第 {}/{} 批，每批最多 25 项",
                batch_index + 1,
                metadata_batches
            ),
            &progress,
        );
        std::thread::sleep(client.interval);
        let reqs: Vec<(String, String)> = chunk
            .iter()
            .filter_map(|(u, _)| parse_gid_token(u))
            .collect();
        items.extend(client.gdata(&reqs)?);
        progress.work_total = items.len();
        emit(
            terminal,
            "metadata",
            format!(
                "元数据已完成 {}/{} 批（{} 项）",
                batch_index + 1,
                metadata_batches,
                items.len()
            ),
            &progress,
        );
    }

    // 3) 本地筛选（评分/标记/有无种子；EH 搜索做不到这些）
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
        + rules.now_offset_days * 86400;
    let mut saved = 0usize;
    progress.work_total = items.len();

    for item in items.iter() {
        if !RUNNING.load(Ordering::SeqCst) {
            return Err("已取消".into());
        }
        progress.processed += 1;
        let gid = item.get("gid").map(|g| g.to_string()).unwrap_or_default();
        let token = text_field(item, "token").to_string();
        let title = text_field(item, "title").to_string();
        let title_jpn = text_field(item, "title_jpn").to_string();
        progress.stage = "filtering".into();
        emit(
            terminal,
            "filtering",
            format!(
                "正在筛选第 {}/{} 项：{}",
                progress.processed,
                items.len(),
                if title_jpn.is_empty() {
                    &title
                } else {
                    &title_jpn
                }
            ),
            &progress,
        );
        let rating = rating_of(item);
        if rating < rules.min_rating.max(MINIMUM_RATING) {
            progress.no_rating += 1;
            continue;
        }
        if !rules.marker_ok(&title, &title_jpn) {
            progress.no_marker += 1;
            continue;
        }
        if int_field(item, "torrentcount").unwrap_or(0) <= 0 {
            progress.no_torrent += 1;
            continue;
        }

        let posted = posted_of(item);
        let age_seconds = posted.map(|timestamp| now.saturating_sub(timestamp));
        let torrents = item
            .get("torrents")
            .and_then(|t| t.as_array())
            .cloned()
            .unwrap_or_default();

        if age_seconds.is_some_and(|age| age <= MINIMUM_STABLE_AGE_SECS) {
            progress.too_recent += torrents.len();
            emit(
                terminal,
                "filtering",
                format!("发布时间不超过 3 天，按规则跳过全部种子且不查询下载数：{title_jpn}"),
                &progress,
            );
            continue;
        }
        let Some(need) = required_downloads_for_posted(&rules, posted, now) else {
            progress.no_downloads += torrents.len();
            emit(
                terminal,
                "filtering",
                format!(
                    "发布时间缺失或无效，无法应用分龄下载数门槛，已跳过该画廊全部种子：{title_jpn}"
                ),
                &progress,
            );
            continue;
        };
        let age_years = age_seconds.map(|age| age as f64 / 365.25 / 86400.0);

        // 4) 下载数（每画廊一次种子页请求）；三天内作品已在上方排除。
        progress.stage = "probing".into();
        progress.checked += 1;
        emit(
            terminal,
            "probing",
            format!("正在查询 Torrent 下载数：{}", title_jpn),
            &progress,
        );
        std::thread::sleep(client.interval);
        let stats_html = client.get(&format!(
            "https://{}/gallerytorrents.php?gid={}&t={}",
            client.host, gid, token
        ))?;
        let stats = parse_torrent_stats(&stats_html);

        // The stats page and gdata list are paired by row order. Never compact
        // a missing stats row and accidentally assign the next torrent's count.
        if stats.len() != torrents.len() {
            progress.no_downloads += torrents.len();
            emit(
                terminal,
                "filtering",
                format!(
                    "Torrent 统计行数与 gdata 列表不匹配（{} / {}），已跳过该画廊全部种子",
                    stats.len(),
                    torrents.len()
                ),
                &progress,
            );
            continue;
        }

        for (idx, t) in torrents.iter().enumerate() {
            if !RUNNING.load(Ordering::SeqCst) {
                return Err("已取消".into());
            }
            let hash = text_field(t, "hash").to_string();
            let tname = text_field(t, "name").to_string();
            if hash.len() != 40 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                progress.unmapped += 1;
                emit(
                    terminal,
                    "filtering",
                    format!("Torrent Infohash 格式无效，已跳过：{tname}"),
                    &progress,
                );
                continue;
            }
            let dl = stats
                .get(idx)
                .and_then(Option::as_ref)
                .map(|s| s.2)
                .unwrap_or(-1);
            if dl < need {
                progress.no_downloads += 1;
                emit(
                    terminal,
                    "filtering",
                    format!("Torrent 下载数未达标，已跳过：{tname}"),
                    &progress,
                );
                continue;
            }
            if !maps_to_gallery(&title_jpn, &tname, &[]) {
                progress.unmapped += 1;
                emit(
                    terminal,
                    "filtering",
                    format!("Torrent 与画廊标题映射未确认，已跳过：{tname}"),
                    &progress,
                );
                continue;
            }
            if known.contains(&hash.to_ascii_lowercase()) {
                emit(
                    terminal,
                    "filtering",
                    format!("Infohash 已在本地 manifest，跳过重复 Torrent：{tname}"),
                    &progress,
                );
                continue;
            }
            let file_name = format!("{hash}-{}.torrent", sanitize_filename(&title_jpn));
            let path = Path::new(&rules.out_dir).join(&file_name);
            let existing_bytes = match std::fs::symlink_metadata(&path) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    return Err(format!("Torrent 保存路径不能是符号链接：{file_name}"));
                }
                Ok(metadata) if !metadata.is_file() => {
                    return Err(format!("Torrent 保存路径不是普通文件：{file_name}"));
                }
                Ok(_) => {
                    let bytes = std::fs::read(&path)
                        .map_err(|error| format!("读取已有 Torrent 失败：{error}"))?;
                    torrent_v1_infohash(&bytes)
                        .filter(|actual| actual.eq_ignore_ascii_case(&hash))
                        .map(|_| bytes)
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => return Err(format!("检查已有 Torrent 失败：{error}")),
            };
            let bytes = if let Some(bytes) = existing_bytes {
                emit(
                    terminal,
                    "filtering",
                    format!("复用已校验的本地 Torrent：{file_name}"),
                    &progress,
                );
                bytes
            } else {
                if !RUNNING.load(Ordering::SeqCst) {
                    return Err("已取消".into());
                }
                emit(
                    terminal,
                    "downloading_torrent",
                    format!("正在从 Tracker 获取 Torrent：{file_name}"),
                    &progress,
                );
                std::thread::sleep(client.interval);
                let bytes = client
                    .get_bytes(&format!("https://ehtracker.org/get/{gid}/{hash}.torrent"))
                    .map_err(|e| format!("下载种子失败：{e}"))?;
                if !torrent_v1_infohash(&bytes)
                    .is_some_and(|actual| actual.eq_ignore_ascii_case(&hash))
                {
                    progress.unmapped += 1;
                    emit(
                        terminal,
                        "filtering",
                        format!("Torrent 数据无效或 infohash 不匹配，已跳过：{file_name}"),
                        &progress,
                    );
                    continue;
                }
                std::fs::write(&path, &bytes).map_err(|e| format!("写入种子失败：{e}"))?;
                bytes
            };
            let saved_item = EhSavedItem {
                infohash: hash.clone(),
                gid: gid.clone(),
                title: title.clone(),
                title_jpn: title_jpn.clone(),
                rating,
                downloads: dl,
                required_dl: need,
                posted,
                posted_utc: posted.map(fmt_utc).unwrap_or_else(|| "未知".into()),
                age_years: age_years.map(|a| (a * 10.0).round() / 10.0),
                tags: item
                    .get("tags")
                    .and_then(|x| x.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default(),
                category: text_field(item, "category").to_string(),
                filecount: int_field(item, "filecount"),
                filesize: int_field(item, "filesize"),
                uploader: text_field(item, "uploader").to_string(),
                torrent_name: tname,
                file: file_name,
                bytes: bytes.len() as i64,
                source_url: format!("https://{}/g/{}/{}", client.host, gid, token),
                saved_at_utc: fmt_utc(now),
                semantic: derive_semantic(item, true, &rules.out_dir),
            };
            // Each validated Torrent is committed before moving to another candidate.
            // A later network error or cancellation can no longer orphan earlier results.
            let added = commit_manifest_additions(
                Path::new(&rules.out_dir),
                std::slice::from_ref(&saved_item),
            )
            .map_err(|error| error.to_string())?;
            known.insert(hash.to_ascii_lowercase());
            saved += added;
            progress.saved = saved;
            emit(
                terminal,
                "filtering",
                format!("已校验并保存 {saved} 个 Torrent；当前下载数 {dl}，要求至少 {need}"),
                &progress,
            );
        }
    }

    progress.saved = saved;

    progress.running = false;
    progress.stage = "done".into();
    progress.message = format!(
        "本轮新增 {saved} 个；评分不足 {}｜标记排除 {}｜无种子 {}｜3 天内跳过 {}｜下载数不达标 {}｜映射未确认 {}",
        progress.no_rating,
        progress.no_marker,
        progress.no_torrent,
        progress.too_recent,
        progress.no_downloads,
        progress.unmapped
    );
    Ok(progress)
}

/// 请求取消当前扫描（在下一个候选边界生效）。
pub fn request_cancel() {
    RUNNING.store(false, Ordering::SeqCst);
}

/// 供 CLI / 测试复用：把 [`collect`] 包成 Arc 友好的入口。
pub fn collect_shared(rules: Arc<EhRules>, terminal: bool) -> Result<EhProgress, String> {
    collect(&rules, terminal)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    #[test]
    fn legacy_rules_round_trip_with_system_proxy_mode() {
        let rules = EhRules::from_json(r#"{"search":"language:chinese$","pages":3}"#)
            .expect("legacy rules should remain valid");

        let saved: serde_json::Value = serde_json::from_str(&rules.to_json()).unwrap();
        assert_eq!(saved["search"], "language:chinese$");
        assert_eq!(saved["network"]["mode"], "system");
    }

    #[test]
    fn daily_automation_state_survives_rust_rules_round_trip() {
        let rules = EhRules::from_json(
            r#"{
                "automation": {
                    "daily_enabled": true,
                    "last_auto_attempt_date": "2026-10-11",
                    "last_run": {
                        "date": "2026-10-11",
                        "source": "daily_auto",
                        "status": "completed",
                        "saved": 3,
                        "error": null
                    }
                }
            }"#,
        )
        .expect("automation state should be accepted as an optional config section");

        let round_trip: serde_json::Value = serde_json::from_str(&rules.to_json()).unwrap();
        assert_eq!(round_trip["automation"]["daily_enabled"], true);
        assert_eq!(
            round_trip["automation"]["last_auto_attempt_date"],
            "2026-10-11"
        );
        assert_eq!(round_trip["automation"]["last_run"]["saved"], 3);
    }

    #[test]
    fn invalid_manual_proxy_is_rejected_without_echoing_credentials() {
        let result = EhRules::from_json(
            r#"{"network":{"mode":"manual","proxy_url":"http://user:secret@/"}}"#,
        );

        let error = match result {
            Err(error) => error,
            Ok(_) => panic!("manual proxy URLs without a host must be rejected"),
        };
        assert!(!error.contains("secret"));
    }

    #[test]
    fn challenge_detection_is_case_insensitive() {
        assert!(is_challenge("<TITLE>Just A Moment</TITLE>"));
        assert!(is_challenge("<title>Attention Required</title>"));
        assert!(is_challenge("cf-challenge"));
        assert!(!is_challenge(
            "<html><title>EH</title><body>gallery</body></html>"
        ));
    }

    #[test]
    fn manual_socks5h_proxy_with_credentials_is_validated() {
        let rules = EhRules::from_json(
            r#"{"network":{"mode":"manual","proxy_url":"socks5h://user:secret@127.0.0.1:7891"}}"#,
        )
        .expect("SOCKS5H with a valid endpoint should be accepted");

        assert_eq!(rules.network.mode, EhProxyMode::Manual);
        assert_eq!(
            rules.network.proxy_url,
            "socks5h://user:secret@127.0.0.1:7891"
        );
    }

    #[test]
    fn invalid_manual_proxy_scheme_is_rejected() {
        let result =
            EhRules::from_json(r#"{"network":{"mode":"manual","proxy_url":"ftp://127.0.0.1:21"}}"#);

        assert!(result.is_err());
    }

    fn serve_once(
        listener: TcpListener,
        response_body: &'static str,
    ) -> std::thread::JoinHandle<String> {
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("request should reach listener");
            let mut request = [0_u8; 2048];
            let count = stream
                .read(&mut request)
                .expect("request should be readable");
            let request = String::from_utf8_lossy(&request[..count]).to_string();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response_body.len(),
                response_body
            )
            .expect("test response should be writable");
            request
        })
    }

    #[test]
    fn manual_proxy_receives_http_request_for_unresolvable_origin() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = serve_once(listener, "proxy response");
        let network = EhNetworkSettings {
            mode: EhProxyMode::Manual,
            proxy_url: format!("http://{address}"),
        };
        let client = Client::new("e-hentai.org", Duration::ZERO, &network).unwrap();

        let body = client
            .http
            .get("http://origin-that-does-not-resolve.invalid/check")
            .send()
            .unwrap()
            .text()
            .unwrap();
        let request = server.join().unwrap();

        assert_eq!(body, "proxy response");
        assert!(request.starts_with("GET http://origin-that-does-not-resolve.invalid/check "));
    }

    #[test]
    fn direct_mode_ignores_a_saved_manual_proxy_url() {
        let origin = TcpListener::bind("127.0.0.1:0").unwrap();
        let origin_address = origin.local_addr().unwrap();
        let proxy_reservation = TcpListener::bind("127.0.0.1:0").unwrap();
        let dead_proxy_address = proxy_reservation.local_addr().unwrap();
        drop(proxy_reservation);
        let server = serve_once(origin, "origin response");
        let network = EhNetworkSettings {
            mode: EhProxyMode::Direct,
            proxy_url: format!("http://{dead_proxy_address}"),
        };
        let client = Client::new("e-hentai.org", Duration::ZERO, &network).unwrap();

        let body = client
            .http
            .get(format!("http://{origin_address}/direct"))
            .send()
            .unwrap()
            .text()
            .unwrap();
        let request = server.join().unwrap();

        assert_eq!(body, "origin response");
        assert!(request.starts_with("GET /direct "));
    }

    #[test]
    fn manual_proxy_failure_does_not_fall_back_to_direct() {
        let origin = TcpListener::bind("127.0.0.1:0").unwrap();
        origin.set_nonblocking(true).unwrap();
        let origin_address = origin.local_addr().unwrap();
        let proxy_reservation = TcpListener::bind("127.0.0.1:0").unwrap();
        let dead_proxy_address = proxy_reservation.local_addr().unwrap();
        drop(proxy_reservation);
        let origin_monitor = std::thread::spawn(move || {
            for _ in 0..50 {
                match origin.accept() {
                    Ok((mut stream, _)) => {
                        let _ =
                            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\n\r\norigin");
                        return true;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("unexpected listener error: {error}"),
                }
            }
            false
        });
        let network = EhNetworkSettings {
            mode: EhProxyMode::Manual,
            proxy_url: format!("http://{dead_proxy_address}"),
        };
        let client = Client::new("e-hentai.org", Duration::ZERO, &network).unwrap();

        let response = client
            .http
            .get(format!("http://{origin_address}/must-not-fallback"))
            .send();

        assert!(response.is_err());
        assert!(!origin_monitor.join().unwrap());
    }

    #[test]
    fn required_dl_matches_declared_tiers() {
        let r = EhRules::default();
        assert_eq!(r.required_dl(10.0), 2000, "五年前应要求 2000");
        assert_eq!(r.required_dl(3.0), 900, "3 年应要求 900");
        assert_eq!(r.required_dl(1.0), 500, "1 年应要求 500");
        assert_eq!(r.required_dl(0.2), 300, "约 2 个月应要求 300");
        assert_eq!(r.required_dl(0.01), 20, "超过 3 天的最年轻档要求 20");
    }

    #[test]
    fn marker_rules_allow_and_exclude() {
        let r = EhRules::default();
        assert!(r.marker_ok("x", "[DL版] foo"));
        assert!(r.marker_ok("[Digital] x", ""));
        assert!(!r.marker_ok("x", "[AI Generated] foo"), "排除名单优先");
        assert!(!r.marker_ok("x", "no marker here"));
        let any = EhRules {
            title_markers: "any".into(),
            exclude_markers: vec![],
            ..Default::default()
        };
        assert!(any.marker_ok("whatever", ""));
    }

    #[test]
    fn posted_is_string_and_must_not_silently_become_zero() {
        let item = serde_json::json!({"posted": "1790042681"});
        assert_eq!(posted_of(&item), Some(1790042681));
        // 缺失/非法必须返回 None，而不是 0（0 会让画龄恒为 0、分档失效）
        assert_eq!(posted_of(&serde_json::json!({})), None);
        assert_eq!(posted_of(&serde_json::json!({"posted": "abc"})), None);
        assert_eq!(posted_of(&serde_json::json!({"posted": 0})), None);
    }

    #[test]
    fn missing_posted_date_does_not_use_the_youngest_download_tier() {
        let rules = EhRules::default();
        let now = 1_800_000_000;
        assert_eq!(required_downloads_for_posted(&rules, None, now), None);
        assert_eq!(
            required_downloads_for_posted(&rules, Some(now - 10 * 86400), now),
            Some(100)
        );
    }

    #[test]
    fn next_page_cursor_comes_from_script_variable() {
        let html = r#"<script>var prevurl=""; var nexturl="https://e-hentai.org/?f_search=a&amp;next=4201693";</script>"#;
        assert_eq!(
            parse_next_page(html, DEFAULT_HOST).as_deref(),
            Some("https://e-hentai.org/?f_search=a&next=4201693")
        );
        assert!(parse_next_page("<html>no pager</html>", DEFAULT_HOST).is_none());
    }

    #[test]
    fn gallery_pairs_and_tokens_parse() {
        let html = r#"<a href="https://e-hentai.org/g/4204783/6a915060bd/"><div class="glink">T</div></a>"#;
        let pairs = parse_gallery_pairs(html, DEFAULT_HOST);
        assert_eq!(pairs.len(), 1);
        assert_eq!(
            parse_gid_token(&pairs[0].0),
            Some(("4204783".into(), "6a915060bd".into()))
        );
        assert_eq!(pairs[0].1, "T");
    }

    #[test]
    fn torrent_stats_parse_seeds_peers_downloads() {
        let html = "<div>Seeds: 124 Peers: 4 Downloads: 924</div><div>Seeds: 0 Peers: 7 Downloads: 5</div>";
        assert_eq!(
            parse_torrent_stats(html),
            vec![Some((124, 4, 924)), Some((0, 7, 5))]
        );
    }

    #[test]
    fn malformed_torrent_stats_row_does_not_borrow_numbers_from_next_row() {
        let html = "<div>Seeds: 12 Peers: 3 Downloads: 321</div><div>Seeds: n/a Peers: 0 Downloads: 4000</div><div>Seeds: 5 Peers: 6 Downloads: 123</div>";
        assert_eq!(
            parse_torrent_stats(html),
            vec![Some((12, 3, 321)), None, Some((5, 6, 123))]
        );
    }

    #[test]
    fn mapping_uses_japanese_title() {
        // 罗马字标题与种子名不匹配，日文原名才匹配（实测结论）
        let jpn = "[赤月屋 (赤月みゅうと)] 僕にしか触れないサキュバス三姉妹に搾られる話4";
        let torrent = "[赤月屋 (赤月みゅうと)] 僕にしか触れないサキュバス三姉妹に搾られる話4〜長女レミィ編(前編)〜.zip";
        assert!(maps_to_gallery(jpn, torrent, &[]));
        assert!(!maps_to_gallery(
            "Akatsuki Myuuto Boku ni shika Furenai",
            torrent,
            &[]
        ));
    }

    #[test]
    fn filename_sanitizing_decodes_entities_and_strips_illegal() {
        assert_eq!(sanitize_filename("Vパン&#039;s/黒:bad"), "Vパン's_黒_bad");
        assert_eq!(sanitize_filename("   "), "untitled");
    }

    #[test]
    fn rules_roundtrip_json_and_tolerate_missing_keys() {
        let r = EhRules::default();
        let back = EhRules::from_json(&r.to_json()).expect("roundtrip");
        assert_eq!(r, back);
        // 只有部分键时，其余取默认值（用户可只写想改的字段）
        let partial = EhRules::from_json(r#"{"min_rating":3.5}"#).expect("partial");
        assert_eq!(partial.min_rating, 3.5);
        assert_eq!(partial.pages, EhRules::default().pages);
    }

    #[test]
    fn atomic_rule_save_replaces_existing_config_without_losing_daily_state() {
        let dir = TestDir::new();
        let path = dir.path().join("eh_subscription_rules.json");
        std::fs::write(&path, r#"{"search":"old"}"#).unwrap();
        let mut rules = EhRules::default();
        rules.automation.daily_enabled = true;
        rules.automation.last_auto_attempt_date = Some("2026-10-11".into());
        rules.automation.last_run = Some(EhAutomationRun {
            date: "2026-10-11".into(),
            source: "daily_auto".into(),
            status: "claimed".into(),
            saved: 0,
            error: None,
        });

        save_rules_atomic(&path, &rules).expect("validated atomic config replacement");

        let saved = EhRules::from_json(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(saved, rules);
    }

    #[test]
    fn int_field_accepts_both_string_and_number() {
        let v = serde_json::json!({"a": "42", "b": 42, "c": "x"});
        assert_eq!(int_field(&v, "a"), Some(42));
        assert_eq!(int_field(&v, "b"), Some(42));
        assert_eq!(int_field(&v, "c"), None);
        assert_eq!(int_field(&v, "missing"), None);
    }

    fn sample_gallery() -> serde_json::Value {
        // 取自 2026-09-22 实测的真实样本（gid=4202535，30 个标签）
        serde_json::json!({
            "gid": 4202535,
            "token": "2b4fcbcc38",
            "title": "Akatsuki Myuuto Boku ni shika Furenai Succubus Sanshimai",
            "title_jpn": "[赤月屋 (赤月みゅうと)] 僕にしか触れないサキュバス三姉妹に搾られる話4 [中国翻訳] [無修正] [DL版]",
            "tags": [
                "language:chinese", "language:translated", "parody:original",
                "group:akatukiya", "artist:akatsuki myuuto",
                "male:blindfold", "male:bondage", "female:big breasts", "female:nakadashi",
                "other:uncensored", "other:multi-work series"
            ]
        })
    }

    #[test]
    fn semantic_layer_aligns_with_scraper_vocabulary() {
        let sem = derive_semantic(&sample_gallery(), false, "");
        assert!(
            sem.work_title.contains("赤月屋"),
            "work_title 应取 title_jpn"
        );
        assert!(!sem.title_aliases.is_empty(), "罗马字标题应进别名");

        // creators：artist / group 分开且带角色
        let roles: Vec<&str> = sem.creators.iter().map(|c| c.role.as_str()).collect();
        assert!(roles.contains(&"artist") && roles.contains(&"group"));
        assert!(sem.creators.iter().any(|c| c.name == "akatsuki myuuto"));

        // 系列 / 语言 / 状态位
        assert_eq!(sem.source_series, vec!["original".to_string()]);
        assert_eq!(sem.resource_language.as_deref(), Some("chinese"));
        assert_eq!(sem.translation_state.as_deref(), Some("translated"));
        assert_eq!(sem.censorship.as_deref(), Some("uncensored"));
        assert_eq!(sem.color_state, None);

        // other:uncensored 归入 censorship，不应再重复出现在 resource_tags
        assert!(!sem.resource_tags.iter().any(|t| t == "other:uncensored"));
        assert!(sem.resource_tags.iter().any(|t| t == "female:big breasts"));
        assert!(sem
            .resource_tags
            .iter()
            .any(|t| t == "other:multi-work series"));

        // 中文译名与 resource_tags 一一对应
        assert_eq!(sem.resource_tags.len(), sem.resource_tags_zh.len());
        let zh_of = |raw: &str| {
            let i = sem
                .resource_tags
                .iter()
                .position(|t| t == raw)
                .expect("tag 存在");
            sem.resource_tags_zh[i].clone()
        };
        assert_eq!(zh_of("female:big breasts"), "巨乳");
        assert_eq!(zh_of("female:nakadashi"), "中出");
        assert_eq!(zh_of("male:bondage"), "束缚");
    }

    #[test]
    fn old_manifest_without_semantic_still_deserializes() {
        // 旧清单没有 semantic 字段（v1）；必须能读回，且语义层为空默认值
        let old = r#"[{"infohash":"abc","gid":"1","title":"t","title_jpn":"tj","rating":4.5,
            "downloads":900,"required_dl":800,"posted":null,"posted_utc":"未知","age_years":null,
            "tags":["female:big breasts"],"category":"Doujinshi","filecount":10,"filesize":100,
            "uploader":"u","torrent_name":"n","file":"f.torrent","bytes":9,
            "source_url":"s","saved_at_utc":"now"}]"#;
        let items: Vec<EhSavedItem> = serde_json::from_str(old).expect("旧清单应可反序列化");
        assert_eq!(items.len(), 1);
        assert_eq!(
            items[0].semantic,
            EhSemantic::default(),
            "缺失的语义层应为默认值"
        );
    }

    #[test]
    fn match_anchors_are_offline_ordered() {
        // 纯逻辑：作品名优先、创作者去重（不联网）
        let a = match_gallery_anchors(
            "[赤月屋 (赤月みゅうと)] 僕にしか触れないサキュバス三姉妹に搾られる話4 [中国翻訳]",
            &[
                "赤月みゅうと".into(),
                "赤月屋".into(),
                "赤月みゅうと".into(),
            ],
        );
        assert_eq!(a[0], "僕にしか触れないサキュバス三姉妹に搾られる話4");
        assert_eq!(a.len(), 3, "重复创作者名应去重");
        assert!(match_gallery_anchors("", &[]).is_empty());
    }

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "rch-eh-handoff-{}-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    const HANDOFF_TORRENT: &[u8] = b"d8:announce23:http://tracker/announce4:infod6:lengthi123e4:name4:test12:piece lengthi16384e6:pieces20:aaaaaaaaaaaaaaaaaaaaee";
    const HANDOFF_INFOHASH: &str = "54ada1b6bfdf4e7792774b033f0026c616dcd0cb";

    fn handoff_item(file: &str, bytes: i64) -> EhSavedItem {
        EhSavedItem {
            infohash: HANDOFF_INFOHASH.into(),
            gid: "1".into(),
            title: "title".into(),
            title_jpn: "标题 & 漫画".into(),
            rating: 4.5,
            downloads: 10,
            required_dl: 0,
            posted: None,
            posted_utc: "2026-10-11".into(),
            age_years: None,
            tags: Vec::new(),
            category: "Doujinshi".into(),
            filecount: Some(1),
            filesize: Some(123),
            uploader: "uploader".into(),
            torrent_name: "torrent".into(),
            file: file.into(),
            bytes,
            source_url: "https://example.invalid/".into(),
            saved_at_utc: "2026-10-11T00:00:00Z".into(),
            semantic: EhSemantic::default(),
        }
    }

    fn write_handoff_manifest(root: &Path, value: &serde_json::Value) {
        std::fs::write(
            root.join("manifest.json"),
            serde_json::to_vec(value).unwrap(),
        )
        .unwrap();
    }

    fn manifest_item(infohash: &str, file: &str) -> EhSavedItem {
        let mut item = handoff_item(file, HANDOFF_TORRENT.len() as i64);
        item.infohash = infohash.into();
        item
    }

    fn manifest_temp_files(root: &Path) -> Vec<PathBuf> {
        std::fs::read_dir(root)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.contains(".rch-tmp-"))
            })
            .collect()
    }

    #[test]
    fn collector_manifest_reader_distinguishes_missing_empty_damaged_and_unreadable() {
        let root = TestDir::new();
        assert!(matches!(
            load_manifest_for_collection(root.path()),
            Ok(snapshot) if snapshot.items.is_empty()
        ));

        std::fs::write(root.path().join("manifest.json"), b"[]").unwrap();
        assert!(matches!(
            load_manifest_for_collection(root.path()),
            Ok(snapshot) if snapshot.items.is_empty()
        ));

        let damaged = b"[{not-json";
        std::fs::write(root.path().join("manifest.json"), damaged).unwrap();
        assert!(matches!(
            load_manifest_for_collection(root.path()),
            Err(ManifestReadError::Damaged(_))
        ));
        assert_eq!(
            std::fs::read(root.path().join("manifest.json")).unwrap(),
            damaged
        );

        assert!(matches!(
            load_manifest_from_reader(&root.path().join("manifest.json"), |_| {
                Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "injected unreadable manifest",
                ))
            }),
            Err(ManifestReadError::Unreadable(error))
                if error.kind() == std::io::ErrorKind::PermissionDenied
        ));
    }

    #[test]
    fn damaged_manifest_stops_collection_before_client_creation_or_network_work() {
        let root = TestDir::new();
        let manifest = root.path().join("manifest.json");
        let damaged = b"not-json";
        std::fs::write(&manifest, damaged).unwrap();

        let mut rules = EhRules::default();
        rules.out_dir = root.path().to_string_lossy().into_owned();
        rules.network.mode = EhProxyMode::Manual;
        rules.network.proxy_url = "not a proxy URL".into();

        let error = collect_inner(&rules, false).unwrap_err();
        assert!(
            error.contains("manifest damaged"),
            "unexpected error: {error}"
        );
        assert_eq!(std::fs::read(&manifest).unwrap(), damaged);
        assert_eq!(
            std::fs::read_dir(root.path())
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "torrent"))
                .count(),
            0
        );
    }

    #[test]
    fn atomic_manifest_write_failures_leave_old_history_readable_and_remove_own_temp() {
        let root = TestDir::new();
        let target = root.path().join("manifest.json");
        let old = serde_json::json!([{
            "infohash": "old-history",
            "file": "old.torrent",
            "future_field": "preserved"
        }]);
        let rows = serde_json::json!([{
            "infohash": "new-history",
            "file": "new.torrent"
        }]);
        let old_bytes = serde_json::to_vec_pretty(&old).unwrap();
        let new_bytes = serde_json::to_vec_pretty(&rows).unwrap();
        std::fs::write(&target, &old_bytes).unwrap();

        let write_error = atomic_write_manifest_with(
            &target,
            rows.as_array().unwrap(),
            &new_bytes,
            |file, _| {
                file.write_all(b"[")?;
                Err(std::io::Error::new(
                    std::io::ErrorKind::WriteZero,
                    "injected temp write failure",
                ))
            },
            |source, destination| std::fs::rename(source, destination),
        )
        .unwrap_err();
        assert_eq!(write_error.kind(), std::io::ErrorKind::WriteZero);
        assert_eq!(std::fs::read(&target).unwrap(), old_bytes);
        assert!(manifest_temp_files(root.path()).is_empty());

        let replace_error = atomic_write_manifest_with(
            &target,
            rows.as_array().unwrap(),
            &new_bytes,
            |file, bytes| {
                file.write_all(bytes)?;
                file.flush()?;
                file.sync_all()
            },
            |_, _| {
                Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "injected replace failure",
                ))
            },
        )
        .unwrap_err();
        assert_eq!(replace_error.kind(), std::io::ErrorKind::PermissionDenied);
        assert_eq!(std::fs::read(&target).unwrap(), old_bytes);
        assert!(manifest_temp_files(root.path()).is_empty());
    }

    #[test]
    fn manifest_commit_creates_appends_deduplicates_and_preserves_unknown_history() {
        let root = TestDir::new();
        let out_dir = root.path().join("EH 订阅 & 测试 [A]");
        std::fs::create_dir_all(&out_dir).unwrap();
        let target = out_dir.join("manifest.json");
        let first = manifest_item(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "中文 & one.torrent",
        );

        assert_eq!(
            commit_manifest_additions(&out_dir, &[first.clone()]).unwrap(),
            1
        );
        let loaded = load_manifest_for_collection(&out_dir).unwrap();
        assert_eq!(loaded.items.len(), 1);
        assert_eq!(loaded.items[0].file, first.file);

        let mut old_row = serde_json::to_value(&first).unwrap();
        old_row["future_field"] = serde_json::json!({ "keep": true });
        std::fs::write(&target, serde_json::to_vec_pretty(&vec![old_row]).unwrap()).unwrap();
        let second = manifest_item("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", "第二个.torrent");
        assert_eq!(
            commit_manifest_additions(&out_dir, &[second.clone()]).unwrap(),
            1
        );

        let after_append = load_manifest_for_collection(&out_dir).unwrap();
        assert_eq!(after_append.items.len(), 2);
        assert_eq!(after_append.rows[0]["future_field"]["keep"], true);
        let committed_bytes = std::fs::read(&target).unwrap();
        let uppercase_duplicate =
            manifest_item("BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB", "第二个.torrent");
        assert_eq!(
            commit_manifest_additions(&out_dir, &[uppercase_duplicate]).unwrap(),
            0
        );
        assert_eq!(std::fs::read(&target).unwrap(), committed_bytes);
        assert!(manifest_temp_files(&out_dir).is_empty());
        let names = std::fs::read_dir(&out_dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        assert_eq!(names, vec![std::ffi::OsString::from("manifest.json")]);
    }

    #[test]
    fn abandoned_temp_manifest_is_never_read_as_the_official_manifest() {
        let root = TestDir::new();
        let stale = root.path().join(".manifest.json.rch-tmp-stale");
        std::fs::write(&stale, b"[{\"looks\":\"valid\"}]").unwrap();

        assert!(load_manifest_for_collection(root.path())
            .unwrap()
            .items
            .is_empty());
        assert_eq!(
            read_manifest_checked(root.path()).status,
            EhManifestReadStatus::Missing
        );
        assert!(stale.exists());
    }

    #[test]
    fn concurrent_manifest_commits_serialize_and_preserve_both_updates() {
        let root = TestDir::new();
        let out_dir = root.path().join("并发 & 收藏");
        std::fs::create_dir_all(&out_dir).unwrap();
        let barrier = Arc::new(std::sync::Barrier::new(3));
        let mut workers = Vec::new();

        for (hash, file) in [
            ("cccccccccccccccccccccccccccccccccccccccc", "one.torrent"),
            ("dddddddddddddddddddddddddddddddddddddddd", "two.torrent"),
        ] {
            let barrier = Arc::clone(&barrier);
            let out_dir = out_dir.clone();
            let item = manifest_item(hash, file);
            workers.push(std::thread::spawn(move || {
                barrier.wait();
                commit_manifest_additions(&out_dir, &[item])
            }));
        }

        barrier.wait();
        for worker in workers {
            assert_eq!(worker.join().unwrap().unwrap(), 1);
        }
        let loaded = load_manifest_for_collection(&out_dir).unwrap();
        assert_eq!(loaded.items.len(), 2);
        assert!(loaded.items.iter().any(|item| item.file == "one.torrent"));
        assert!(loaded.items.iter().any(|item| item.file == "two.torrent"));
    }

    #[test]
    fn checked_manifest_reports_missing_empty_and_damaged_without_rewriting() {
        let root = TestDir::new();
        assert_eq!(
            read_manifest_checked(root.path()).status,
            EhManifestReadStatus::Missing
        );

        write_handoff_manifest(root.path(), &serde_json::json!([]));
        assert_eq!(
            read_manifest_checked(root.path()).status,
            EhManifestReadStatus::Empty
        );

        let damaged = b"[{not-json";
        std::fs::write(root.path().join("manifest.json"), damaged).unwrap();
        let report = read_manifest_checked(root.path());
        assert_eq!(report.status, EhManifestReadStatus::Damaged);
        assert_eq!(
            std::fs::read(root.path().join("manifest.json")).unwrap(),
            damaged,
            "a failed read must never replace or clear the existing manifest"
        );
    }

    #[test]
    fn checked_manifest_keeps_valid_rows_when_one_row_is_malformed() {
        let root = TestDir::new();
        std::fs::write(root.path().join("ok.torrent"), HANDOFF_TORRENT).unwrap();
        let item = handoff_item("ok.torrent", HANDOFF_TORRENT.len() as i64);
        let mut rows = vec![serde_json::to_value(item).unwrap(), serde_json::json!(7)];
        write_handoff_manifest(
            root.path(),
            &serde_json::Value::Array(std::mem::take(&mut rows)),
        );

        let report = read_manifest_checked(root.path());
        assert_eq!(report.status, EhManifestReadStatus::Damaged);
        assert_eq!(report.invalid_entries, 1);
        assert_eq!(report.entries.len(), 1);
        assert_eq!(
            report.entries[0].validation_status,
            EhTorrentValidationStatus::Valid
        );
    }

    #[test]
    fn checked_manifest_verifies_bencode_infohash_and_file_size() {
        let root = TestDir::new();
        std::fs::write(root.path().join("valid.torrent"), HANDOFF_TORRENT).unwrap();
        let item = handoff_item("valid.torrent", HANDOFF_TORRENT.len() as i64);
        write_handoff_manifest(
            root.path(),
            &serde_json::json!([serde_json::to_value(item).unwrap()]),
        );

        let report = read_manifest_checked(root.path());
        assert_eq!(report.status, EhManifestReadStatus::Ready);
        assert_eq!(report.entries.len(), 1);
        assert_eq!(report.entries[0].file_status, EhSavedFileStatus::Present);
        assert_eq!(
            report.entries[0].validation_status,
            EhTorrentValidationStatus::Valid
        );
        assert!(report.entries[0].full_path.is_some());
    }

    #[test]
    fn checked_torrent_directory_uses_top_level_files_without_manifest() {
        let root = TestDir::new();
        let valid_path = root.path().join("有效 & [测试].torrent");
        std::fs::write(&valid_path, HANDOFF_TORRENT).unwrap();
        std::fs::write(root.path().join("损坏.torrent"), b"not bencoded").unwrap();
        std::fs::write(root.path().join("ignore.txt"), b"not a torrent").unwrap();
        let nested = root.path().join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("nested.torrent"), HANDOFF_TORRENT).unwrap();

        let report = read_torrent_directory_checked(root.path());

        assert_eq!(report.status, EhManifestReadStatus::Ready);
        assert_eq!(report.entries.len(), 2);
        assert!(
            report.entries.iter().any(|entry| {
                entry.item.file == "有效 & [测试].torrent"
                    && entry.item.infohash == HANDOFF_INFOHASH
                    && entry.file_status == EhSavedFileStatus::Present
                    && entry.validation_status == EhTorrentValidationStatus::Valid
            }),
            "valid top-level torrent should be discovered and checked"
        );
        assert!(
            report.entries.iter().any(|entry| {
                entry.item.file == "损坏.torrent"
                    && entry.file_status == EhSavedFileStatus::Present
                    && entry.validation_status == EhTorrentValidationStatus::InvalidTorrent
            }),
            "invalid top-level torrent should remain visible as invalid"
        );
        assert!(!root.path().join("manifest.json").exists());
    }

    #[test]
    fn checked_torrent_directory_rejects_symlink_entries() {
        let root = TestDir::new();
        let outside = root.path().parent().unwrap().join(format!(
            "{}-outside-directory.torrent",
            root.path().file_name().unwrap().to_string_lossy()
        ));
        let link = root.path().join("outside.torrent");
        std::fs::write(&outside, HANDOFF_TORRENT).unwrap();

        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(&outside, &link).unwrap();

        let report = read_torrent_directory_checked(root.path());

        assert_eq!(report.entries.len(), 1);
        assert_eq!(report.entries[0].file_status, EhSavedFileStatus::UnsafePath);
        assert_eq!(
            report.entries[0].validation_status,
            EhTorrentValidationStatus::NotChecked
        );
        assert_eq!(report.entries[0].full_path, None);
        std::fs::remove_file(outside).unwrap();
    }

    #[test]
    fn checked_manifest_distinguishes_missing_and_invalid_torrents() {
        let root = TestDir::new();
        std::fs::write(root.path().join("invalid.torrent"), b"not bencoded").unwrap();
        let missing = handoff_item("missing.torrent", 1);
        let invalid = handoff_item("invalid.torrent", 12);
        write_handoff_manifest(
            root.path(),
            &serde_json::json!([
                serde_json::to_value(missing).unwrap(),
                serde_json::to_value(invalid).unwrap()
            ]),
        );

        let report = read_manifest_checked(root.path());
        assert_eq!(report.entries[0].file_status, EhSavedFileStatus::Missing);
        assert_eq!(
            report.entries[0].validation_status,
            EhTorrentValidationStatus::NotChecked
        );
        assert_eq!(report.entries[1].file_status, EhSavedFileStatus::Present);
        assert_eq!(
            report.entries[1].validation_status,
            EhTorrentValidationStatus::InvalidTorrent
        );
    }

    #[test]
    fn checked_manifest_reports_size_and_infohash_mismatches() {
        let root = TestDir::new();
        std::fs::write(root.path().join("hash-mismatch.torrent"), HANDOFF_TORRENT).unwrap();
        std::fs::write(root.path().join("size-mismatch.torrent"), HANDOFF_TORRENT).unwrap();
        let mut hash_mismatch = handoff_item("hash-mismatch.torrent", HANDOFF_TORRENT.len() as i64);
        hash_mismatch.infohash = "0000000000000000000000000000000000000000".into();
        let size_mismatch = handoff_item("size-mismatch.torrent", HANDOFF_TORRENT.len() as i64 - 1);
        write_handoff_manifest(
            root.path(),
            &serde_json::json!([
                serde_json::to_value(hash_mismatch).unwrap(),
                serde_json::to_value(size_mismatch).unwrap()
            ]),
        );

        let report = read_manifest_checked(root.path());
        assert_eq!(
            report.entries[0].validation_status,
            EhTorrentValidationStatus::InfohashMismatch
        );
        assert_eq!(
            report.entries[1].validation_status,
            EhTorrentValidationStatus::SizeMismatch
        );
    }

    #[test]
    fn checked_manifest_rejects_path_traversal_without_resolving_outside_file() {
        let root = TestDir::new();
        let outside = root.path().parent().unwrap().join(format!(
            "{}-outside.torrent",
            root.path().file_name().unwrap().to_string_lossy()
        ));
        std::fs::write(&outside, HANDOFF_TORRENT).unwrap();
        let item = handoff_item("../outside.torrent", HANDOFF_TORRENT.len() as i64);
        let windows_item = handoff_item(r"..\outside.torrent", HANDOFF_TORRENT.len() as i64);
        write_handoff_manifest(
            root.path(),
            &serde_json::json!([
                serde_json::to_value(item).unwrap(),
                serde_json::to_value(windows_item).unwrap()
            ]),
        );

        let report = read_manifest_checked(root.path());
        for entry in &report.entries {
            assert_eq!(entry.file_status, EhSavedFileStatus::UnsafePath);
            assert_eq!(entry.full_path, None);
        }
        std::fs::remove_file(outside).unwrap();
    }

    #[test]
    fn checked_manifest_rejects_symlink_escape() {
        let root = TestDir::new();
        let outside = root.path().parent().unwrap().join(format!(
            "{}-symlink-target.torrent",
            root.path().file_name().unwrap().to_string_lossy()
        ));
        let link = root.path().join("escaped.torrent");
        std::fs::write(&outside, HANDOFF_TORRENT).unwrap();

        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(&outside, &link).unwrap();

        let item = handoff_item("escaped.torrent", HANDOFF_TORRENT.len() as i64);
        write_handoff_manifest(
            root.path(),
            &serde_json::json!([serde_json::to_value(item).unwrap()]),
        );

        let report = read_manifest_checked(root.path());
        assert_eq!(report.entries[0].file_status, EhSavedFileStatus::UnsafePath);
        assert_eq!(report.entries[0].full_path, None);
        std::fs::remove_file(outside).unwrap();
    }

    #[test]
    fn checked_manifest_accepts_chinese_and_special_character_paths() {
        let root = TestDir::new();
        let nested = root.path().join("中文 #1");
        std::fs::create_dir_all(&nested).unwrap();
        let file = nested.join("漫画 & [测试].torrent");
        std::fs::write(&file, HANDOFF_TORRENT).unwrap();
        let item = handoff_item(
            "中文 #1/漫画 & [测试].torrent",
            HANDOFF_TORRENT.len() as i64,
        );
        write_handoff_manifest(
            root.path(),
            &serde_json::json!([serde_json::to_value(item).unwrap()]),
        );

        let report = read_manifest_checked(root.path());
        assert_eq!(report.entries[0].file_status, EhSavedFileStatus::Present);
        assert_eq!(
            report.entries[0].validation_status,
            EhTorrentValidationStatus::Valid
        );
    }

    #[test]
    fn legacy_manifest_reader_keeps_its_missing_or_corrupt_as_empty_behavior() {
        let root = TestDir::new();
        assert!(read_manifest(root.path().to_str().unwrap()).is_empty());
        std::fs::write(root.path().join("manifest.json"), b"bad json").unwrap();
        assert!(read_manifest(root.path().to_str().unwrap()).is_empty());
        assert_eq!(
            read_manifest_checked(root.path()).status,
            EhManifestReadStatus::Damaged
        );
    }
}
