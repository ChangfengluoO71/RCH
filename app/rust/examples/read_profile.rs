//! 阅读读取画像（**只读测量**，第 68 轮；第 78 轮加本地文件 A/B 模式）
//!
//! # 三种用法
//!
//! 1) 本地文件（第 78 轮：EPUB 打开优化 A/B）：
//! ```text
//! cargo run --example read_profile -- --file <x.epub|y.cbz> [--pages 5]            # 改后
//! cargo run --example read_profile -- --file <x.epub> --legacy                    # 改前（历史 crate 路径）
//! ```
//! 2) 没有现成文件时先生成一份合成漫画 EPUB（300 条目 ≈ 真实 EPUB 的形状）：
//! ```text
//! cargo run --example read_profile -- --file %TEMP%\many.epub --gen-epub --entries 300
//! ```
//! 3) 远程 115（第 68 轮原用法，对 DB 副本执行，绝不动真实库）：
//! ```text
//! RCH_PERF_LOG=<perf.jsonl> cargo run --example read_profile -- \
//!   --root <DB副本目录> --source 115_xxx --path "<资产逻辑路径>" [--pages 5]
//! ```
//!
//! # 输出怎么读
//!
//! 每行给出"这次操作产生了几次 `ByteSource::read_at`"。远程源上 **1 次读 = 1 次 CDN Range
//! 往返**（115 CDN 实测 243 ms/次），所以读次数就是打开/翻页成本的直接度量；本地文件模式下
//! 读次数同样有效（耗时是本地磁盘的，不代表远端）。

use rust_lib_app::api::book::book_page;
use rust_lib_app::api::source::{
    cloud115_cookie_connect, cloud115_cookie_disconnect, open_cloud115_cookie_book,
};
use rust_lib_app::db;
use rust_lib_app::document::epub::EpubBook;
use rust_lib_app::document::{open_document, Document};
use rust_lib_app::source::local::LocalFile;
use rust_lib_app::source::ByteSource;
use std::io::Write as _;
use std::sync::{Arc, Mutex};
use std::time::Instant;

struct Args {
    root: String,
    source: String,
    path: String,
    pages: u32,
    /// 本地文件模式（第 78 轮）：直接量一份本地 EPUB/ZIP 的打开与翻页成本。
    file: Option<String>,
    /// A/B 的"改前"：强制走历史 crate 路径（逐条目读 local header）。
    legacy: bool,
    /// 先用合成漫画 EPUB 覆盖 `--file` 指向的路径。
    gen_epub: bool,
    /// 合成 EPUB 的目标条目数（默认 300）。
    entries: usize,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        root: String::new(),
        source: String::new(),
        path: String::new(),
        pages: 5,
        file: None,
        legacy: false,
        gen_epub: false,
        entries: 300,
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut value = || it.next().ok_or_else(|| format!("{flag} 缺少取值"));
        match flag.as_str() {
            "--root" => args.root = value()?,
            "--source" => args.source = value()?,
            "--path" => args.path = value()?,
            "--file" => args.file = Some(value()?),
            "--pages" => {
                args.pages = value()?
                    .parse()
                    .map_err(|_| "--pages 必须是正整数".to_string())?
            }
            "--entries" => {
                args.entries = value()?
                    .parse()
                    .map_err(|_| "--entries 必须是正整数".to_string())?
            }
            "--legacy" => args.legacy = true,
            "--gen-epub" => args.gen_epub = true,
            other => return Err(format!("未知参数 {other}")),
        }
    }
    if args.file.is_none() && (args.root.is_empty() || args.source.is_empty() || args.path.is_empty())
    {
        return Err("必须提供 --file，或 --root / --source / --path".into());
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
    if let Some(file) = args.file.clone() {
        if let Err(error) = run_local_file(&args, &file) {
            eprintln!("本地模式失败: {error}");
            std::process::exit(3);
        }
        return;
    }
    run_cloud(&args);
}

// ============================================================
// 本地文件模式（第 78 轮）：不依赖书源会话，量"打开读次数 / 每页读次数"
// ============================================================

/// 读次数计量表（与 [`CountingSource`] 共享）。
#[derive(Clone, Default)]
struct ReadMeter {
    reads: Arc<Mutex<Vec<(u64, usize)>>>,
}

impl ReadMeter {
    fn count(&self) -> usize {
        self.reads.lock().unwrap().len()
    }

    fn bytes(&self) -> u64 {
        self.reads.lock().unwrap().iter().map(|(_, n)| *n as u64).sum()
    }
}

/// 计数字节源：每次 `read_at` = 对底层（本地文件 / 远端 CDN）1 次请求。
struct CountingSource<S: ByteSource> {
    inner: S,
    meter: ReadMeter,
}

impl<S: ByteSource> CountingSource<S> {
    fn new(inner: S, meter: ReadMeter) -> Self {
        CountingSource { inner, meter }
    }
}

