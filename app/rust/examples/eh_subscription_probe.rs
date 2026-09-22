//! EH 订阅可行性探针（调研用，一次性脚本，不进产品代码路径）。
//!
//! 回答四个问题：
//!   A 连通性：本机匿名能否拿到 gdata（HTTP 状态、Cloudflare/challenge 特征、限流响应头）。
//!   B 搜索语法：`language:chinese$ rating>=4 torrents=1` 能否直接命中候选画廊。
//!   C 字段核验：gdata 里 rating / torrentcount / tags / category 的真实取值形状，
//!     以及"无评分画廊"（rating 0.00 + rating_count 0）在 rating>=4 下是否被正确排除。
//!   D 文件清单映射（关键）：gallerytorrents.php 给出的每个种子的下载数 + 种子内文件数，
//!     与 gallerytorrents.php?gid&t=hash 详情页给出的文件清单做校验——
//!     这是"只推送能确认命中目标画廊的种子"这条产品要求的可行性证据。
//!
//! 运行（在 app/rust 下）：
//!   cargo run --release --example eh_subscription_probe -- basic
//!   cargo run --release --example eh_subscription_probe -- search --pages 3
//!   cargo run --release --example eh_subscription_probe -- torrents --gid 1234567 --token abcdef1234
//!   cargo run --release --example eh_subscription_probe -- scan --gid 1234567 --token abcdef1234 --min-dl 500
//!
//! ==== 2026-09-22 实测修正（原实现已被证伪，以下为真实契约）====
//! * gdata 必须 **POST JSON body**：`{"method":"gdata","gidlist":[[gid,token],...],"namespace":1}`
//!   - GET query 形式（`?method=gdata&gid=..&token=..`）返回 `{"error":"Empty JSON Request"}`；
//!   - body 用 `gid` 而非 `gidlist` 返回 `{"error":"gdata request needs a gidlist"}`；
//!   - 成功响应外层是 `{"gmetadata":[{...}]}`。
//! * gdata 字段类型：`rating` / `torrentcount` / `filecount` 是**字符串**，`filesize` / `posted` 是数字；
//!   **没有** `language` 字段（语言在 `tags` 的 `language:*` 里）；无种子画廊 `torrents` 为 `[]`。
//! * `title` 是罗马字，`title_jpn` 才是日文原名——与种子名做匹配必须用 `title_jpn`（实测罗马字匹配全部失败）。
//! * 画廊种子页 `gallerytorrents.php?gid=<gid>&t=<token>` 对任何画廊都可访问（token 来自 gdata），
//!   页面内含 `Seeds: n Peers: n Downloads: n` 与种子直链 `https://ehtracker.org/get/<gid>/<infohash>.torrent`。
//! * 种子文件为**单文件 zip/rar**（`info.length` + `info.name`），infohash 即 `torrents[].hash`。
//! * 搜索：`/torrents.php?o=cd` 全站按 Downloads 降序（列名 Added/Torrent Name/Gallery/Size/Seeds/Peers/DLs/Uploader）。
//! * **`high resolution` 不是 EH 标签**（`high resolution$` 与已知存在的 `full color$` 一样返回 No hits），
//!   因此"高清"条件无法直接用标签表达，需改用文件大小/来源标记（[DL版] 等）替代。
//!
//! 环境变量：
//!   EH_COOKIE   可选。EH 对匿名请求不给图片配额；gdata 通常匿名可用，
//!               但若返回 403/挑战页，把浏览器 Cookie 整串填进来即可（只读用途）。
//!   EH_HOST     默认 e-hentai.org，可设为 exhentai.org。
//!   EH_INTERVAL 默认 2.5 秒/请求，避免触发限流。

use std::time::Duration;

fn main() {
    run();
}

// ---------------------------------------------------------------------------
// HTTP
// ---------------------------------------------------------------------------

struct Ctx {
    http: reqwest::blocking::Client,
    host: String,
    cookie: Option<String>,
    interval_secs: Option<f64>,
}

impl Ctx {
    fn new() -> Self {
        let http = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("build http client");
        Self {
            http,
            host: std::env::var("EH_HOST").unwrap_or_else(|_| "e-hentai.org".into()),
            cookie: std::env::var("EH_COOKIE").ok().filter(|s| !s.trim().is_empty()),
            interval_secs: None,
        }
    }

    /// 由配置覆盖请求间隔（规则文件里的 request_interval_secs）。
    fn set_interval(&mut self, secs: f64) {
        self.interval_secs = Some(secs);
    }

    fn interval(&self) -> Duration {
        let secs = self
            .interval_secs
            .or_else(|| {
                std::env::var("EH_INTERVAL")
                    .ok()
                    .and_then(|v| v.parse::<f64>().ok())
            })
            .unwrap_or(2.5);
        Duration::from_secs_f64(secs.max(0.5))
    }

    fn ua(&self) -> &'static str {
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) \
         Chrome/124.0.0.0 Safari/537.36"
    }

    fn get(&self, url: &str) -> Result<(u16, String), String> {
        let mut req = self.http.get(url).header("User-Agent", self.ua());
        if let Some(c) = &self.cookie {
            req = req.header("Cookie", c.as_str());
        }
        let resp = req.send().map_err(|e| format!("send {url}: {e}"))?;
        let status = resp.status().as_u16();
        let headers = resp.headers().clone();
        let body = resp.text().unwrap_or_default();
        println!(
            "[http] {} -> {status}  bytes={}  retry-after={:?}  x-ratelimit={:?}",
            url,
            body.len(),
            headers.get("retry-after").map(|v| v.to_str().unwrap_or("?")),
            headers
                .get("x-ratelimit-remaining")
                .map(|v| v.to_str().unwrap_or("?"))
        );
        Ok((status, body))
    }

    /// 实测：搜索/页面都应使用 GET query；POST 到 `/` 只会返回首页（因此默认不用它）。
    #[allow(dead_code)]
    fn post_form(&self, url: &str, form: &[(&str, String)]) -> Result<(u16, String), String> {
        let mut req = self
            .http
            .post(url)
            .header("User-Agent", self.ua())
            .header("Referer", format!("https://{}/", self.host))
            .form(form);
        if let Some(c) = &self.cookie {
            req = req.header("Cookie", c.as_str());
        }
        let resp = req.send().map_err(|e| format!("post {url}: {e}"))?;
        let status = resp.status().as_u16();
        let body = resp.text().unwrap_or_default();
        println!("[http] POST {url} -> {status}  bytes={}", body.len());
        Ok((status, body))
    }

    fn get_bytes(&self, url: &str) -> Result<Vec<u8>, String> {
        let mut req = self.http.get(url).header("User-Agent", self.ua());
        if let Some(c) = &self.cookie {
            req = req.header("Cookie", c.as_str());
        }
        let resp = req.send().map_err(|e| format!("get {url}: {e}"))?;
        let status = resp.status().as_u16();
        let bytes = resp.bytes().map_err(|e| format!("read {url}: {e}"))?;
        println!("[http] {} -> {status}  bytes={}", url, bytes.len());
        Ok(bytes.to_vec())
    }

    fn gallerytorrents_url(&self, gid: &str, token: &str) -> String {
        format!(
            "https://{}/gallerytorrents.php?gid={}&t={}",
            self.host, gid, token
        )
    }
}

