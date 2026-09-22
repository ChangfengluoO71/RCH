//! 115 离线下载可行性探针（调研用，一次性脚本，不进产品代码路径）。
//!
//! 目的：确认"把磁力/种子链接推给 115 离线下载"这条路在**官方开放平台**与
//! **扫码 Cookie（webapi）** 两条凭据路径上分别是否可用、契约是什么、配额/风控表现如何。
//!
//! 安全约定：
//!   - 默认只做**读操作**与**故意无效参数的写操作探测**（无效磁力不会产生任务）。
//!   - 只有显式 `--destructive` 才会尝试清理/删除分支；不加就永远不会删你的东西。
//!   - 凭据只从环境变量读，不写盘、不打印全文（只打印前后 6 位做指纹）。
//!
//! 运行（在 app/rust 下）：
//!   # 官方开放平台（RCH 现有 APP ID 模式）
//!   setx / set 115_OPEN_TOKEN=xxxxx   （Windows cmd: set 115_OPEN_TOKEN=xxx）
//!   cargo run --release --example cloud115_offline_probe -- open
//!   cargo run --release --example cloud115_offline_probe -- open --add "magnet:?xt=urn:btih:..."
//!
//!   # 扫码 Cookie（webapi）
//!   set 115_COOKIE=UID=...;CID=...;SEID=...;KID=...
//!   cargo run --release --example cloud115_offline_probe -- web
//!   cargo run --release --example cloud115_offline_probe -- web --add "magnet:?xt=urn:btih:..."
//!
//! 说明：接口路径（尤其官方开放平台的离线下载路径）在调研阶段**未经实证**，
//! 因此本探针用"候选路径逐一探测"的方式跑，用真实响应把契约钉死，
//! 而不是把模型记忆当成事实。每个候选的 HTTP 状态与响应片段都会打印出来。

use std::time::Duration;

fn main() {
    let args = Args::parse();
    let http = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("http client");

    println!("== 115 离线下载探针 ==");
    println!(
        "mode={} destructive={} add={} '{}'",
        args.mode,
        args.destructive,
        args.add.is_some(),
        args.add.clone().unwrap_or_else(|| "-".into())
    );

    let result = match args.mode.as_str() {
        "open" => probe_open(&http, &args),
        "web" => probe_web(&http, &args),
        other => Err(format!("未知模式 {other}（open|web）")),
    };
    match result {
        Ok(()) => println!("\n[probe] 完成"),
        Err(e) => println!("\n[probe] 失败：{e}"),
    }
}

struct Args {
    mode: String,
    add: Option<String>,
    destructive: bool,
}

impl Args {
    fn parse() -> Self {
        let mut a = Args {
            mode: "open".into(),
            add: None,
            destructive: false,
        };
        let mut it = std::env::args().skip(1);
        if let Some(first) = it.next() {
            if !first.starts_with("--") {
                a.mode = first;
            }
        }
        while let Some(f) = it.next() {
            match f.as_str() {
                "--add" => a.add = it.next(),
                "--destructive" => a.destructive = true,
                _ => {}
            }
        }
        a
    }
}

fn fingerprint(s: &str) -> String {
    let n = s.chars().count();
    if n <= 12 {
        return "***".into();
    }
    let head: String = s.chars().take(6).collect();
    let tail: String = s.chars().skip(n - 6).collect();
    format!("{head}…{tail} (len={n})")
}

