//! P0-B2 第一阶段：**只做 trace**，不改变任何生产行为。
//!
//! # 目的
//!
//! 把"打开一个 40 页 CBZ 要 81 次远程 read"逐次分类，回答：
//!
//! 1. 多少次属于 central directory？
//! 2. 多少次属于 local file header？
//! 3. 多少次属于实际 page data？
//! 4. 是否存在"仅为了枚举文件名而打开全部 entry"？
//! 5. 是否存在 local-header ↔ central-directory 来回 seek 导致单窗口持续失效？
//! 6. 256 KiB 放大发生在哪一层？
//!
//! # 为什么 trace 全在测试侧
//!
//! 本文件只实现一个**记录型 `ByteSource`**，挂在生产 `document::open_document` 前面。
//! 分类依据是每次 `read_at` 的**调用栈符号**（`std::backtrace::Backtrace`），因此
//! 不需要在生产代码里加任何标记，也就不会改变行为。
//!
//! 运行：
//! ```text
//! cargo test --test p0b2_zip_read_trace -- --nocapture --test-threads=1
//! ```

use rust_lib_app::document::{open_document, Document};
use rust_lib_app::source::ByteSource;
use std::io::{self, Write as _};
use std::sync::{Arc, Mutex};
use std::time::Instant;

const PAGES: usize = 40;
const PAGE_BYTES: usize = 300 * 1024;

/// 一次 `read_at` 的完整记录。
#[derive(Clone, Debug)]
struct ReadRecord {
    seq: usize,
    offset: u64,
    /// 调用方**请求**的字节数（zip 往往只想要几十字节）。
    requested: usize,
    /// 实际交给底层的一次取数长度（= 放大后的 Range 长度）。
    fetched: usize,
    in_central_directory: bool,
    operation: String,
    /// 上一窗口 (offset, fetched)，用于观察 thrashing。
    previous_window: Option<(u64, u64)>,
    t_us: u64,
}

#[derive(Clone)]
struct TraceState {
    reads: Arc<Mutex<Vec<ReadRecord>>>,
    last_window: Arc<Mutex<Option<(u64, u64)>>>,
    central_directory_start: u64,
    started: Instant,
}

struct TracingSource {
    data: Vec<u8>,
    state: TraceState,
}

impl ByteSource for TracingSource {
    fn len(&self) -> u64 {
        self.data.len() as u64
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        let requested = buf.len();
        let backtrace = std::backtrace::Backtrace::force_capture().to_string();
        let operation = classify(&backtrace, offset, self.state.central_directory_start);
        let previous_window = *self.state.last_window.lock().unwrap();

        let start = offset as usize;
        let n = if start >= self.data.len() {
            0
        } else {
            (self.data.len() - start).min(requested)
        };
        buf[..n].copy_from_slice(&self.data[start..start + n]);

        {
            let mut guard = self.state.reads.lock().unwrap();
            let seq = guard.len();
            guard.push(ReadRecord {
                seq,
                offset,
                requested,
                fetched: requested,
                in_central_directory: offset >= self.state.central_directory_start,
                operation,
                previous_window,
                t_us: self.state.started.elapsed().as_micros() as u64,
            });
        }
        *self.state.last_window.lock().unwrap() = Some((offset, requested as u64));
        Ok(n)
    }
}

/// 按调用栈里最先出现的已知帧归类。
///
/// 调用栈是"内层在前"的，因此**从前往后**扫，第一个命中的就是最内层的相关操作。
fn classify(backtrace: &str, offset: u64, cd_start: u64) -> String {
    const PATTERNS: &[(&str, &str)] = &[
        ("find_data_start", "local-header/data-start 解析"),
        ("central_header_to_zip_file", "central-directory 条目解析"),
        ("read_central_header", "central-directory 读取"),
        ("magic_finder", "EOCD 尾部扫描"),
        ("find_central_directory", "EOCD 定位"),
        ("get_metadata", "ZipArchive 初始化"),
        ("by_index_raw", "entry 原始打开"),
        ("by_index", "entry 内容读取"),
        ("name_for_index", "仅元数据枚举"),
        ("ZipArchive", "ZipArchive 其他"),
    ];
    for (needle, label) in PATTERNS {
        if backtrace.contains(needle) {
            return (*label).to_string();
        }
    }
    if offset >= cd_start {
        "未分类（central directory 区域）".to_string()
    } else {
        "未分类（数据区域）".to_string()
    }
}

