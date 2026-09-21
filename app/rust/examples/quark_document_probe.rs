//! 夸克远端文档探针（**只读诊断**，第 78 轮现场用；第 79 轮加 PDF 模式并更名）
//!
//! 用法（对 DB 副本执行，绝不动真实库、不打印 cookie）：
//! ```text
//! # EPUB / ZIP：归档结构体检 + 改后 vs 改前的打开读数
//! cargo run --example quark_document_probe -- \
//!   --root <DB副本目录> --source quark_xxxxxxxx --fid <资产fid> [--pages 3]
//! # PDF：惰性按需读 vs 整份读入（打开 + 首页渲染）
//! cargo run --example quark_document_probe -- \
//!   --root <DB副本目录> --source quark_xxxxxxxx --fid <资产fid> --pdf [--pages 1]
//! ```
//!
//! 它回答三类问题：
//! 1. **归档结构体检**（只读远端 EOCD + 中央目录）：条目数、压缩方式直方图、加密位、
//!    ZIP64 哨兵、`local_header` 是否可信、CD 与 EOCD 的相对位置（是否夹了前缀数据），
//!    以及"快路径闸门"逐条判定；
//! 2. **EPUB 打开读数 A/B**：`EpubBook::open`（只读中央目录）与 `EpubBook::open_legacy`
//!    （crate 逐条目读 local header）各自几次 Range 读；
//! 3. **PDF 打开/封面成本 A/B**（`--pdf`）：`PdfBook::open`（第 79 轮：pdfium 按需取字节）
//!    与 `PdfBook::open_eager`（历史：整份读入 ⇒ 1 次读、整个文件大小）。

use rust_lib_app::db;
use rust_lib_app::document::epub::EpubBook;
use rust_lib_app::document::Document;
use rust_lib_app::source::quark::{QuarkClient, QuarkFile};
use rust_lib_app::source::ByteSource;
use std::sync::{Arc, Mutex};
use std::time::Instant;

struct Args {
    root: String,
    source: String,
    fid: String,
    pages: u32,
    /// PDF 模式：只做"惰性按需读 vs 整份读入"的 A/B，跳过 ZIP/EPUB 结构体检。
    pdf: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        root: String::new(),
        source: String::new(),
        fid: String::new(),
        pages: 3,
        pdf: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut value = || it.next().ok_or_else(|| format!("{flag} 缺少取值"));
        match flag.as_str() {
            "--root" => args.root = value()?,
            "--source" => args.source = value()?,
            "--fid" => args.fid = value()?,
            "--pdf" => args.pdf = true,
            "--pages" => {
                args.pages = value()?
                    .parse()
                    .map_err(|_| "--pages 必须是正整数".to_string())?
            }
            other => return Err(format!("未知参数 {other}")),
        }
    }
    if args.root.is_empty() || args.source.is_empty() || args.fid.is_empty() {
        return Err("必须提供 --root / --source / --fid".into());
    }
    Ok(args)
}

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

/// 计数字节源：每次 `read_at` = 对远端 1 次 Range 请求。
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

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// 精确读一段（循环补读；EOF 就少返回）。
fn read_range(
    client: &QuarkClient,
    url: &str,
    offset: u64,
    len: usize,
) -> std::io::Result<Vec<u8>> {
    let mut buf = vec![0u8; len];
    let mut got = 0usize;
    while got < len {
        let n = client.read_range_url(url, offset + got as u64, &mut buf[got..])?;
        if n == 0 {
            break;
        }
        got += n;
    }
    buf.truncate(got);
    Ok(buf)
}