/// EH 的 gdata 成功响应外层是 `{"gmetadata":[...]}`（实测）；此函数保留用于诊断非 JSON 响应。
#[allow(dead_code)]
fn json_body(body: &str) -> String {
    let t = body.trim();
    if t.starts_with('{') {
        return t.to_string();
    }
    let start = t.find('(').map(|i| i + 1).unwrap_or(0);
    let end = t.rfind(')').unwrap_or(t.len());
    t[start..end].trim().to_string()
}

fn is_challenge(status: u16, body: &str) -> bool {
    status == 403
        || status == 503
        || body.contains("cf-challenge")
        || body.contains("Just a moment")
        || body.contains("<title>Attention Required")
        || body.contains("This gallery is not available")
}

// ---------------------------------------------------------------------------
// args
// ---------------------------------------------------------------------------

struct Args {
    cmd: String,
    gid: Option<String>,
    token: Option<String>,
    pages: usize,
    min_dl: i64,
    out: Option<String>,
    min_rating: f64,
    markers: String,
    config: Option<String>,
    init_config: Option<String>,
}

fn parse_args() -> Args {
    let mut a = Args {
        cmd: "basic".into(),
        gid: None,
        token: None,
        pages: 3,
        min_dl: 500,
        out: None,
        min_rating: 4.0,
        // 用户口径（2026-09-22）：高清用主标题标记 [Digital]/[DL版] 近似
        markers: "Digital|DL版|DL".into(),
        config: None,
        init_config: None,
    };
    let mut it = std::env::args().skip(1);
    if let Some(first) = it.next() {
        if !first.starts_with("--") {
            a.cmd = first;
        }
    }
    while let Some(flag) = it.next() {
        let val = it.next();
        match flag.as_str() {
            "--gid" => a.gid = val,
            "--token" => a.token = val,
            "--pages" => a.pages = val.and_then(|v| v.parse().ok()).unwrap_or(3),
            "--min-dl" => a.min_dl = val.and_then(|v| v.parse().ok()).unwrap_or(500),
            "--out" => a.out = val,
            "--min-rating" => a.min_rating = val.and_then(|v| v.parse().ok()).unwrap_or(4.0),
            "--markers" => a.markers = val.unwrap_or_else(|| "any".into()),
            "--config" => a.config = val,
            "--init-config" => a.init_config = val,
            _ => {}
        }
    }
    a
}

fn run() {
    let args = parse_args();
    let mut ctx = Ctx::new();
    println!("== EH 订阅探针 ==");
    println!(
        "host={} cookie={} cmd={}",
        ctx.host,
        if ctx.cookie.is_some() { "已提供" } else { "匿名" },
        args.cmd
    );
    let result = match args.cmd.as_str() {
        "basic" => probe_basic(&ctx),
        "search" => probe_search(&ctx, args.pages),
        "torrents" => probe_torrents(&ctx, &args),
        "scan" => probe_scan(&ctx, &args),
        "collect" => probe_collect(&mut ctx, &args),
        "init-config" => write_default_config(
            args.init_config.as_deref().unwrap_or("eh_rules.json"),
            &Rules::default(),
        ),
        other => Err(format!(
            "未知子命令 {other}（basic|search|torrents|scan|collect|init-config）"
        )),
    };
    match result {
        Ok(()) => println!("\n[probe] 完成"),
        Err(e) => println!("\n[probe] 失败：{e}"),
    }
}

// ---------------------------------------------------------------------------
// A basic
// ---------------------------------------------------------------------------

fn probe_basic(ctx: &Ctx) -> Result<(), String> {
    let (status, body) = ctx.get(&format!("https://{}/", ctx.host))?;
    println!("[A1] 首页 status={status} challenge={}", is_challenge(status, &body));

    let (status, body) = ctx.get(&format!(
        "https://{}/api.php?method=gdata&gid=1&token=1",
        ctx.host
    ))?;
    println!("[A2] 非法 gid 的 gdata status={status} challenge={}", is_challenge(status, &body));
    let head: String = body.chars().take(200).collect();
    println!("     body head: {}", head.replace('\n', " "));

    // 用搜索接口取一个真实 gid/token，再验证 gdata 全字段（实测：必须 GET query）。
    let search_url = format!(
        "https://{}/?f_search={}",
        ctx.host,
        urlencode("language:chinese$ uncensored")
    );
    let (status, body) = ctx.get(&search_url)?;
    if is_challenge(status, &body) {
        println!("[A3] 搜索被拦截（status={status}）：匿名可能不够，请提供 EH_COOKIE");
        return Ok(());
    }
    let pairs = parse_gallery_pairs(&body, &ctx.host);
    println!("[A3] 搜索结果解析到 {} 个画廊", pairs.len());
    if let Some((url, title)) = pairs.first() {
        println!("     首个：{title}  {url}");
        match fetch_gdata(ctx, url) {
            Ok(v) => dump_gdata_shape(&v),
            Err(e) => println!("[A4] gdata 失败：{e}"),
        }
    }
    Ok(())
}

fn fetch_gdata(ctx: &Ctx, gallery_url: &str) -> Result<serde_json::Value, String> {
    let (gid, token) = parse_gid_token(gallery_url).ok_or("无法从 URL 解析 gid/token")?;
    let item = fetch_gdata_one(ctx, &gid, &token)?;
    Ok(item)
}

/// 实测契约：POST JSON `{"method":"gdata","gidlist":[[gid,token]],"namespace":1}` → `gmetadata[0]`。
fn fetch_gdata_one(ctx: &Ctx, gid: &str, token: &str) -> Result<serde_json::Value, String> {
    let payload = serde_json::json!({
        "method": "gdata",
        "gidlist": [[gid, token]],
        "namespace": 1,
    })
    .to_string();
    let resp = ctx
        .http
        .post(format!("https://{}/api.php", ctx.host))
        .header("User-Agent", ctx.ua())
        .header("Content-Type", "application/json")
        .body(payload)
        .send()
        .map_err(|e| format!("gdata post: {e}"))?;
    let status = resp.status().as_u16();
    let body = resp.text().unwrap_or_default();
    if is_challenge(status, &body) {
        return Err(format!("status={status} 被拦截"));
    }
    let v: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| format!("gdata 非 JSON: {e} / body={body}"))?;
    if let Some(err) = v.get("error") {
        return Err(format!("gdata 返回错误：{err}"));
    }
    v.get("gmetadata")
        .and_then(|m| m.as_array())
        .and_then(|a| a.first())
        .cloned()
        .ok_or_else(|| format!("gmetadata 为空：{body}"))
}

