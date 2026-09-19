//! ②-b 一次性**有界**回填 triage：把一批 `failed` 封面 job 重新排队，走**生产抓取路径**
//! 重抓一次，从而拿到**具体失败子原因**的分布（只有在 ① 的安全子原因 + ③-1 的具体码之后，
//! 分布才有意义；此前所有失败都塌缩成笼统的 `provider`）。
//!
//! # 安全边界（重要）
//!
//! * **只应在数据库副本上运行**：`--root` 指向的目录里的 `database.db` 必须是副本。
//!   工具不校验这一点（无法可靠区分），但会把它写进报告。真实库通常正被应用锁住。
//! * 只改副本里的两处：`remote_cover_job`（`failed` → `pending`，**保留 attempt**）与
//!   `remote_scan_epoch.session_token`（把副本 epoch 绑到本次临时会话，claim 谓词才认）。
//!   **不**改真实库、**不**改生成/列表/索引数据。
//! * 网络请求全部走生产 worker，自带 `provider_budget` 账号级限速与 Cover 优先级；
//!   `--limit` 决定本次最多重排多少 job。回填"永久失败"是**产品策略**问题（见 ② 结论），
//!   本工具只为**取证**服务。
//!
//! # 用法
//!
//! ```text
//! cargo run --example cover_failure_triage -- \
//!     --root <副本目录> --source <source_id> [--limit 40] [--dry-run] \
//!     [--report out.json] [--timeout-secs 600]
//! ```

use rusqlite::{params, Connection};
use serde_json::{json, Value};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rust_lib_app::api::remote_scan::notify_source_session_ready;
use rust_lib_app::api::source::{cloud115_cookie_connect, quark_connect};
use rust_lib_app::db;

struct Args {
    root: String,
    source: String,
    limit: usize,
    offset: usize,
    asset: String,
    dry_run: bool,
    report: String,
    timeout_secs: u64,
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        root: String::new(),
        source: String::new(),
        limit: 40,
        offset: 0,
        asset: String::new(),
        dry_run: false,
        report: String::new(),
        timeout_secs: 600,
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut value = || it.next().ok_or_else(|| format!("{flag} 缺少取值"));
        match flag.as_str() {
            "--root" => args.root = value()?,
            "--source" => args.source = value()?,
            "--limit" => {
                args.limit = value()?
                    .parse()
                    .map_err(|_| "--limit 必须是正整数".to_string())?
            }
            "--offset" => {
                args.offset = value()?
                    .parse()
                    .map_err(|_| "--offset 必须是非负整数".to_string())?
            }
            "--asset" => args.asset = value()?,
            "--dry-run" => {
                args.dry_run = true;
                continue;
            }
            "--report" => args.report = value()?,
            "--timeout-secs" => {
                args.timeout_secs = value()?
                    .parse()
                    .map_err(|_| "--timeout-secs 必须是正整数".to_string())?
            }
            other => return Err(format!("未知参数: {other}")),
        }
    }
    if args.root.trim().is_empty() || args.source.trim().is_empty() {
        return Err("必须给出 --root 与 --source".into());
    }
    Ok(args)
}

/// 当前分布：`(state, error_code, count)`。
fn distribution(conn: &Connection, source: &str) -> rusqlite::Result<Vec<(String, String, i64)>> {
    conn.prepare(
        "SELECT state, COALESCE(NULLIF(error_code,''),'-'), COUNT(*)
           FROM remote_cover_job WHERE source_id=?1
          GROUP BY 1,2 ORDER BY 3 DESC, 1, 2",
    )?
    .query_map([source], |row| {
        Ok((row.get(0)?, row.get(1)?, row.get(2)?))
    })?
    .collect()
}

/// 逐 job 明细，便于事后按资产核对。
fn detail(
    conn: &Connection,
    source: &str,
) -> rusqlite::Result<Vec<(String, String, String, i64)>> {
    conn.prepare(
        "SELECT asset_id, state, COALESCE(NULLIF(error_code,''),'-'), attempt
           FROM remote_cover_job WHERE source_id=?1
          ORDER BY state, attempt DESC, asset_id",
    )?
    .query_map([source], |row| {
        Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
    })?
    .collect()
}

