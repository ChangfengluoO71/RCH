//! 阅读页读取画像（**只读测量**，第 68 轮：回答"流式阅读加载为什么慢"）
//!
//! 用法（对 DB 副本执行，绝不动真实库）：
//! ```text
//! RCH_PERF_LOG=<perf.jsonl> cargo run --example read_profile -- \
//!   --root <DB副本目录> --source 115_xxx --path "<资产逻辑路径>" [--pages 5]
//! ```
//! 输出：每页耗时与字节数；配合 `RCH_PERF_LOG` 里的 `source.read_at` / `cdn.range`
//! 事件即可算出"读一页 = 几次远端请求、每次多大、有多少重复区间"。
use rust_lib_app::api::book::book_page;
use rust_lib_app::api::source::{
    cloud115_cookie_connect, cloud115_cookie_disconnect, open_cloud115_cookie_book,
};
use rust_lib_app::db;
use std::time::Instant;

struct Args {
    root: String,
    source: String,
    path: String,
    pages: u32,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        root: String::new(),
        source: String::new(),
        path: String::new(),
        pages: 5,
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut value = || it.next().ok_or_else(|| format!("{flag} 缺少取值"));
        match flag.as_str() {
            "--root" => args.root = value()?,
            "--source" => args.source = value()?,
            "--path" => args.path = value()?,
            "--pages" => {
                args.pages = value()?
                    .parse()
                    .map_err(|_| "--pages 必须是正整数".to_string())?
            }
            other => return Err(format!("未知参数 {other}")),
        }
    }
    if args.root.is_empty() || args.source.is_empty() || args.path.is_empty() {
        return Err("必须提供 --root / --source / --path".into());
    }
    Ok(args)
}

fn main() {
    let args = match parse_args() {
        Ok(args) => args,
        Err(error) => {
            eprintln!("参数错误: {error}");
            std::process::exit(2);
        }
    };
    // 数据根必须先于第一次 db::get()：db 路径 = <cache_root>/database.db。
    rust_lib_app::cache::set_custom_cache_root(&args.root);
    println!("数据根(副本): {}", args.root);
    println!("书源        : {}", args.source);
    println!("资产路径    : {}", args.path);

    let (cookie, root_id): (String, String) = {
        let conn = db::get().lock().expect("db lock");
        conn.query_row(
            "SELECT COALESCE(cookie,''), COALESCE(root_id,'') FROM book_sources WHERE id=?1",
            [&args.source],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("读取书源失败")
    };
    if cookie.is_empty() {
        eprintln!("该书源没有 cookie，无法建立会话");
        std::process::exit(2);
    }

    let session = tokio::runtime::Runtime::new()
        .expect("tokio")
        .block_on(async {
            let info = cloud115_cookie_connect(cookie.clone(), root_id.clone())
                .await
                .expect("连接 115 失败");
            let handle = open_cloud115_cookie_book(
                info.id,
                args.path.clone(),
                "range".to_string(),
            )
            .await
            .expect("打开书籍失败");
            println!("打开成功: handle={}", handle.handle);
            let mut total = 0u128;
            for index in 0..args.pages {
                let started = Instant::now();
                match book_page(handle.handle, index).await {
                    Ok(bytes) => {
                        let elapsed = started.elapsed().as_millis();
                        total += elapsed;
                        println!(
                            "page {index}: {elapsed} ms, {} bytes",
                            bytes.len()
                        );
                    }
                    Err(error) => {
                        println!("page {index}: 失败 {error}");
                        break;
                    }
                }
            }
            println!(
                "合计 {} 页 {} ms（平均 {} ms/页）",
                args.pages,
                total,
                total / args.pages.max(1) as u128
            );
            cloud115_cookie_disconnect(info.id).await;
        });
    let _ = session;
}