/// gdata 字段的**真实类型**：rating/torrentcount/filecount/posted 是字符串，filesize 是数字。
fn rating_of(v: &serde_json::Value) -> f64 {
    v.get("rating")
        .and_then(|r| match r {
            serde_json::Value::String(s) => s.parse().ok(),
            serde_json::Value::Number(n) => n.as_f64(),
            _ => None,
        })
        .unwrap_or(-1.0)
}

fn torrentcount_of(v: &serde_json::Value) -> i64 {
    int_field(v, "torrentcount").unwrap_or(-1)
}

fn language_of(v: &serde_json::Value) -> String {
    v.get("tags")
        .and_then(|t| t.as_array())
        .map(|tags| {
            tags.iter()
                .filter_map(|t| t.as_str())
                .find(|t| t.starts_with("language:"))
                .map(|t| t.trim_start_matches("language:").to_string())
                .unwrap_or_else(|| "?".into())
        })
        .unwrap_or_else(|| "?".into())
}

/// 打印 gdata 的真实字段形状（调研报告 B/C 的证据来源）。
fn dump_gdata_shape(v: &serde_json::Value) {
    println!("[A4] gdata item 键：{:?}", v.as_object().map(|o| o.keys().cloned().collect::<Vec<_>>()));
    for key in ["gid", "token", "title", "title_jpn", "category", "rating", "torrentcount", "filecount", "filesize", "posted", "uploader"] {
        if let Some(val) = v.get(key) {
            println!("     {key} = {val}  (type={})", json_type(val));
        }
    }
    println!("     [判定] language 字段存在? {} ；实际语言标签={}", v.get("language").is_some(), language_of(v));
    if let Some(tags) = v.get("tags").and_then(|t| t.as_array()) {
        println!("     tags ({} 条)：", tags.len());
        for t in tags.iter().take(40) {
            println!("       - {t}");
        }
    }
    if let Some(torrents) = v.get("torrents").and_then(|t| t.as_array()) {
        println!("     torrents ({} 条)：", torrents.len());
        for t in torrents.iter().take(10) {
            println!("       - {t}");
        }
    }
}

/// gdata 的数字字段**可能是字符串也可能是数字**（实测：`rating`/`torrentcount`/`filecount`/`posted`
/// 都是字符串，`filesize` 是数字）。统一用本函数解析，避免 `as_i64()` 在字符串上静默返回 None。
fn int_field(v: &serde_json::Value, key: &str) -> Option<i64> {
    match v.get(key)? {
        serde_json::Value::String(s) => s.trim().parse::<i64>().ok(),
        serde_json::Value::Number(n) => n.as_i64(),
        _ => None,
    }
}

/// 发布时间的 unix 秒（gdata 的 `posted` 实测为字符串）。解析失败返回 None，
/// 由调用方决定如何处置——绝不静默当作 0（那会让画龄恒为 0，分档阈值全部失效）。
fn posted_of(v: &serde_json::Value) -> Option<i64> {
    int_field(v, "posted").filter(|t| *t > 0)
}

/// unix 秒 → `YYYY-MM-DD HH:MM UTC`（不引 chrono，手算 civil date）。
fn fmt_utc(secs: i64) -> String {
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

fn json_type(v: &serde_json::Value) -> &'static str {
    match v {
        serde_json::Value::String(_) => "string",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
        serde_json::Value::Bool(_) => "bool",
        serde_json::Value::Null => "null",
    }
}

// ---------------------------------------------------------------------------
// B search
// ---------------------------------------------------------------------------