/// 构造与生产测试同构的 40 页 CBZ（Stored，布局可预测）。
fn build_cbz() -> Vec<u8> {
    use zip::write::SimpleFileOptions;
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for page in 0..PAGES {
        writer
            .start_file(format!("{page:04}.jpg"), options)
            .expect("start entry");
        writer
            .write_all(&vec![page as u8; PAGE_BYTES])
            .expect("write entry");
    }
    writer.finish().expect("finish").into_inner()
}

/// 前向扫描找到 central directory 起始偏移。
///
/// 本装置构造的页面数据是**常量字节**（`page as u8`，范围 0x00..0x27），不含
/// `PK\x01\x02`，因此第一个签名就是真正的 central directory 起点。
fn find_central_directory_start(data: &[u8]) -> u64 {
    let magic = b"PK\x01\x02";
    for index in 0..data.len().saturating_sub(4) {
        if &data[index..index + 4] == magic {
            return index as u64;
        }
    }
    data.len() as u64
}

fn trace_open() -> (Vec<ReadRecord>, u64, u64) {
    let data = build_cbz();
    let central_directory_start = find_central_directory_start(&data);
    let total_bytes = data.len() as u64;
    let state = TraceState {
        reads: Arc::new(Mutex::new(Vec::new())),
        last_window: Arc::new(Mutex::new(None)),
        central_directory_start,
        started: Instant::now(),
    };

    let document = open_document(
        TracingSource {
            data,
            state: state.clone(),
        },
        "trace.cbz",
    )
    .expect("open cbz");
    assert_eq!(document.page_count(), PAGES as u32);

    let records = state.reads.lock().unwrap().clone();
    (records, total_bytes, central_directory_start)
}

#[test]
fn p0b2_trace_classifies_every_open_read() {
    let (records, total_bytes, cd_start) = trace_open();

    println!(
        "P0B2-TRACE archive_bytes={total_bytes} central_directory_start={cd_start} reads={}",
        records.len()
    );
    for record in &records {
        println!(
            "P0B2-READ seq={} offset={} requested={} fetched={} cd={} op=\"{}\" prev={:?}",
            record.seq,
            record.offset,
            record.requested,
            record.fetched,
            record.in_central_directory,
            record.operation,
            record.previous_window
        );
    }

    let mut by_operation: Vec<(String, usize, u64)> = Vec::new();
    for record in &records {
        match by_operation
            .iter_mut()
            .find(|(name, _, _)| *name == record.operation)
        {
            Some(entry) => {
                entry.1 += 1;
                entry.2 += record.fetched as u64;
            }
            None => by_operation.push((record.operation.clone(), 1, record.fetched as u64)),
        }
    }
    by_operation.sort_by(|a, b| b.1.cmp(&a.1));
    println!("P0B2-SUMMARY operation count bytes");
    for (name, count, bytes) in &by_operation {
        println!("P0B2-SUMMARY   \"{name}\" count={count} bytes={bytes}");
    }

    let amplified: Vec<&ReadRecord> = records
        .iter()
        .filter(|record| record.fetched >= 256 * 1024 && record.requested < 1024)
        .collect();
    let amplified_bytes: u64 = amplified.iter().map(|record| record.fetched as u64).sum();
    println!(
        "P0B2-SUMMARY small_request_big_fetch count={} bytes={} (requested<1KiB, fetched>=256KiB)",
        amplified.len(),
        amplified_bytes
    );

    let mut alternations = 0;
    for pair in records.windows(2) {
        if pair[0].in_central_directory != pair[1].in_central_directory {
            alternations += 1;
        }
    }
    println!(
        "P0B2-SUMMARY region_alternations={alternations} of {} gaps",
        records.len().saturating_sub(1)
    );

    assert!(!records.is_empty(), "trace must capture reads");
}