/// 有界选取：`failed` 里按 job_key 稳定排序取前 `limit` 个（可复现，不随机）。
/// 给了 `asset` 就进入**定向模式**：只针对该资产（便于按体积/格式挑最小样本做 A/B）。
fn failed_keys(
    conn: &Connection,
    source: &str,
    limit: usize,
    offset: usize,
    asset: &str,
) -> rusqlite::Result<Vec<String>> {
    if !asset.trim().is_empty() {
        return conn
            .prepare(
                "SELECT job_key FROM remote_cover_job
                  WHERE source_id=?1 AND state='failed' AND asset_id=?2
                  ORDER BY job_key",
            )?
            .query_map(params![source, asset], |row| row.get(0))?
            .collect();
    }
    conn.prepare(
        "SELECT job_key FROM remote_cover_job
          WHERE source_id=?1 AND state='failed'
          ORDER BY job_key LIMIT ?2 OFFSET ?3",
    )?
    .query_map(params![source, limit as i64, offset as i64], |row| row.get(0))?
    .collect()
}

/// 重排：`failed` → `pending`（保留 attempt / 清空租约与退避），只作用于给定 job_key。
fn requeue(conn: &Connection, keys: &[String], now: i64) -> rusqlite::Result<usize> {
    let mut changed = 0;
    let mut stmt = conn.prepare(
        "UPDATE remote_cover_job
            SET state='pending', next_attempt_at=NULL, lease_owner=NULL, lease_until=NULL,
                updated_at=?1
          WHERE job_key=?2 AND state='failed'",
    )?;
    for key in keys {
        changed += stmt.execute(params![now, key])?;
    }
    Ok(changed)
}

/// 把副本 epoch 绑到本次临时会话，并把待重排 job 的 `session_epoch` **对齐**到该
/// `(source, generation)` 的当前 epoch 行。
///
/// 为什么需要对齐：claim 的联接要求 `epoch.session_epoch = job.session_epoch` 且
/// `epoch.session_token = ?`。真实环境里 app 会**轮换 session_epoch**（实测运行中的应用
/// 把 gen 24 从 `c31b67270b…` 换成 `86760e3d…`），历史 job 因此与 epoch 行不再相等、
/// 谁都领不走。产品侧的补偿路径在推进时会把新 `session_epoch` 写回 job；
/// 本工具手工重排时必须补同一步，否则会误判成"代码没修好"。
/// **仅作用于副本。**
fn bind_session(
    conn: &Connection,
    source: &str,
    keys: &[String],
    session: u64,
) -> rusqlite::Result<(usize, usize)> {
    let epochs = conn.execute(
        "UPDATE remote_scan_epoch SET session_token=?2
          WHERE source_id=?1 AND session_epoch<>''",
        params![source, session as i64],
    )?;
    let mut aligned = 0;
    let mut stmt = conn.prepare(
        "UPDATE remote_cover_job SET
             session_epoch=(SELECT e.session_epoch FROM remote_scan_epoch e
                             WHERE e.source_id=remote_cover_job.source_id
                               AND e.generation=remote_cover_job.generation
                               AND e.session_token=?2 LIMIT 1)
          WHERE job_key=?1
            AND EXISTS(SELECT 1 FROM remote_scan_epoch e
                        WHERE e.source_id=remote_cover_job.source_id
                          AND e.generation=remote_cover_job.generation
                          AND e.session_token=?2)",
    )?;
    for key in keys {
        aligned += stmt.execute(params![key, session as i64])?;
    }
    Ok((epochs, aligned))
}

/// 未完成的到期工作：`running` 或"到期的 pending"。
fn due_work(conn: &Connection, source: &str, now: i64) -> rusqlite::Result<i64> {
    conn.query_row(
        "SELECT COUNT(*) FROM remote_cover_job
          WHERE source_id=?1 AND (state='running'
                OR (state='pending' AND (next_attempt_at IS NULL OR next_attempt_at<=?2)))",
        params![source, now],
        |row| row.get(0),
    )
}