fn probe_search(ctx: &Ctx, max_pages: usize) -> Result<(), String> {
    let queries = [
        // 实测结论已写入：`rating>=4` / `torrents=1` / `high resolution$` 均返回 No hits（不是有效语法/标签）；
        // 有效的是 `language:<lang>$` 与普通标签词、`reclass:*`。这里保留一个负对照。
        "language:chinese$ uncensored",
        "language:chinese$",
        "language:chinese$ full color$",
        "language:chinese$ high resolution$",
    ];
    for q in queries {
        println!("\n[B] 查询：{q}");
        let mut next: Option<String> = Some(format!("https://{}/", ctx.host));
        let mut page = 0usize;
        let mut total = 0usize;
        let mut no_rating = 0usize;
        let mut non_chinese = 0usize;
        let mut samples: Vec<String> = Vec::new();
        while let Some(url) = next.take() {
            page += 1;
            if page > max_pages {
                break;
            }
            // 实测：搜索必须走 GET query（`/?f_search=...`）；POST 到 `/` 只会返回首页。
            let target = if page == 1 {
                format!("https://{}/?f_search={}", ctx.host, urlencode(q))
            } else {
                url
            };
            let (status, body) = ctx.get(&target)?;
            if is_challenge(status, &body) {
                println!("    status={status} 被拦截，跳过该查询");
                break;
            }
            let pairs = parse_gallery_pairs(&body, &ctx.host);
            total += pairs.len();
            println!("    page {page}: {} 条", pairs.len());
            for (u, title) in pairs.iter().take(3) {
                // 首屏逐条核验 language/rating 是否真的符合查询条件。
                if let Ok(v) = fetch_gdata(ctx, u) {
                    let lang = language_of(&v);
                    let rating = rating_of(&v);
                    let tor = torrentcount_of(&v);
                    if rating < 4.0 {
                        no_rating += 1;
                    }
                    if lang != "chinese" {
                        non_chinese += 1;
                    }
                    samples.push(format!(
                        "      · {title} | lang={lang} rating={rating:.2} torrents={tor}"
                    ));
                }
            }
            next = parse_next_page(&body, &ctx.host);
        }
        for s in &samples {
            println!("{s}");
        }
        println!(
            "    [B 小结] 命中 {total} 条；抽样中 rating<4 的有 {no_rating} 条，lang!=chinese 的有 {non_chinese} 条"
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// C/D torrents
// ---------------------------------------------------------------------------

fn probe_torrents(ctx: &Ctx, args: &Args) -> Result<(), String> {
    let gid = args.gid.clone().ok_or("需要 --gid")?;
    let token = args
        .token
        .clone()
        .ok_or("实测：token 应直接取自 gdata 的 token 字段（如 6a915060bd），不再用搜索反查")?;
    let item = fetch_gdata_one(ctx, &gid, &token)?;
    println!(
        "[D0] gid={} rating={:.2} torrentcount={} title_jpn={}",
        gid,
        rating_of(&item),
        torrentcount_of(&item),
        item.get("title_jpn").and_then(|v| v.as_str()).unwrap_or("?")
    );
    let url = ctx.gallerytorrents_url(&gid, &token);
    println!("\n[D] 种子列表页：{url}");
    let (status, body) = ctx.get(&url)?;
    if is_challenge(status, &body) {
        return Err(format!("种子列表页被拦截 status={status}"));
    }
    let stats = parse_torrent_stats(&body);
    println!("[D1] 页面统计字段 {} 组（下载数在此）", stats.len());
    for s in &stats {
        println!("     Seeds={} Peers={} Downloads={} Size={}", s.0, s.1, s.2, s.3);
    }
    // gdata 里的 torrents[] 是权威列表（含 infohash），页面用于取下载数。
    if let Some(list) = item.get("torrents").and_then(|t| t.as_array()) {
        for (i, t) in list.iter().enumerate() {
            let hash = t.get("hash").and_then(|v| v.as_str()).unwrap_or("?");
            let name = t.get("name").and_then(|v| v.as_str()).unwrap_or("?");
            let dls = stats.get(i).map(|s| s.2).unwrap_or(-1);
            println!("     #{i} infohash={hash} downloads={dls}\n         name={name}");
            // 下载种子文件本身并解析文件清单（"确认命中"的凭据）。
            let torrent_url = format!("https://ehtracker.org/get/{gid}/{hash}.torrent");
            match ctx.get_bytes(&torrent_url) {
                Ok(bytes) => match parse_torrent_files(&bytes) {
                    Some((tname, files)) => {
                        println!(
                            "         torrent name={tname}\n         文件数={} 单/多文件={}",
                            files.len(),
                            if files.is_empty() { "single" } else { "multi" }
                        );
                        for f in files.iter().take(8) {
                            println!("           · {f}");
                        }
                        // 映射判定：用 title_jpn（非罗马字 title）做包含匹配。
                        let jpn = item.get("title_jpn").and_then(|v| v.as_str()).unwrap_or("");
                        println!(
                            "         [映射] title_jpn 归一化包含于种子名/文件名: {}",
                            maps_to_gallery(jpn, &tname, &files)
                        );
                    }
                    None => println!("         torrent 解析失败（非 bencode？）"),
                },
                Err(e) => println!("         torrent 下载失败：{e}"),
            }
        }
    }
    Ok(())
}

/// 实测：Downloads 出现在详情页文本 `Seeds: n Peers: n Downloads: n`，Size 为 `Size: x MiB`。
fn parse_torrent_stats(html: &str) -> Vec<(i64, i64, i64, String)> {
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
        let size = between(&seg, "Size: ", " ").unwrap_or_default();
        if nums.len() >= 3 {
            out.push((nums[0], nums[1], nums[2], size));
        }
        rest = &rest[6.min(rest.len())..];
    }
    out
}

/// scan：**修正后的真实流程**——
///   搜索(GET) → gdata 批量(≤25/次) → 代码内按 rating/tag 过滤 → 取该画廊种子页的下载数 → 过阈值。
/// 注意：`rating>=4` / `torrents=1` 不是有效搜索语法，评分与是否有种子都必须在本地过滤。
fn probe_scan(ctx: &Ctx, args: &Args) -> Result<(), String> {
    println!("\n[C] 扫描 min_dl={} pages={} min_rating=4.0", args.min_dl, args.pages);
    let query = "language:chinese$ uncensored";
    let mut pairs: Vec<(String, String)> = Vec::new();
    let mut next: Option<String> = Some(format!(
        "https://{}/?f_search={}",
        ctx.host,
        urlencode(query)
    ));
    let mut page = 0usize;
    while let Some(u) = next.take() {
        page += 1;
        if page > args.pages {
            break;
        }
        if page > 1 {
            std::thread::sleep(ctx.interval());
        }
        let (st, b) = ctx.get(&u)?;
        if is_challenge(st, &b) {
            println!("     第 {page} 页被拦截，停止翻页");
            break;
        }
        let found = parse_gallery_pairs(&b, &ctx.host);
        println!("     page {page}: {} 条", found.len());
        pairs.extend(found);
        next = parse_next_page(&b, &ctx.host);
    }
    // 去重
    let mut uniq: Vec<(String, String)> = Vec::new();
    for p in pairs {
        if !uniq.iter().any(|(u, _)| u == &p.0) {
            uniq.push(p);
        }
    }
    println!("     候选画廊 {} 条", uniq.len());

    // gdata 批量（实测：一次最多 25 个 gid 的 gidlist）
    let mut meta: Vec<serde_json::Value> = Vec::new();
    for chunk in uniq.chunks(25) {
        let list: Vec<Vec<String>> = chunk
            .iter()
            .filter_map(|(u, _)| {
                parse_gid_token(u).map(|(g, t)| vec![g, t])
            })
            .collect();
        let payload =
            serde_json::json!({"method":"gdata","gidlist":list,"namespace":1}).to_string();
        std::thread::sleep(ctx.interval());
        let resp = ctx
            .http
            .post(format!("https://{}/api.php", ctx.host))
            .header("User-Agent", ctx.ua())
            .header("Content-Type", "application/json")
            .body(payload)
            .send()
            .map_err(|e| format!("gdata 批量失败：{e}"))?;
        let body = resp.text().unwrap_or_default();
        let v: serde_json::Value = serde_json::from_str(&body)
            .map_err(|e| format!("gdata 批量非 JSON：{e} / {body}"))?;
        if let Some(arr) = v.get("gmetadata").and_then(|m| m.as_array()) {
            meta.extend(arr.iter().cloned());
        }
        let _ = std::thread::sleep(ctx.interval());
    }
    println!("     gdata 返回 {} 条", meta.len());

    let (mut pass_rating, mut no_torrent, mut hit) = (0usize, 0usize, 0usize);
    for p in meta.iter() {
        let rating = rating_of(p);
        let gid = p.get("gid").map(|g| g.to_string()).unwrap_or_default();
        let tok = p.get("token").and_then(|t| t.as_str()).unwrap_or("");
        let jpn = p.get("title_jpn").and_then(|t| t.as_str()).unwrap_or("");
        if rating < 4.0 {
            println!("     [rating<4] {rating:.2}  {jpn:.32}");
            continue;
        }
        pass_rating += 1;
        if torrentcount_of(p) <= 0 {
            no_torrent += 1;
            println!("     [no torrent] r={rating:.2}  {jpn:.32}");
            continue;
        }
        // 取下载数
        std::thread::sleep(ctx.interval());
        let (st, b) = ctx.get(&ctx.gallerytorrents_url(&gid, tok))?;
        if is_challenge(st, &b) {
            println!("     [stop] 种子页被拦截");
            break;
        }
        let stats = parse_torrent_stats(&b);
        let mut best = 0i64;
        for (i, t) in p
            .get("torrents")
            .and_then(|x| x.as_array())
            .map(|a| a.iter().collect::<Vec<_>>())
            .unwrap_or_default()
            .iter()
            .enumerate()
        {
            let hash = t.get("hash").and_then(|h| h.as_str()).unwrap_or("");
            let tname = t.get("name").and_then(|h| h.as_str()).unwrap_or("");
            let dl = stats.get(i).map(|s| s.2).unwrap_or(-1);
            best = best.max(dl);
            let mapped = maps_to_gallery(jpn, tname, &[]);
            println!(
                "     [tor] dl={dl:<6} r={rating:.2} map={mapped:<5} magnet=magnet:?xt=urn:btih:{hash}"
            );
        }
        if best >= args.min_dl {
            hit += 1;
            println!("     >>> [PASS] dl={best} >= {}  {jpn:.32}", args.min_dl);
        }
    }
    println!(
        "\n[C 小结] 候选 {} → 评分≥4 {} → 有种子可查 {} → 下载数≥{} 的 {} 条",
        meta.len(),
        pass_rating,
        pass_rating - no_torrent,
        args.min_dl,
        hit
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// 可编辑筛选规则（--config rules.json）
// ---------------------------------------------------------------------------

#[derive(serde::Deserialize, Clone, Default)]
#[serde(default)]
struct AgeTier {
    /// 画龄下限（年）。tier 从大到小匹配，取第一个满足的。
    min_age_years: f64,
    /// 该画龄段要求的下载数下限。
    min_downloads: i64,
}

#[derive(serde::Deserialize, Clone)]
#[serde(default)]
struct Rules {
    /// EH 搜索查询（唯一能被服务端过滤的部分：语言 + 标签）
    search: String,
    /// 评分下限（本地过滤；EH 搜索不支持 rating）
    min_rating: f64,
    /// 主标题标记白名单（"any" = 不筛）。实测 [Digital]/[DL版] 在标题而非标签里。
    title_markers: String,
    /// 主标题标记黑名单（命中即排除），例如 ["AI Generated"]
    exclude_markers: Vec<String>,
    /// 分时间段下载数要求（时间越久要求越高）
    age_tiers: Vec<AgeTier>,
    /// 默认输出目录（可被 --out 覆盖）
    out_dir: String,
    /// 每轮扫描的搜索页数（实测分页可用：跟随 `next=<gid>`；每页 25 条）
    pages: usize,
    /// 每个候选之间/请求之间的间隔秒数
    request_interval_secs: f64,
    /// **仅用于验证分档逻辑**：把"当前时间"往后偏移 N 天再算画龄。
    /// 现场数据都是刚发布的画廊（画龄 <2 天），没有这个开关就无法证明分档阈值会随画龄切换。
    now_offset_days: i64,
}

impl Default for Rules {
    fn default() -> Self {
        Self {
            search: "language:chinese$ uncensored".into(),
            min_rating: 4.0,
            title_markers: "Digital|DL版|DL".into(),
            exclude_markers: vec!["AI Generated".into()],
            // 用户口径：时间越久要求越高；五年前 800。最年轻一档不设门槛（新发种子下载数天然为个位数），
            // 靠"重复扫描 + 已有条目去重"自然实现"累积达标再推"。
            age_tiers: vec![
                AgeTier { min_age_years: 5.0, min_downloads: 800 },
                AgeTier { min_age_years: 2.0, min_downloads: 500 },
                AgeTier { min_age_years: 0.5, min_downloads: 300 },
                AgeTier { min_age_years: 0.083, min_downloads: 100 },
                AgeTier { min_age_years: 0.0, min_downloads: 0 },
            ],
            out_dir: "eh_torrents".into(),
            pages: 2,
            request_interval_secs: 2.5,
            now_offset_days: 0,
        }
    }
}

impl Rules {
    fn load(path: &str) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("读配置 {path} 失败：{e}"))?;
        serde_json::from_str(&text).map_err(|e| format!("配置 {path} 不是合法 JSON：{e}"))
    }

    /// 按画龄选出该段要求的下载数下限。
    fn required_dl(&self, age_years: f64) -> i64 {
        let mut tiers = self.age_tiers.clone();
        tiers.sort_by(|a, b| b.min_age_years.partial_cmp(&a.min_age_years).unwrap_or(std::cmp::Ordering::Equal));
        tiers
            .iter()
            .find(|t| age_years >= t.min_age_years)
            .map(|t| t.min_downloads)
            .unwrap_or_else(|| tiers.last().map(|t| t.min_downloads).unwrap_or(0))
    }

    fn marker_ok(&self, title: &str, title_jpn: &str) -> bool {
        let hay = format!("{title} {title_jpn}");
        let low = hay.to_lowercase();
        if self.exclude_markers.iter().any(|m| {
            let m = m.to_lowercase();
            low.contains(&format!("[{m}]")) || low.contains(&m)
        }) {
            return false;
        }
        if self.title_markers.trim().eq_ignore_ascii_case("any") {
            return true;
        }
        has_marker(title, title_jpn, &self.title_markers)
    }
}

/// 生成一份带注释的默认配置，方便用户直接编辑。
fn write_default_config(path: &str, r: &Rules) -> Result<(), String> {
    let json = serde_json::json!({
        "_说明": "EH 种子订阅筛选规则。改这个文件即可自定义标签与下载次数，无需改代码。",
        "_search_说明": "EH 搜索语法：language:chinese$ ; other:\"full color\"$ ; uncensored。注意 rating 与 torrents 不是搜索维度，只能在本地过滤。",
        "search": r.search,
        "_min_rating_说明": "评分下限（本地过滤）。0 票的画廊 rating 为 0.00，会被自动排除。",
        "min_rating": r.min_rating,
        "_title_markers_说明": "主标题标记白名单（用 | 分隔，或写 any 表示不筛）。实测 [Digital]/[DL版] 出现在主标题里，不在标签里。",
        "title_markers": r.title_markers,
        "_exclude_markers_说明": "命中即排除的标题标记，例如 AI 生成作品。",
        "_pages_说明": "每轮扫描的搜索页数（每页 25 条，实测分页跟随 next=<gid> 可用）。页数越多覆盖越广、耗时与请求数线性增长。",
        "exclude_markers": r.exclude_markers,
        "_age_tiers_说明": "分时间段下载数要求：按 min_age_years 从大到小匹配，取第一个满足的。默认『≥5年:800 / ≥2年:500 / ≥半年:300 / ≥1月:100 / 刚发布:0』。年轻一档不设门槛是有意的：新发种子下载数天然为个位数，靠重复扫描 + 已保存去重自然实现『累积到达标线再保存』。",
        "age_tiers": r.age_tiers.iter().map(|t| serde_json::json!({
            "min_age_years": t.min_age_years,
            "min_downloads": t.min_downloads,
        })).collect::<Vec<_>>(),
        "out_dir": r.out_dir,
        "pages": r.pages,
        "request_interval_secs": r.request_interval_secs,
        "_now_offset_days_说明": "仅用于验证分档逻辑：把当前时间往后偏移 N 天再算画龄。正常使用请保持 0。",
        "now_offset_days": r.now_offset_days,
    });
    std::fs::write(
        path,
        serde_json::to_string_pretty(&json).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("写配置 {path} 失败：{e}"))?;
    println!("[collect] 已写出默认配置 → {path}（可直接编辑后 --config 复用）");
    Ok(())
}

// ---------------------------------------------------------------------------
// collect：筛完直接保存 .torrent 到固定文件夹（115 推送留待后续）
// ---------------------------------------------------------------------------

/// 用户口径（2026-09-22）：
///  · 条件 = 搜索条件（语言/标签）+ 评分下限 + 标题标记（[Digital]/[DL版]）+ 分时间段下载数
///  · 规则来自可编辑 JSON 配置（--config），命令行参数可覆盖
///  · 产出 = <out>/<infohash>-<安全画廊名>.torrent + <out>/manifest.json（按 infohash 去重，可重复运行）
fn probe_collect(ctx: &mut Ctx, args: &Args) -> Result<(), String> {
    // 载入规则：--config 指定则读文件，否则用默认规则并按 out/参数覆盖
    let mut rules = match args.config.as_deref() {
        Some(p) => Rules::load(p)?,
        None => Rules::default(),
    };
    if let Some(r) = args.config.as_deref() {
        println!("[collect] 规则来自 {r}");
    } else {
        println!("[collect] 未指定 --config，使用默认规则（可用 --config 自定义标签与下载次数）");
    }
    if args.out.is_some() {
        rules.out_dir = args.out.clone().unwrap();
    }
    if args.min_rating != 4.0 {
        rules.min_rating = args.min_rating;
    }
    if args.markers != "Digital|DL版|DL" {
        rules.title_markers = args.markers.clone();
    }
    if args.pages != 3 {
        rules.pages = args.pages;
    }
    ctx.set_interval(rules.request_interval_secs);

    let out = rules.out_dir.clone();
    let _ = args;
    std::fs::create_dir_all(&out).map_err(|e| format!("创建目录 {out} 失败：{e}"))?;
    let manifest_path = format!("{}/manifest.json", out.trim_end_matches(['/', '\\']));
    let mut manifest: Vec<serde_json::Value> = std::fs::read_to_string(&manifest_path)
        .ok()
        .and_then(|s| serde_json::from_str::<Vec<serde_json::Value>>(&s).ok())
        .unwrap_or_default();
    println!(
        "\n[collect] out={out}\n          search={:?}\n          min_rating={} markers={} exclude={:?}\n          age_tiers={:?}\n          已有条目={}",
        rules.search, rules.min_rating, rules.title_markers, rules.exclude_markers,
        rules.age_tiers.iter().map(|t| format!("{}y:{}", t.min_age_years, t.min_downloads)).collect::<Vec<_>>(),
        manifest.len()
    );

    // 1) 候选：只能用服务端支持的语法（语言 + 标签）；分页靠跟随 `next=<gid>` 链接
    let mut pairs: Vec<(String, String)> = Vec::new();
    let mut url = format!(
        "https://{}/?f_search={}",
        ctx.host,
        urlencode(&rules.search)
    );
    let mut page = 0usize;
    while page < rules.pages.max(1) {
        page += 1;
        if page > 1 {
            std::thread::sleep(ctx.interval());
        }
        let (st, body) = ctx.get(&url)?;
        if is_challenge(st, &body) {
            println!("[collect] 第 {page} 页被拦截，停止翻页");
            break;
        }
        let found = parse_gallery_pairs(&body, &ctx.host);
        let before = pairs.len();
        for f in found {
            if !pairs.iter().any(|(u, _)| u == &f.0) {
                pairs.push(f);
            }
        }
        println!(
            "[collect] page {page}: 本页新增 {} 条（累计 {}）",
            pairs.len() - before,
            pairs.len()
        );
        match parse_next_page(&body, &ctx.host) {
            Some(n) => url = n,
            None => {
                println!("[collect] 没有更多页");
                break;
            }
        }
    }
    println!("[collect] 候选合计 {} 条（{} 页）", pairs.len(), page);

    // 2) gdata 批量（每次 ≤25 个 gid）
    let mut items: Vec<serde_json::Value> = Vec::new();
    for chunk in pairs.chunks(25) {
        let list: Vec<Vec<String>> = chunk
            .iter()
            .filter_map(|(u, _)| parse_gid_token(u).map(|(g, t)| vec![g, t]))
            .collect();
        let payload =
            serde_json::json!({"method":"gdata","gidlist":list,"namespace":1}).to_string();
        std::thread::sleep(ctx.interval());
        let resp = ctx
            .http
            .post(format!("https://{}/api.php", ctx.host))
            .header("User-Agent", ctx.ua())
            .header("Content-Type", "application/json")
            .body(payload)
            .send()
            .map_err(|e| format!("gdata 批量失败：{e}"))?;
        let text = resp.text().unwrap_or_default();
        let v: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| format!("gdata 非 JSON：{e} / {text}"))?;
        if let Some(arr) = v.get("gmetadata").and_then(|m| m.as_array()) {
            items.extend(arr.iter().cloned());
        }
    }
    println!("[collect] gdata {} 条", items.len());

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
        + rules.now_offset_days * 86400;
    if rules.now_offset_days != 0 {
        println!(
            "[collect] ⚠ 已启用时间偏移 now_offset_days={}（仅用于验证分档，真实场景应保持 0）",
            rules.now_offset_days
        );
    }
    let (mut saved, mut skipped) = (0usize, 0usize);
    // 分项拒绝计数：没有这一步就无法判断"0 命中"是哪条规则造成的（规则可编辑时必须可解释）
    let (mut no_rating, mut no_marker, mut no_torrent, mut no_dl) = (0usize, 0usize, 0usize, 0usize);
    for p in items.iter() {
        let rating = rating_of(p);
        let gid = p.get("gid").map(|g| g.to_string()).unwrap_or_default();
        let token = p.get("token").and_then(|t| t.as_str()).unwrap_or("");
        let title = p.get("title").and_then(|t| t.as_str()).unwrap_or("");
        let title_jpn = p.get("title_jpn").and_then(|t| t.as_str()).unwrap_or("");
        // 注意：posted 是字符串（实测），必须用 posted_of；解析失败时明确标注而不是当成 0
        let posted = posted_of(p);
        let age_years = match posted {
            Some(t) => (now - t) as f64 / 365.25 / 86400.0,
            None => f64::NAN,
        };
        if rating < rules.min_rating {
            no_rating += 1;
            continue;
        }
        if !rules.marker_ok(title, title_jpn) {
            no_marker += 1;
            if no_marker <= 3 {
                println!(
                    "[collect] 被标记规则排除：{}",
                    format!("{title} {title_jpn}").chars().take(60).collect::<String>()
                );
            }
            continue;
        }
        if torrentcount_of(p) <= 0 {
            no_torrent += 1;
            continue;
        }
        // 3) 下载数
        std::thread::sleep(ctx.interval());
        let (st, b) = ctx.get(&ctx.gallerytorrents_url(&gid, token))?;
        if is_challenge(st, &b) {
            println!("[collect] 种子页被拦截，停止");
            break;
        }
        let stats = parse_torrent_stats(&b);
        let list = p.get("torrents").and_then(|t| t.as_array()).cloned().unwrap_or_default();
        for (i, t) in list.iter().enumerate() {
            let hash = t.get("hash").and_then(|h| h.as_str()).unwrap_or("");
            let tname = t.get("name").and_then(|h| h.as_str()).unwrap_or("");
            let dl = stats.get(i).map(|s| s.2).unwrap_or(-1);
            let need = rules.required_dl(age_years);
            if dl < need {
                no_dl += 1;
                if no_dl <= 5 {
                    println!(
                        "[collect] 下载数未达标 dl={dl} < need={need} age={age_years:.2}y  {}",
                        title_jpn.chars().take(42).collect::<String>()
                    );
                }
                continue;
            }
            if !maps_to_gallery(title_jpn, tname, &[]) {
                println!(
                    "[collect] 跳过（映射未确认）dl={dl} need={need} age={age_years:.1}y  {}",
                    title.chars().take(40).collect::<String>()
                );
                skipped += 1;
                continue;
            }
            // 4) 保存 .torrent（按 infohash 去重）
            if manifest
                .iter()
                .any(|m| m.get("infohash").and_then(|v| v.as_str()) == Some(hash))
            {
                println!("[collect] 已存在，跳过 {hash}");
                continue;
            }
            std::thread::sleep(ctx.interval());
            let bytes = match ctx.get_bytes(&format!("https://ehtracker.org/get/{gid}/{hash}.torrent"))
            {
                Ok(b) => b,
                Err(e) => {
                    println!("[collect] 下载失败 {hash}：{e}");
                    continue;
                }
            };
            if bytes.first() != Some(&b'd') {
                println!("[collect] 非 bencode，拒绝保存 {hash}");
                continue;
            }
            let file_name = format!("{}-{}.torrent", hash, sanitize_filename(title_jpn));
            let path = format!("{}/{}", out.trim_end_matches(['/', '\\']), file_name);
            std::fs::write(&path, &bytes).map_err(|e| format!("写 {path} 失败：{e}"))?;
            manifest.push(serde_json::json!({
                "infohash": hash,
                "gid": gid,
                "title": title,
                "title_jpn": title_jpn,
                "rating": rating,
                "torrentcount": torrentcount_of(p),
                "tags": p.get("tags").cloned().unwrap_or(serde_json::Value::Array(vec![])),
                "category": p.get("category").cloned().unwrap_or(serde_json::Value::Null),
                "filecount": int_field(p, "filecount"),
                "filesize": int_field(p, "filesize"),
                "uploader": p.get("uploader").cloned().unwrap_or(serde_json::Value::Null),
                "downloads": dl,
                "posted": posted,
                "posted_utc": posted.map(fmt_utc).unwrap_or_else(|| "未知".into()),
                "age_years": if age_years.is_nan() {
                    serde_json::Value::Null
                } else {
                    serde_json::json!((age_years * 10.0).round() / 10.0)
                },
                "required_dl": need,
                "torrent_name": tname,
                "file": file_name,
                "bytes": bytes.len(),
                "source_url": format!("https://{}/g/{}/{}", ctx.host, gid, token),
                "saved_at": now,
                "saved_at_utc": fmt_utc(now),
            }));
            saved += 1;
            let age_display = if age_years.is_nan() {
                "未知".to_string()
            } else {
                format!("{age_years:.1}y")
            };
            println!(
                "[collect] 已保存 dl={dl} need={need} age={age_display} posted={}  {}",
                posted.map(fmt_utc).unwrap_or_else(|| "未知".into()),
                title_jpn.chars().take(44).collect::<String>()
            );
        }
    }
    let json = serde_json::to_string_pretty(&manifest).map_err(|e| format!("manifest 序列化失败：{e}"))?;
    std::fs::write(&manifest_path, json).map_err(|e| format!("写 manifest 失败：{e}"))?;
    println!(
        "\n[collect] 新保存 {saved} 个 .torrent；映射未确认跳过 {skipped}\n\
         [collect] 拒绝分项：评分不足 {no_rating}；标记规则排除 {no_marker}；无种子 {no_torrent}；下载数不达标 {no_dl}\n\
         [collect] manifest 共 {} 条 → {manifest_path}",
        manifest.len()
    );
    Ok(())
}

fn has_marker(title: &str, title_jpn: &str, markers: &str) -> bool {
    let hay = format!("{title} {title_jpn}").to_lowercase();
    markers
        .split('|')
        .map(|m| m.trim().to_lowercase())
        .filter(|m| !m.is_empty())
        .any(|m| hay.contains(&format!("[{m}]")))
}

fn sanitize_filename(s: &str) -> String {
    // gdata 的 title/title_jpn 里带 HTML 实体（实测如 &#039;），需先解码再落文件名
    let decoded = s
        .replace("&#039;", "'")
        .replace("&quot;", "\"")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&#39;", "'")
        .replace("&apos;", "'");
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
// torrent（bencode）解析：实测种子为单文件 zip，infohash 即 gdata torrents[].hash
// ---------------------------------------------------------------------------

enum BVal {
    Int(i64),
    Bytes(Vec<u8>),
    List(Vec<BVal>),
    Dict(Vec<(Vec<u8>, BVal)>),
}

impl BVal {
    fn get(&self, key: &str) -> Option<&BVal> {
        match self {
            BVal::Dict(d) => d.iter().find(|(k, _)| k == key.as_bytes()).map(|(_, v)| v),
            _ => None,
        }
    }
    fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            BVal::Bytes(b) => Some(b),
            _ => None,
        }
    }
    fn as_usize(&self) -> Option<usize> {
        match self {
            BVal::Int(i) => Some(*i as usize),
            _ => None,
        }
    }
}