impl<S: ByteSource> ByteSource for CountingSource<S> {
    fn len(&self) -> u64 {
        self.inner.len()
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> std::io::Result<usize> {
        self.meter.reads.lock().unwrap().push((offset, buf.len()));
        self.inner.read_at(offset, buf)
    }
}

fn run_local_file(args: &Args, path: &str) -> Result<(), String> {
    if args.gen_epub {
        generate_epub(std::path::Path::new(path), args.entries)
            .map_err(|error| format!("生成合成 EPUB 失败: {error}"))?;
        println!("已生成合成漫画 EPUB: {path}（目标条目数 {}）", args.entries);
    }
    let file = LocalFile::open(path).map_err(|error| format!("打开本地文件失败: {error}"))?;
    let size = file.len();
    let meter = ReadMeter::default();
    let src = CountingSource::new(file, meter.clone());

    let started = Instant::now();
    let book: Box<dyn Document> = if args.legacy {
        Box::new(
            EpubBook::open_legacy(src, path).map_err(|error| format!("打开失败: {error}"))?,
        )
    } else {
        open_document(src, path).map_err(|error| format!("打开失败: {error}"))?
    };
    let open_ms = started.elapsed().as_millis();

    println!("文件        : {path}（{size} 字节）");
    println!(
        "路径        : {}",
        if args.legacy {
            "改前 —— crate ZipArchive（逐条目读 local header；章节解析用新版按需，历史版本还会在打开期逐章读）"
        } else {
            "改后 —— 只读中央目录 + 条目/章节按需读"
        }
    );
    println!(
        "打开        : {open_ms} ms, {} 次读, {} 字节",
        meter.count(),
        meter.bytes()
    );
    let count = book.page_count();
    println!("页数        : {count}");

    let pages = args.pages.min(count);
    let mut total_ms = 0u128;
    let mut total_reads = 0usize;
    for index in 0..pages {
        let before = meter.count();
        let started = Instant::now();
        match book.page_bytes(index) {
            Ok(bytes) => {
                let elapsed = started.elapsed().as_millis();
                let reads = meter.count() - before;
                total_ms += elapsed;
                total_reads += reads;
                println!(
                    "page {index}: {elapsed} ms, {reads} 次读, {} bytes",
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
        "翻页合计    : {pages} 页 {total_ms} ms, {total_reads} 次读（平均 {} ms/页；远端 1 次读 = 1 次 Range 往返）",
        total_ms / pages.max(1) as u128
    );
    Ok(())
}

/// 生成一本"章节 xhtml + 图片"的漫画 EPUB（条目数 = 2 * 章节数 + 4）。
///
/// 形状与 `document::epub` 单测里的夹具一致：container.xml + OPF + 每章一个 xhtml +
/// 每页一张 jpg + 一个 css。图片约 4 KiB 且不可压缩，保证中央目录不在文件末尾 64 KiB 窗口内
/// 之外的任何特殊性。
fn generate_epub(path: &std::path::Path, entries: usize) -> std::io::Result<()> {
    let chapters = entries.saturating_sub(4) / 2;
    let file = std::fs::File::create(path)?;
    let mut writer = zip::ZipWriter::new(file);
    let stored = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored);
    let deflated = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    writer.start_file("mimetype", stored).map_err(write_err)?;
    writer
        .write_all(b"application/epub+zip")
        .map_err(write_err)?;
    writer
        .start_file("META-INF/container.xml", deflated)
        .map_err(write_err)?;
    writer
        .write_all(
            br#"<?xml version="1.0"?><container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#,
        )
        .map_err(write_err)?;

    let mut manifest = String::new();
    let mut spine = String::new();
    for index in 0..chapters {
        manifest.push_str(&format!(
            r#"<item id="c{index}" href="text/p{index:03}.xhtml" media-type="application/xhtml+xml"/><item id="i{index}" href="images/p{index:03}.jpg" media-type="image/jpeg"/>"#
        ));
        spine.push_str(&format!(r#"<itemref idref="c{index}"/>"#));
    }
    writer
        .start_file("OEBPS/content.opf", deflated)
        .map_err(write_err)?;
    writer
        .write_all(
            format!(
                r#"<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf" version="2.0"><metadata><dc:title xmlns:dc="http://purl.org/dc/elements/1.1/">many</dc:title></metadata><manifest>{manifest}</manifest><spine>{spine}</spine></package>"#
            )
            .as_bytes(),
        )
        .map_err(write_err)?;

    for index in 0..chapters {
        writer
            .start_file(format!("OEBPS/text/p{index:03}.xhtml"), deflated)
            .map_err(write_err)?;
        writer
            .write_all(
                format!(r#"<html><body><img src="../images/p{index:03}.jpg"/></body></html>"#)
                    .as_bytes(),
            )
            .map_err(write_err)?;
    }
    for index in 0..chapters {
        writer
            .start_file(format!("OEBPS/images/p{index:03}.jpg"), deflated)
            .map_err(write_err)?;
        writer.write_all(&page_image(index)).map_err(write_err)?;
    }
    writer
        .start_file("OEBPS/style.css", deflated)
        .map_err(write_err)?;
    writer.write_all(b"body{margin:0}").map_err(write_err)?;
    writer.finish().map_err(write_err)?;
    Ok(())
}

/// 第 `index` 页的图片字节（约 4 KiB、不可压缩，内容可判别）。
fn page_image(index: usize) -> Vec<u8> {
    let mut bytes = vec![0xff, 0xd8, 0xff, 0xe0];
    bytes.extend_from_slice(&(index as u32).to_le_bytes());
    let mut state = 0x2545_f491_4f6c_dd1du64 ^ (index as u64).wrapping_mul(0x9e37_79b9);
    while bytes.len() < 4096 {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        bytes.push((state >> 33) as u8);
    }
    bytes
}

fn write_err(error: impl std::fmt::Display) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::Other, error.to_string())
}

// ============================================================
// 远程 115 模式（第 68 轮原用法）
// ============================================================

fn run_cloud(args: &Args) {
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