fn print_distribution(title: &str, rows: &[(String, String, i64)]) {
    println!("== {title} ==");
    if rows.is_empty() {
        println!("  (无行)");
    }
    for (state, code, count) in rows {
        println!("  {state:<10} {code:<28} {count}");
    }
}

fn main() {
    let args = match parse_args() {
        Ok(args) => args,
        Err(error) => {
            eprintln!("参数错误: {error}");
            std::process::exit(2);
        }
    };
    // 数据根必须先于第一次 db::get() 设置：db 路径 = <cache_root>/database.db。
    rust_lib_app::cache::set_custom_cache_root(&args.root);
    println!("数据根(副本): {}", args.root);
    println!("书源        : {}", args.source);
    println!("上限/偏移   : {} / {}", args.limit, args.offset);

    let (before, source_type) = {
        let conn = db::get().lock().expect("db lock");
        let (source_type, cookie_len, root_id): (String, i64, String) = conn
            .query_row(
                "SELECT type, LENGTH(COALESCE(cookie,'')), COALESCE(root_id,'')
                   FROM book_sources WHERE id=?1",
                [&args.source],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap_or_else(|error| {
                eprintln!("读取书源失败（副本里没有这个 source？）: {error}");
                std::process::exit(1);
            });
        println!("书源类型    : {source_type}（cookie {} 字节，root_id {}）", cookie_len, root_id);
        if source_type != "quark" && source_type != "115" {
            eprintln!("本工具目前只支持 quark / 115（其它 provider 的会话入口不同）");
            std::process::exit(2);
        }
        let before = distribution(&conn, &args.source).expect("分布查询失败");
        print_distribution("回填前分布", &before);
        (before, source_type)
    };

    let keys = {
        let conn = db::get().lock().expect("db lock");
        failed_keys(&conn, &args.source, args.limit, args.offset, &args.asset)
            .expect("选取失败 job 失败")
    };
    println!("\n选中重排: {} 个 failed job（按 job_key 稳定取前 {}）", keys.len(), args.limit);
    for key in keys.iter().take(5) {
        println!("  · {key}");
    }
    if keys.len() > 5 {
        println!("  · …（其余 {} 个省略）", keys.len() - 5);
    }

    if args.dry_run {
        println!("\n--dry-run：不重排、不联网、不写报告。");
        return;
    }
    if keys.is_empty() {
        println!("\n没有可重排的 failed job，退出。");
        return;
    }

    // 1) 重排（仅副本）
    let requeued = {
        let conn = db::get().lock().expect("db lock");
        requeue(&conn, &keys, now_ms()).expect("重排失败")
    };
    println!("已重排(副本) : {requeued}");

    // 2) 用副本里的凭据开一个临时会话（connect 自带 /config + 首屏 list 校验，
    //    凭据过期会在这里直接失败，而不是产出假的失败分布）
    let (cookie, root_id) = {
        let conn = db::get().lock().expect("db lock");
        conn.query_row(
            "SELECT COALESCE(cookie,''), COALESCE(root_id,'') FROM book_sources WHERE id=?1",
            [&args.source],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .expect("读取凭据失败")
    };
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let session = if source_type == "115" {
        match runtime.block_on(cloud115_cookie_connect(cookie, root_id)) {
            Ok(info) => {
                println!("临时会话     : 115 id={} root={}", info.id, info.root);
                info.id
            }
            Err(error) => {
                eprintln!("115 连接失败（副本里的 cookie 可能已过期，本轮不出分布）: {error}");
                std::process::exit(1);
            }
        }
    } else {
        match runtime.block_on(quark_connect(cookie, root_id)) {
            Ok(info) => {
                println!("临时会话     : quark id={} root={}", info.id, info.root);
                info.id
            }
            Err(error) => {
                eprintln!("夸克连接失败（副本里的 cookie 可能已过期，本轮不出分布）: {error}");
                std::process::exit(1);
            }
        }
    };
    // 3) 把副本 epoch 绑到该会话
    let (epoch_rows, aligned_jobs) = {
        let conn = db::get().lock().expect("db lock");
        bind_session(&conn, &args.source, &keys, session).expect("绑定会话失败")
    };
    println!("epoch 绑定   : {epoch_rows} 行；job session_epoch 对齐 {aligned_jobs} 行");

    // 4) 走生产入口唤醒 worker（rebind → 补偿 → 补齐 → wage worker）
    let reconcile: Value = match runtime.block_on(notify_source_session_ready(
        args.source.clone(),
        session,
    )) {
        Ok(report) => json!({
            "binding_available": report.binding_available,
            "compensation_promoted": report.compensation_promoted,
            "blocker_cleared": report.blocker_cleared,
            "jobs_created": report.jobs_created,
            "truncated": report.truncated,
            "claimable": report.claimable,
        }),
        Err(error) => {
            eprintln!("notify_source_session_ready 失败: {error}");
            std::process::exit(1);
        }
    };
    println!("reconcile    : {reconcile}");

    // 5) 有界等待 worker 抽干（只看副本里的到期工作）
    let started = std::time::Instant::now();
    let deadline = Duration::from_secs(args.timeout_secs);
    loop {
        let due = {
            let conn = db::get().lock().expect("db lock");
            due_work(&conn, &args.source, now_ms()).unwrap_or(-1)
        };
        if due <= 0 {
            break;
        }
        if started.elapsed() > deadline {
            println!("等待超时（{}s），仍剩 {} 个到期工作，按当前结果出报告", args.timeout_secs, due);
            break;
        }
        std::thread::sleep(Duration::from_millis(1000));
    }
    let elapsed = started.elapsed().as_secs();

    // 6) 出报告
    let (after, rows) = {
        let conn = db::get().lock().expect("db lock");
        (
            distribution(&conn, &args.source).expect("分布查询失败"),
            detail(&conn, &args.source).expect("明细查询失败"),
        )
    };
    println!();
    print_distribution("回填后分布", &after);
    println!("\n耗时         : {elapsed}s");

    // perf 计数器快照：⑤ 的字节数证据（`CoverBytesFetched` 含归档/PDF 整包读取）
    let counters = rust_lib_app::perf::snapshot();
    let counter_of = |name: &str| -> i64 {
        counters
            .get("counters")
            .and_then(|c| c.get(name))
            .and_then(|v| v.as_i64())
            .unwrap_or(-1)
    };
    println!("perf 计数    : CoverRangeReads={} CoverDocumentReads={} CoverBytesFetched={} ({:.1} MB) CoverEscalations={} CoverFailures={}",
        counter_of("CoverRangeReads"),
        counter_of("CoverDocumentReads"),
        counter_of("CoverBytesFetched"),
        counter_of("CoverBytesFetched") as f64 / 1048576.0,
        counter_of("CoverEscalations"),
        counter_of("CoverFailures"));

    let report = json!({
        "tool": "cover_failure_triage",
        "note": "②-b 有界一次性回填：failed→pending（保留 attempt）后走生产抓取路径重抓，只作用于数据库副本",
        "root": args.root,
        "source": args.source,
        "limit": args.limit,
        "offset": args.offset,
        "asset": args.asset,
        "requeued": requeued,
        "session_id": session,
        "epoch_rows_bound": epoch_rows,
        "jobs_session_epoch_aligned": aligned_jobs,
        "reconcile": reconcile,
        "elapsed_secs": elapsed,
        "perf_counters": counters.get("counters").cloned().unwrap_or(Value::Null),
        "before": before.iter().map(|(s, c, n)| json!({"state": s, "error_code": c, "count": n})).collect::<Vec<_>>(),
        "after": after.iter().map(|(s, c, n)| json!({"state": s, "error_code": c, "count": n})).collect::<Vec<_>>(),
        "jobs": rows.iter().map(|(asset, state, code, attempt)| json!({
            "asset_id": asset, "state": state, "error_code": code, "attempt": attempt
        })).collect::<Vec<_>>(),
    });
    if !args.report.trim().is_empty() {
        match serde_json::to_string_pretty(&report) {
            Ok(text) => match std::fs::write(&args.report, text) {
                Ok(()) => println!("报告         : {}", args.report),
                Err(error) => eprintln!("写报告失败: {error}"),
            },
            Err(error) => eprintln!("序列化报告失败: {error}"),
        }
    }
}