fn bencode_parse(b: &[u8], i: &mut usize) -> Option<BVal> {
    match *b.get(*i)? {
        b'i' => {
            let start = *i + 1;
            let end = b[start..].iter().position(|&c| c == b'e')? + start;
            let n: i64 = std::str::from_utf8(&b[start..end]).ok()?.parse().ok()?;
            *i = end + 1;
            Some(BVal::Int(n))
        }
        b'l' => {
            *i += 1;
            let mut out = Vec::new();
            while *b.get(*i)? != b'e' {
                out.push(bencode_parse(b, i)?);
            }
            *i += 1;
            Some(BVal::List(out))
        }
        b'd' => {
            *i += 1;
            let mut out = Vec::new();
            while *b.get(*i)? != b'e' {
                let k = match bencode_parse(b, i)? {
                    BVal::Bytes(k) => k,
                    _ => return None,
                };
                let v = bencode_parse(b, i)?;
                out.push((k, v));
            }
            *i += 1;
            Some(BVal::Dict(out))
        }
        c if c.is_ascii_digit() => {
            let start = *i;
            let colon = b[start..].iter().position(|&c| c == b':')? + start;
            let len: usize = std::str::from_utf8(&b[start..colon]).ok()?.parse().ok()?;
            let from = colon + 1;
            let to = from + len;
            let data = b.get(from..to)?.to_vec();
            *i = to;
            Some(BVal::Bytes(data))
        }
        _ => None,
    }
}

