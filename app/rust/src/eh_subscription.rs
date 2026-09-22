//! EH 订阅（可选插件）：按可编辑规则筛选 E-Hentai 画廊种子并保存到指定文件夹。
//!
//! 设计边界（与 SPEC §12「不做在线漫画站聚合」并存的插件边界，见
//! `docs/research/eh-subscription-115-feasibility.md` §5.1）：
//! * **只读来源**：仅访问 EH 元数据接口与 `.torrent` 文件；绝不读取图片、不解析压缩包内容。
//! * **写出口唯一**：只写 `<out_dir>/*.torrent` 与 `<out_dir>/manifest.json`；
//!   不触碰书源、SQLite 目录、canonical metadata 或任何阅读数据。
//! * **默认不自动运行**：必须由用户在设置页显式点击。
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
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

pub const DEFAULT_HOST: &str = "e-hentai.org";
/// 页数安全上限。单查询总量约 440 页；这里给足够大的上限，避免误操作把一轮跑成几小时。
pub const MAX_PAGES: usize = 500;
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

impl Default for AgeTier {
    fn default() -> Self {
        Self { min_age_years: 0.0, min_downloads: 0 }
    }
}

/// 订阅筛选规则。整体可序列化为 JSON 供用户直接编辑。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct EhRules {
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
}

impl Default for EhRules {
    fn default() -> Self {
        Self {
            search: "language:chinese$ uncensored".into(),
            min_rating: 4.0,
            title_markers: "Digital|DL版|DL".into(),
            exclude_markers: vec!["AI Generated".into()],
            // 用户口径：时间越久要求越高（五年前 800）；最年轻一档不设门槛，
            // 由"重复扫描 + manifest 去重"自然实现"累积到达标线再保存"。
            age_tiers: vec![
                // 用户口径（2026-09-22）：分时间段，时间越久要求越高；
                // 在首版（800/500/300/100/0）基础上各档 +200。
                AgeTier { min_age_years: 5.0, min_downloads: 1000 },
                AgeTier { min_age_years: 2.0, min_downloads: 700 },
                AgeTier { min_age_years: 0.5, min_downloads: 500 },
                AgeTier { min_age_years: 0.083, min_downloads: 300 },
                AgeTier { min_age_years: 0.0, min_downloads: 200 },
            ],
            out_dir: String::new(),
            pages: 10,
            host: DEFAULT_HOST.into(),
            request_interval_secs: 2.5,
            now_offset_days: 0,
        }
    }
}

impl EhRules {
    pub fn from_json(text: &str) -> Result<Self, String> {
        serde_json::from_str(text).map_err(|e| format!("规则 JSON 解析失败：{e}"))
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
        if self.title_markers.trim().is_empty() || self.title_markers.trim().eq_ignore_ascii_case("any")
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
    pub checked: usize,
    pub saved: usize,
    /// 拒绝分项（规则可编辑，必须能看出是哪条规则挡住的）
    pub no_rating: usize,
    pub no_marker: usize,
    pub no_torrent: usize,
    pub no_downloads: usize,
    pub unmapped: usize,
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
    fn new(host: &str, interval: Duration) -> Result<Self, String> {
        let http = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| format!("创建 HTTP 客户端失败：{e}"))?;
        Ok(Self { http, host: host.to_string(), interval })
    }