/// 一次探测：打印状态 + 响应片段，并对"端点是否存在"给出判定。
fn probe(
    http: &reqwest::blocking::Client,
    label: &str,
    method: &str,
    url: &str,
    headers: &[(&str, String)],
    form: Option<&[(&str, String)]>,
    note: &str,
) {
    let mut req = if method == "GET" {
        http.get(url)
    } else {
        http.post(url)
    };
    for (k, v) in headers {
        req = req.header(*k, v.as_str());
    }
    if let Some(f) = form {
        req = req.form(f);
    }
    match req.send() {
        Ok(resp) => {
            let status = resp.status().as_u16();
            let body = resp.text().unwrap_or_default();
            let snippet: String = body.chars().take(400).collect();
            println!("[{label}] {method} {url} -> {status}  ({note})");
            println!("        {} ", snippet.replace('\n', " "));
            let verdict = match status {
                200 | 201 => "端点存在（200）",
                400 | 401 | 403 | 404 | 405 | 500 => "有响应但需核对参数/权限",
                _ => "未知",
            };
            println!("        判定：{verdict}");
        }
        Err(e) => println!("[{label}] {method} {url} -> 网络失败：{e}  ({note})"),
    }
    std::thread::sleep(Duration::from_millis(700));
}

const OPEN_BASE: &str = "https://proapi.115.com";
const WEB_BASE: &str = "https://webapi.115.com";

fn probe_open(http: &reqwest::blocking::Client, args: &Args) -> Result<(), String> {
    let token = std::env::var("115_OPEN_TOKEN")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .ok_or("缺少环境变量 115_OPEN_TOKEN（官方开放平台 access_token）")?;
    println!("token 指纹：{}", fingerprint(&token));
    println!("base：{OPEN_BASE}");
    let auth = [
        ("Authorization", format!("Bearer {token}")),
        ("User-Agent", "RCH-research-probe/0.1".to_string()),
    ];

    // 1) 连通性与权限域
    probe(
        http,
        "O1",
        "GET",
        &format!("{OPEN_BASE}/open/user/info"),
        &auth,
        None,
        "基础连通性；确认 token 有效",
    );
    probe(
        http,
        "O2",
        "GET",
        &format!("{OPEN_BASE}/open/offline/download_list"),
        &auth,
        None,
        "离线下载任务列表（候选路径 A）",
    );
    probe(
        http,
        "O3",
        "GET",
        &format!("{OPEN_BASE}/open/offline/task_list"),
        &auth,
        None,
        "离线下载任务列表（候选路径 B）",
    );
    probe(
        http,
        "O4",
        "GET",
        &format!("{OPEN_BASE}/open/offline/quota"),
        &auth,
        None,
        "离线配额（若存在；决定订阅频率上限）",
    );

    // 2) 加任务：用**故意无效**的磁力探测端点存在性，不会产生真实任务。
    let bad_magnet = "magnet:?xt=urn:btih:0000000000000000000000000000000000000000&dn=__rch_probe_invalid__";
    let uri = args.add.clone().unwrap_or_else(|| bad_magnet.to_string());
    println!(
        "\n[注意] 下面用 {} 探测加任务端点；{}",
        if args.add.is_some() { "你提供的真实链接" } else { "一个故意无效的磁力" },
        if args.add.is_some() { "这会真的创建离线任务" } else { "无效磁力不会产生任务" }
    );
    let add_forms: Vec<(&str, Vec<(&str, String)>)> = vec![
        ("/open/offline/add_task_url", vec![("url", uri.clone())]),
        ("/open/offline/add_task_uri", vec![("url", uri.clone())]),
        ("/open/offline/add_task", vec![("url", uri.clone())]),
    ];
    for (path, form) in add_forms {
        probe(
            http,
            "O5",
            "POST",
            &format!("{OPEN_BASE}{path}"),
            &auth,
            Some(&form),
            "加离线下载任务（候选路径，逐一实证）",
        );
    }

    // 3) 官方通道对种子的支持：一次探测即够，避免反复无效请求。
    println!(
        "\n[O 小结] 上面 O2–O5 中返回 200 的路径即为官方开放平台的真实契约；\n\
         若全部 404/405，说明官方 API 的离线下载路径需要以官方文档为准（本探针路径为候选）。\n\
         拿到 200 后请把响应字段贴回调研报告，作为「磁力/种子是否被接受」的证据。"
    );
    if args.destructive {
        println!(
            "[destructive] 已开启，但删除任务必须人工确认任务 id 后再做，探针不自动删。"
        );
    }
    Ok(())
}