/// 返回 (种子内名称, 文件清单)。文件清单为空 = 单文件种子（实测形态）。
fn parse_torrent_files(bytes: &[u8]) -> Option<(String, Vec<String>)> {
    let mut i = 0usize;
    let meta = bencode_parse(bytes, &mut i)?;
    let info = meta.get("info")?;
    let name = info
        .get("name")
        .and_then(|v| v.as_bytes())
        .map(|b| String::from_utf8_lossy(b).to_string())
        .unwrap_or_default();
    let mut files = Vec::new();
    if let Some(BVal::List(list)) = info.get("files") {
        for f in list {
            if let Some(BVal::List(path)) = f.get("path") {
                let p = path
                    .iter()
                    .filter_map(|c| c.as_bytes())
                    .map(|b| String::from_utf8_lossy(b).to_string())
                    .collect::<Vec<_>>()
                    .join("/");
                let len = f.get("length").and_then(|v| v.as_usize()).unwrap_or(0);
                files.push(format!("{p}  ({len} bytes)"));
            }
        }
    }
    Some((name, files))
}

/// 映射判定：必须用 title_jpn（实测 title 是罗马字，直接匹配全部失败）。
fn maps_to_gallery(title_jpn: &str, torrent_name: &str, files: &[String]) -> bool {
    let needle = normalize_for_match(title_jpn);
    if needle.is_empty() {
        return false;
    }
    let hay = {
        let mut s = normalize_for_match(torrent_name);
        for f in files {
            s.push_str(&normalize_for_match(f));
        }
        s
    };
    // 取标题前 24 字符做包含判定；同时允许"作者+作品名前 12 字"的弱匹配。
    let strong: String = needle.chars().take(24).collect();
    let weak: String = needle.chars().take(12).collect();
    hay.contains(&strong) || (weak.len() >= 8 && hay.contains(&weak))
}