    fn get(&self, url: &str) -> Result<String, String> {
        let resp = self
            .http
            .get(url)
            .header("User-Agent", UA)
            .send()
            .map_err(|e| format!("请求失败：{e}"))?;
        let status = resp.status().as_u16();
        let body = resp.text().unwrap_or_default();
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
            .map_err(|e| format!("下载失败：{e}"))?;
        let status = resp.status().as_u16();
        if status >= 400 {
            return Err(format!("下载返回 {status}"));
        }
        resp.bytes()
            .map(|b| b.to_vec())
            .map_err(|e| format!("读取响应失败：{e}"))
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
            .map_err(|e| format!("gdata 请求失败：{e}"))?;
        let body = resp.text().unwrap_or_default();
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

fn is_challenge(body: &str) -> bool {
    body.contains("cf-challenge")
        || body.contains("Just a moment")
        || body.contains("<title>Attention Required")
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
pub fn parse_torrent_stats(html: &str) -> Vec<(i64, i64, i64)> {
    let flat = strip_tags(html);
    let mut out = Vec::new();
    let mut rest = flat.as_str();
    while let Some(i) = rest.find("Seeds:") {
        rest = &rest[i..];
        let seg: String = rest.chars().take(160).collect();
        let nums: Vec<i64> = seg
            .split(|c: char| !c.is_ascii_digit())
            .filter(|s| !s.is_empty())
            .filter_map(|s| s.parse().ok())
            .collect();
        if nums.len() >= 3 {
            out.push((nums[0], nums[1], nums[2]));
        }
        rest = &rest[(6).min(rest.len())..];
    }
    out
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

// ---------------------------------------------------------------------------
// 主流程
// ---------------------------------------------------------------------------

/// 连通性预检结果（供 UI 显示；不抛错，让调用方决定是否继续）。
#[derive(Debug, Clone, Serialize)]
pub struct EhProbe {
    pub host: String,
    pub host_ok: bool,
    pub host_detail: String,
    /// 种子直链所在的 tracker 域名（固定 ehtracker.org，与主站分开）
    pub tracker: String,
    pub tracker_ok: bool,
    pub tracker_detail: String,
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
    let client = match Client::new(&host, Duration::from_millis(500)) {
        Ok(c) => c,
        Err(e) => {
            return EhProbe {
                host: host.clone(),
                host_ok: false,
                host_detail: e,
                tracker: "ehtracker.org".into(),
                tracker_ok: false,
                tracker_detail: "未探测".into(),
            }
        }
    };
    let (host_ok, host_detail) = match client.get(&format!("https://{host}/")) {
        Ok(body) if body.len() > 500 => (true, format!("HTTP 200，{} 字节", body.len())),
        Ok(body) => (false, format!("返回内容异常（{} 字节）", body.len())),
        Err(e) => (false, e),
    };
    let (tracker_ok, tracker_detail) = match client.get("https://ehtracker.org/") {
        Ok(_) => (true, "可达".into()),
        Err(e) => (false, e),
    };
    EhProbe {
        host,
        host_ok,
        host_detail,
        tracker: "ehtracker.org".into(),
        tracker_ok,
        tracker_detail,
    }
}

/// 执行一轮订阅扫描：搜索 → 元数据 → 本地筛选 → 查下载数 → 映射确认 → 保存。
///
/// `terminal` 为 true 时把进度打到 stderr（CLI 用）；UI 通过 [`progress`] 轮询。
pub fn collect(rules: &EhRules, terminal: bool) -> Result<EhProgress, String> {
    if RUNNING.swap(true, Ordering::SeqCst) {
        return Err("已有一次扫描正在进行".into());
    }
    let result = collect_inner(rules, terminal);
    RUNNING.store(false, Ordering::SeqCst);
    match &result {
        Ok(p) => set_progress(p.clone()),
        Err(e) => {
            let mut p = progress();
            p.running = false;
            p.stage = "failed".into();
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
    set_progress(snapshot);
}

fn collect_inner(rules: &EhRules, terminal: bool) -> Result<EhProgress, String> {
    if rules.out_dir.trim().is_empty() {
        return Err("请先选择保存目录".into());
    }
    std::fs::create_dir_all(&rules.out_dir).map_err(|e| format!("创建目录失败：{e}"))?;

    let host = if rules.host.trim().is_empty() {
        DEFAULT_HOST.to_string()
    } else {
        rules.host.trim().to_string()
    };
    let client = Client::new(&host, rules.interval())?;
    let mut progress = EhProgress {
        running: true,
        stage: "searching".into(),
        pages: rules.pages.clamp(1, MAX_PAGES),
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
        if page > 1 {
            std::thread::sleep(client.interval);
        }
        let html = client.get(&url)?;
        for p in parse_gallery_pairs(&html, &client.host) {
            if !pairs.iter().any(|(u, _)| u == &p.0) {
                pairs.push(p);
            }
        }
        progress.page = page;
        progress.candidates = pairs.len();
        emit(terminal, "searching", format!("搜索第 {page} 页：累计候选 {} 条", pairs.len()), &progress);
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
    emit(terminal, "metadata", format!("拉取 {} 条元数据", pairs.len()), &progress);
    let mut items: Vec<serde_json::Value> = Vec::new();
    for chunk in pairs.chunks(25) {
        std::thread::sleep(client.interval);
        let reqs: Vec<(String, String)> = chunk
            .iter()
            .filter_map(|(u, _)| parse_gid_token(u))
            .collect();
        items.extend(client.gdata(&reqs)?);
    }

    // 3) 本地筛选（评分/标记/有无种子；EH 搜索做不到这些）
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
        + rules.now_offset_days * 86400;
    let mut manifest = read_manifest(&rules.out_dir);
    let known: HashSet<String> = manifest.iter().map(|m| m.infohash.clone()).collect();
    let mut saved = 0usize;

    for item in items.iter() {
        if !RUNNING.load(Ordering::SeqCst) {
            return Err("已取消".into());
        }
        let gid = item.get("gid").map(|g| g.to_string()).unwrap_or_default();
        let token = text_field(item, "token").to_string();
        let title = text_field(item, "title").to_string();
        let title_jpn = text_field(item, "title_jpn").to_string();
        let rating = rating_of(item);
        if rating < rules.min_rating {
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

        // 4) 下载数（每画廊一次种子页请求）
        progress.stage = "probing".into();
        progress.checked += 1;
        std::thread::sleep(client.interval);
        let stats_html = client.get(&format!(
            "https://{}/gallerytorrents.php?gid={}&t={}",
            client.host, gid, token
        ))?;
        let stats = parse_torrent_stats(&stats_html);
        let posted = posted_of(item);
        let age_years = posted.map(|t| (now - t) as f64 / 365.25 / 86400.0);
        let need = rules.required_dl(age_years.unwrap_or(0.0));
        let torrents = item
            .get("torrents")
            .and_then(|t| t.as_array())
            .cloned()
            .unwrap_or_default();

        for (idx, t) in torrents.iter().enumerate() {
            let hash = text_field(t, "hash").to_string();
            let tname = text_field(t, "name").to_string();
            if hash.is_empty() {
                continue;
            }
            let dl = stats.get(idx).map(|s| s.2).unwrap_or(-1);
            if dl < need {
                progress.no_downloads += 1;
                continue;
            }
            if !maps_to_gallery(&title_jpn, &tname, &[]) {
                progress.unmapped += 1;
                continue;
            }
            if known.contains(&hash) {
                continue;
            }
            std::thread::sleep(client.interval);
            let bytes = client
                .get_bytes(&format!("https://ehtracker.org/get/{gid}/{hash}.torrent"))
                .map_err(|e| format!("下载种子失败：{e}"))?;
            if bytes.first() != Some(&b'd') {
                progress.unmapped += 1;
                continue;
            }
            let file_name = format!("{hash}-{}.torrent", sanitize_filename(&title_jpn));
            let path = Path::new(&rules.out_dir).join(&file_name);
            std::fs::write(&path, &bytes).map_err(|e| format!("写入种子失败：{e}"))?;
            manifest.push(EhSavedItem {
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
                    .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
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
            });
            saved += 1;
            progress.saved = saved;
            emit(
                terminal,
                "filtering",
                format!("已保存 {saved} 个（dl={dl} ≥ {need}）"),
                &progress,
            );
        }
    }

    let json = serde_json::to_string_pretty(&manifest).map_err(|e| format!("清单序列化失败：{e}"))?;
    std::fs::write(manifest_path(&rules.out_dir), json).map_err(|e| format!("写清单失败：{e}"))?;

    progress.running = false;
    progress.stage = "done".into();
    progress.message = format!(
        "本轮新增 {saved} 个；评分不足 {}｜标记排除 {}｜无种子 {}｜下载数不达标 {}｜映射未确认 {}",
        progress.no_rating, progress.no_marker, progress.no_torrent, progress.no_downloads, progress.unmapped
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

    #[test]
    fn required_dl_matches_declared_tiers() {
        let r = EhRules::default();
        assert_eq!(r.required_dl(10.0), 1000, "五年前应要求 1000");
        assert_eq!(r.required_dl(3.0), 700, "3 年应要求 700");
        assert_eq!(r.required_dl(1.0), 500, "1 年应要求 500");
        assert_eq!(r.required_dl(0.2), 300, "2 个月应要求 300");
        assert_eq!(r.required_dl(0.01), 200, "刚发布应要求 200");
    }

    #[test]
    fn marker_rules_allow_and_exclude() {
        let r = EhRules::default();
        assert!(r.marker_ok("x", "[DL版] foo"));
        assert!(r.marker_ok("[Digital] x", ""));
        assert!(!r.marker_ok("x", "[AI Generated] foo"), "排除名单优先");
        assert!(!r.marker_ok("x", "no marker here"));
        let any = EhRules { title_markers: "any".into(), exclude_markers: vec![], ..Default::default() };
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
        assert_eq!(parse_gid_token(&pairs[0].0), Some(("4204783".into(), "6a915060bd".into())));
        assert_eq!(pairs[0].1, "T");
    }

    #[test]
    fn torrent_stats_parse_seeds_peers_downloads() {
        let html = "<div>Seeds: 124 Peers: 4 Downloads: 924</div><div>Seeds: 0 Peers: 7 Downloads: 5</div>";
        assert_eq!(parse_torrent_stats(html), vec![(124, 4, 924), (0, 7, 5)]);
    }

    #[test]
    fn mapping_uses_japanese_title() {
        // 罗马字标题与种子名不匹配，日文原名才匹配（实测结论）
        let jpn = "[赤月屋 (赤月みゅうと)] 僕にしか触れないサキュバス三姉妹に搾られる話4";
        let torrent = "[赤月屋 (赤月みゅうと)] 僕にしか触れないサキュバス三姉妹に搾られる話4〜長女レミィ編(前編)〜.zip";
        assert!(maps_to_gallery(jpn, torrent, &[]));
        assert!(!maps_to_gallery("Akatsuki Myuuto Boku ni shika Furenai", torrent, &[]));
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
    fn int_field_accepts_both_string_and_number() {
        let v = serde_json::json!({"a": "42", "b": 42, "c": "x"});
        assert_eq!(int_field(&v, "a"), Some(42));
        assert_eq!(int_field(&v, "b"), Some(42));
        assert_eq!(int_field(&v, "c"), None);
        assert_eq!(int_field(&v, "missing"), None);
    }
}