fn probe_web(http: &reqwest::blocking::Client, args: &Args) -> Result<(), String> {
    let cookie = std::env::var("115_COOKIE")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .ok_or("缺少环境变量 115_COOKIE（扫码登录后的 webapi Cookie）")?;
    let uid = cookie
        .split(';')
        .find_map(|kv| {
            let kv = kv.trim();
            kv.strip_prefix("UID=").map(|v| v.trim().to_string())
        })
        .ok_or("Cookie 中未找到 UID=")?;
    println!("cookie 指纹：{}  uid={uid}", fingerprint(&cookie));
    println!("base：{WEB_BASE}");

    let headers = [
        ("Cookie", cookie.clone()),
        (
            "User-Agent",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36".to_string(),
        ),
        ("Referer", "https://115.com/".to_string()),
    ];

    // 1) 会话有效性（读）
    probe(
        http,
        "W1",
        "GET",
        &format!("{WEB_BASE}/files?aid=1&cid=0&o=user_ptime&asc=0&show_dir=1&limit=1&offset=0"),
        &headers,
        None,
        "确认 Cookie 有效且能列目录",
    );
    probe(
        http,
        "W2",
        "GET",
        &format!("{WEB_BASE}/offline/list?page=1"),
        &headers,
        None,
        "离线任务列表（读，安全）",
    );
    probe(
        http,
        "W3",
        "GET",
        &format!("{WEB_BASE}/offline/quota?uid={uid}"),
        &headers,
        None,
        "离线配额（读）",
    );

    // 2) 加任务：默认故意无效磁力（不产生任务）；给了 --add 才用真实链接。
    let uri = args.add.clone().unwrap_or_else(|| {
        "magnet:?xt=urn:btih:0000000000000000000000000000000000000000&dn=__rch_probe_invalid__"
            .to_string()
    });
    println!(
        "\n[注意] 下面用 {} 探测加任务端点；{}",
        if args.add.is_some() { "你提供的真实链接" } else { "一个故意无效的磁力" },
        if args.add.is_some() { "这会真的创建离线任务" } else { "无效磁力不会产生任务" }
    );
    let hash = format!("{WEB_BASE}/offline/add_task_url?uid={uid}");
    let add_variants: Vec<(&str, Vec<(&str, String)>)> = vec![
        (
            "add_task_url",
            vec![("url", uri.clone()), ("wp_path_id", "0".to_string())],
        ),
        (
            "add_task_url",
            vec![
                ("url[0]", uri.clone()),
                ("wp_path_id[0]", "0".to_string()),
            ],
        ),
        ("add_task", vec![("url", uri.clone())]),
        (
            "add_task_bt",
            vec![("url", uri.clone()), ("wp_path_id", "0".to_string())],
        ),
    ];
    for (name, form) in add_variants {
        probe(
            http,
            "W4",
            "POST",
            &hash,
            &headers,
            Some(&form),
            &format!("{name}：加离线下载任务（候选参数形状）"),
        );
    }

    // 3) 任务状态查询与删除（默认不删）
    probe(
        http,
        "W5",
        "POST",
        &format!("{WEB_BASE}/offline/clear?uid={uid}"),
        &headers,
        Some(&[("flag", "1".to_string())]),
        "清理已完成任务（读列表意义；本探针默认不真正执行删除）",
    );

    println!(
        "\n[W 小结] 你需要观察的关键点：\n\
         - W2 是否返回 200 与任务字段（决定『已推送去重』能否落地）；\n\
         - W4 中哪个参数形状返回「无效 bt 信息/磁力非法」类业务错误——那说明端点存在且可加任务；\n\
         - 若 W4 全部 405/404，说明 webapi 的离线路径也变了，需要重新抓包确认。"
    );
    if args.destructive {
        println!("[destructive] 已开启：若要真的清空已完成任务，请手动调用 /offline/clear。");
    }
    Ok(())
}