// ---------------------------------------------------------------------------
// HTML 解析（最小实现，够调研用）
// ---------------------------------------------------------------------------

fn parse_gallery_pairs(html: &str, host: &str) -> Vec<(String, String)> {
    let needle = format!("https://{host}/g/");
    let mut out: Vec<(String, String)> = Vec::new();
    let mut rest = html;
    while let Some(i) = rest.find(&needle) {
        rest = &rest[i..];
        let url: String = rest.chars().take_while(|c| *c != '"' && *c != '\'' && !c.is_whitespace()).collect();
        let after = &rest[url.len().min(rest.len())..];
        let title = between(after, ">", "</a>")
            .map(|s| strip_tags(&s))
            .unwrap_or_default();
        if title.len() > 1
            && parse_gid_token(&url).is_some()
            && !out.iter().any(|(u, _)| u == &url)
        {
            out.push((url.clone(), title));
        }
        rest = &rest[url.len().min(rest.len())..];
        if rest.is_empty() {
            break;
        }
    }
    out
}

/// 实测（2026-09-22）：
///  · EH 搜索分页**不是** `?page=N`（实测那样只会重复返回同一页）；真实分页游标在页面脚本变量里：
///    `var nexturl="https://e-hentai.org/?f_search=...&next=4201693";`（`next=<本页最后一个 gid>`）
///  · 页面上**没有** `<a>Next</a>` 锚点，也没有 `id="next"`；只能解析这个 JS 变量。
///  · 同段脚本还有 `maxdate`/`mindate`/`rangeurl`，但实测 `&maxdate=`/`&f_dd=` 等参数对结果**无影响**
///    （结果数恒为 "Found about 12,000 results"），即**服务端不支持按日期窗口过滤**。
///  · 实测单查询总量约 1.1 万条 → 25 条/页约 440 页；`pages` 越大覆盖面越广。
fn parse_next_page(html: &str, host: &str) -> Option<String> {
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
    // 主路径：JS 变量 nexturl
    if let Some(after) = html.split("nexturl=\"").nth(1) {
        if let Some(end) = after.find('"') {
            let url = after[..end].trim();
            if !url.is_empty() {
                return Some(url.to_string());
            }
        }
    }
    // 兜底 1：id="next" 锚点
    if let Some(seg) = between(html, "id=\"next\"", "</a>") {
        if let Some(href) = between(&seg, "href=\"", "\"") {
            return Some(href);
        }
    }
    // 兜底 2：任何链接文字为 Next 的锚点
    let lower = html.to_lowercase();
    let pos = lower.find(">next<")?;
    let start = html[..pos].rfind("href=\"")?;
    let rest = &html[start + 6..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

fn parse_gid_token(url: &str) -> Option<(String, String)> {
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
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&nbsp;", " ")
        .trim()
        .to_string()
}

fn normalize_for_match(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// 最小 URL 编码（避免引额外依赖）：非 unreserved 字符一律百分号编码。
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