/// PDF 模式：在**同一份远端文件**上对比"惰性按需读"（第 79 轮）与"整份读入"（历史行为）
/// 的打开 + 首页渲染成本。
///
/// 远端上用户感知的成本 ≈ `读次数 × RTT + 总字节`：历史行为固定"1 次读、整个文件大小"
/// （现场库里 609 个远端 PDF，这就是封面每枚 17–34 MB 的来源）。
fn run_pdf(client: &Arc<QuarkClient>, fid: &str, name: &str, url: &str, size: u64, pages: u32) {
    println!("\n===== PDF 打开/封面成本 A/B（同一份远端文件）=====");
    // (标签, 每页渲染字节) —— 两个后端必须在同一份文件上渲染出**逐字节相同**的页面，
    // 否则说明惰性读取有短读/错页（`m_GetBlock` 的返回值是成功/失败而不是字节数）。
    let mut rendered: Vec<(&str, Vec<Vec<u8>>)> = Vec::new();
    for (label, eager) in [("改后(惰性按需读)", false), ("改前(整份读入)", true)] {
        let file = Arc::new(QuarkFile::new(
            Arc::clone(client),
            fid.to_string(),
            size,
            url.to_string(),
        ));
        let meter = ReadMeter::default();
        let src = CountingSource::new(Arc::clone(&file), meter.clone());
        let started = Instant::now();
        let opened = if eager {
            rust_lib_app::document::pdf::PdfBook::open_eager(src, name)
        } else {
            rust_lib_app::document::pdf::PdfBook::open(src, name)
        };
        let book = match opened {
            Ok(book) => book,
            Err(error) => {
                println!("  {label}: 打开失败 {error}");
                continue;
            }
        };
        let open_ms = started.elapsed().as_millis();
        let count = book.page_count();
        println!(
            "  {label}: 打开 {open_ms} ms / {} 次读 / {} 字节 / 页数 {count}",
            meter.count(),
            meter.bytes()
        );
        let mut outputs = Vec::new();
        for index in 0..pages.min(count) {
            let (before_reads, before_bytes) = (meter.count(), meter.bytes());
            let started = Instant::now();
            match book.page_bytes(index) {
                Ok(bytes) => {
                    println!(
                        "      page {index}: {} ms / {} 次读 / {} 字节（输出 {} 字节）",
                        started.elapsed().as_millis(),
                        meter.count() - before_reads,
                        meter.bytes() - before_bytes,
                        bytes.len()
                    );
                    outputs.push(bytes);
                }
                Err(error) => println!("      page {index} 渲染失败: {error}"),
            }
        }
        // 封面尺寸（340 宽）渲染同一页：第 79 轮"封面按显示宽度渲染"的直接量化。
        if pages > 0 && count > 0 {
            let (before_reads, before_bytes) = (meter.count(), meter.bytes());
            let started = Instant::now();
            match book.page_bytes_for_display(0, 340) {
                Ok(bytes) => println!(
                    "      cover(340px) page 0: {} ms / {} 次读 / {} 字节（输出 {} 字节）",
                    started.elapsed().as_millis(),
                    meter.count() - before_reads,
                    meter.bytes() - before_bytes,
                    bytes.len()
                ),
                Err(error) => println!("      cover(340px) page 0 渲染失败: {error}"),
            }
        }
        rendered.push((label, outputs));
    }
    if rendered.len() == 2 {
        let (left_label, left) = &rendered[0];
        let (right_label, right) = &rendered[1];
        let same = left.len() == right.len() && left.iter().zip(right).all(|(a, b)| a == b);
        println!(
            "  内容一致性: {}（{left_label} vs {right_label}，{} 页）",
            if same { "✓ 逐字节相同" } else { "✗ 不一致" },
            left.len()
        );
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

    // 数据根必须先于第一次 db::get()：db 路径 = <cache_root>/database.db。
    rust_lib_app::cache::set_custom_cache_root(&args.root);
    println!("数据根(副本): {}", args.root);
    println!("书源        : {}", args.source);
    println!("资产 fid    : {}", args.fid);

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

    let client = Arc::new(QuarkClient::new(&cookie, &root_id).expect("构造夸克客户端失败"));
    let name = client.resolve_name(&args.fid).expect("解析文件名失败");
    let info = client.downlink(&args.fid).expect("取直链失败");
    let url = info.url.clone();
    let (supports, size) = client.probe(&url);
    println!("\n文件        : {name}");
    println!(
        "大小(probe) : {size} B    Range: {}",
        if supports { "支持" } else { "不支持" }
    );
    if !supports || size == 0 {
        eprintln!("Range 不可用 / 大小为 0 ⇒ 现场不会走快路径（快路径要求可随机读）");
        std::process::exit(3);
    }

    if args.pdf {
        run_pdf(&client, &args.fid, &name, &url, size, args.pages);
        return;
    }

    let quark = client.as_ref();
    let sig_at = |off: u64, want: u32| -> bool {
        read_range(quark, &url, off, 4)
            .map(|b| b.len() == 4 && u32_at(&b, 0) == want)
            .unwrap_or(false)
    };

    // ---------- 1) 归档结构体检 ----------
    println!("\n===== 归档结构体检（只读 EOCD + 中央目录）=====");
    let tail_len = (22 + 65_535).min(size as usize);
    let tail_start = size - tail_len as u64;
    let tail = read_range(quark, &url, tail_start, tail_len).expect("读尾部失败");

    // EOCD：枚举尾部**所有**签名候选，再看哪一条的"中央目录"能验证通过。
    // （本层原实现要求"EOCD 正好落在文件末尾"，crate 更宽松 ⇒ 这里要看清真实形状）
    let mut candidates: Vec<(usize, u16)> = Vec::new();
    if tail.len() >= 22 {
        for at in (0..=tail.len() - 22).rev() {
            if u32_at(&tail, at) == 0x0605_4b50 {
                candidates.push((at, u16_at(&tail, at + 20)));
            }
        }
    }
    println!("EOCD 候选       : {} 个", candidates.len());
    for (at, comment) in candidates.iter().take(5) {
        let end = at + 22 + *comment as usize;
        println!(
            "  @尾部 {at}（绝对 {}）注释 {comment} B ⇒ 声明结尾 {end} / 尾部长度 {}（差 {} B）",
            tail_start + *at as u64,
            tail.len(),
            tail.len() as i64 - end as i64
        );
    }
    if candidates.is_empty() {
        println!("⇒ 尾部没有任何 EOCD 签名 ⇒ 快路径必然回退");
        return;
    }
    let hex = |b: &[u8]| {
        b.iter()
            .map(|x| format!("{x:02x}"))
            .collect::<Vec<_>>()
            .join(" ")
    };
    println!(
        "文件末尾 48B    : {}",
        hex(&tail[tail.len().saturating_sub(48)..])
    );

    // 候选筛选：CD 尺寸/偏移合法，且"CD 头签名"能在 EOCD 里写的偏移或
    // `EOCD 绝对位置 - cd_size` 处验证通过。
    let mut chosen: Option<(usize, u64, u64, i64)> = None; // (尾部偏移, eocd_abs, cd_at, delta)
    for (at, comment) in &candidates {
        let eocd_abs = tail_start + *at as u64;
        let cd_size = u32_at(&tail, *at + 12) as u64;
        let cd_offset = u32_at(&tail, *at + 16) as u64;
        let trailing = size.saturating_sub(eocd_abs + 22 + *comment as u64);
        if cd_size == 0 || cd_size == u32::MAX as u64 || cd_offset == u32::MAX as u64 {
            println!("  候选 @{eocd_abs}: ZIP64 哨兵/空 CD ⇒ 跳过（尾部剩余 {trailing} B）");
            continue;
        }
        if cd_size > 8 * 1024 * 1024 || cd_offset.saturating_add(cd_size) > size {
            println!("  候选 @{eocd_abs}: CD 越界或超限(cd_size={cd_size}) ⇒ 跳过（尾部剩余 {trailing} B）");
            continue;
        }
        let derived = eocd_abs.saturating_sub(cd_size);
        let ok_written = sig_at(cd_offset, 0x0201_4b50);
        let ok_derived = sig_at(derived, 0x0201_4b50);
        println!(
            "  候选 @{eocd_abs}: cd_offset={cd_offset} cd_size={cd_size} CD签名@写的={ok_written} @推导={ok_derived} 尾部剩余={trailing} B"
        );
        if ok_written {
            chosen = Some((*at, eocd_abs, cd_offset, 0));
        } else if ok_derived {
            chosen = Some((*at, eocd_abs, derived, cd_offset as i64 - derived as i64));
        }
        if chosen.is_some() {
            break;
        }
    }
    let Some((eocd_at, eocd_abs, cd_at, delta)) = chosen else {
        println!("⇒ 所有候选都无法定位中央目录 ⇒ 快路径必然回退");
        return;
    };
    let cd_size = u32_at(&tail, eocd_at + 12) as u64;
    let cd_offset = u32_at(&tail, eocd_at + 16) as u64;
    let entries = u16_at(&tail, eocd_at + 10) as u64;
    let comment = u16_at(&tail, eocd_at + 20) as usize;
    println!(
        "\n选定 EOCD       : 位置 {eocd_abs}（注释 {comment} B，尾部剩余 {} B）",
        size - (eocd_abs + 22 + comment as u64)
    );
    println!("条目数(EOCD)    : {entries}");
    println!("CD              : offset={cd_offset} size={cd_size} ⇒ 实际读自 {cd_at}（delta={delta}）");
    let cd = read_range(quark, &url, cd_at, cd_size as usize).expect("读中央目录失败");
    let mut at = 0usize;
    let mut n = 0usize;
    let mut method_hist: std::collections::BTreeMap<u16, usize> = Default::default();
    let mut encrypted = 0usize;
    let mut zip64_csize = 0usize;
    let mut bad_offset = 0usize;
    let mut first_local: Option<u64> = None;
    while at + 46 <= cd.len() {
        if u32_at(&cd, at) != 0x0201_4b50 {
            break;
        }
        let flags = u16_at(&cd, at + 8);
        let method = u16_at(&cd, at + 10);
        let csize = u32_at(&cd, at + 20) as u64;
        let name_len = u16_at(&cd, at + 28) as usize;
        let extra_len = u16_at(&cd, at + 30) as usize;
        let comment_len = u16_at(&cd, at + 32) as usize;
        let local_header = u32_at(&cd, at + 42) as u64;
        if at + 46 + name_len > cd.len() {
            break;
        }
        let entry_name = String::from_utf8_lossy(&cd[at + 46..at + 46 + name_len]).into_owned();
        *method_hist.entry(method).or_insert(0) += 1;
        if flags & 0x1 != 0 {
            encrypted += 1;
        }
        if csize == u32::MAX as u64 {
            zip64_csize += 1;
        }
        let local_abs = (local_header as i64 + delta).max(0) as u64;
        if local_abs >= size || !sig_at(local_abs, 0x0403_4b50) {
            bad_offset += 1;
        }
        if first_local.is_none() {
            first_local = Some(local_abs);
        }
        if n < 3 {
            println!(
                "  条目[{n}] method={method} flags=0x{flags:04x} csize={csize} local_abs={local_abs} {entry_name}"
            );
        }
        n += 1;
        at += 46 + name_len + extra_len + comment_len;
    }
    println!("CD 解析         : 条目 {n}（EOCD 说 {entries}），偏移修正 delta={delta}");
    println!("压缩方式直方图  : {method_hist:?}");
    println!(
        "加密条目 {encrypted}    csize=0xFFFFFFFF 条目 {zip64_csize}    local header 不可信 {bad_offset}"
    );
    println!(
        "首个 local header: {first_local:?}（LFH 签名 = {}）",
        first_local
            .map(|off| sig_at(off, 0x0403_4b50))
            .unwrap_or(false)
    );

    println!("\n----- 快路径闸门逐条判定 -----");
    println!("① EOCD 可定位                    : 是");
    println!("② CD 可定位（含偏移修正 delta）  : 是（delta={delta}）");
    println!(
        "③ cd_size<=8MiB 且不越界         : {}",
        if cd_size <= 8 * 1024 * 1024 && cd_offset + cd_size <= size {
            "是"
        } else {
            "**否**"
        }
    );
    println!(
        "④ 全部条目 method∈{{0,8}}         : {}",
        if method_hist.keys().all(|m| *m == 0 || *m == 8) {
            "是"
        } else {
            "**否**"
        }
    );
    println!("⑤ 无加密条目                     : {}", if encrypted == 0 { "是" } else { "**否**" });
    println!("⑥ 无 ZIP64 尺寸哨兵              : {}", if zip64_csize == 0 { "是" } else { "**否**" });
    println!(
        "⑦ local header 全部可信          : {}",
        if bad_offset == 0 { "是" } else { "**否**" }
    );

    // ---------- 2) 改前 / 改后 A/B ----------
    println!("\n===== 打开成本 A/B（同一份远端文件）=====");
    let file = Arc::new(QuarkFile::new(
        Arc::clone(&client),
        args.fid.clone(),
        size,
        url.clone(),
    ));

    let after_meter = ReadMeter::default();
    let started = Instant::now();
    let book = EpubBook::open(
        CountingSource::new(Arc::clone(&file), after_meter.clone()),
        &name,
    )
    .expect("打开失败(改后)");
    let open_ms = started.elapsed().as_millis();
    println!(
        "改后 open         : {open_ms} ms, {} 次读, {} 字节   页数={}",
        after_meter.count(),
        after_meter.bytes(),
        book.page_count()
    );
    for index in 0..args.pages.min(book.page_count()) {
        let before = after_meter.count();
        let started = Instant::now();
        match book.page_bytes(index) {
            Ok(bytes) => println!(
                "  page {index}: {} ms, {} 次读, {} 字节",
                started.elapsed().as_millis(),
                after_meter.count() - before,
                bytes.len()
            ),
            Err(error) => {
                println!("  page {index} 失败: {error}");
                break;
            }
        }
    }

    let before_meter = ReadMeter::default();
    let started = Instant::now();
    let legacy = EpubBook::open_legacy(
        CountingSource::new(Arc::clone(&file), before_meter.clone()),
        &name,
    )
    .expect("打开失败(改前)");
    let legacy_ms = started.elapsed().as_millis();
    println!(
        "改前 open_legacy  : {legacy_ms} ms, {} 次读, {} 字节   页数={}",
        before_meter.count(),
        before_meter.bytes(),
        legacy.page_count()
    );
    for index in 0..args.pages.min(legacy.page_count()) {
        let before = before_meter.count();
        let started = Instant::now();
        match legacy.page_bytes(index) {
            Ok(bytes) => println!(
                "  page {index}: {} ms, {} 次读, {} 字节",
                started.elapsed().as_millis(),
                before_meter.count() - before,
                bytes.len()
            ),
            Err(error) => {
                println!("  page {index} 失败: {error}");
                break;
            }
        }
    }
}
