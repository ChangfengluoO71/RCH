//! P0-B2：ZIP/CBZ 远程读取放大的验收测试。
//!
//! # 契约（由 P0-B2 trace 实测 + ZIP 格式语义推出，不是随手拍的阈值）
//!
//! - 打开并索引 N 页，**metadata 网络访问不得保持 `N × 256 KiB`**：
//!   每 entry 的元数据成本上限取 `2 KiB`（单条 local header 实际 30 B + 文件名 + extra，
//!   2 KiB 已是宽松上限），另有固定开销上限 `64 KiB` 覆盖 EOCD / 中央目录。
//! - **请求数不得随 entry count 以 `~2N` 增长**：允许 `N + 16`（实测 40 页时约 `N + 2`）。
//! - **central directory 不得被每个 local-header seek 反复重下**（实测基线 41 次）。
//! - **小 metadata read 不得被放大成 256 KiB Range**（实测基线 40 次 × 256 KiB）。
//! - **正文顺序读仍保留大窗口收益**（读一整页的传输量必须接近页大小，不能退化成一堆小块）。
//!
//! # 基线（P0-B2 第一阶段 trace，40 页 / 12 MiB）
//!
//! ```text
//! reads=81  transferred≈10.05 MiB
//!   local-header/data-start 解析  40 次  10,485,760 B
//!   central-directory 条目解析   39 次      42,978 B
//!   EOCD 尾部扫描                 2 次       4,230 B
//! region_alternations=79 of 80
//! ```

use rust_lib_app::document::{open_document, Document};
use rust_lib_app::source::ByteSource;
use std::io::{self, Write as _};
use std::sync::{Arc, Mutex};

const PAGE_BYTES: usize = 300 * 1024;

/// 每 entry 允许的元数据字节上限（宽松：真实 local header 仅 30 B + 文件名 + extra）。
const PER_ENTRY_METADATA_BYTES: u64 = 2 * 1024;
/// 固定开销上限：EOCD + 中央目录。
const FIXED_METADATA_BYTES: u64 = 64 * 1024;
/// 请求数允许的线性系数与常数项。
const REQUEST_SLACK: usize = 16;

#[derive(Clone, Default)]
struct Meter {
    reads: Arc<Mutex<Vec<(u64, usize)>>>,
    cd_start: u64,
}

impl Meter {
    fn requests(&self) -> usize {
        self.reads.lock().unwrap().len()
    }

    fn bytes(&self) -> u64 {
        self.reads
            .lock()
            .unwrap()
            .iter()
            .map(|(_, n)| *n as u64)
            .sum()
    }

    fn max_fetch(&self) -> usize {
        self.reads
            .lock()
            .unwrap()
            .iter()
            .map(|(_, n)| *n)
            .max()
            .unwrap_or(0)
    }

    fn central_directory_requests(&self) -> usize {
        self.reads
            .lock()
            .unwrap()
            .iter()
            .filter(|(offset, _)| *offset >= self.cd_start)
            .count()
    }

    fn reset(&self) {
        self.reads.lock().unwrap().clear();
    }

    fn snapshot(&self) -> Vec<(u64, usize)> {
        self.reads.lock().unwrap().clone()
    }
}

struct MeteredSource {
    data: Arc<Vec<u8>>,
    meter: Meter,
}

impl ByteSource for MeteredSource {
    fn len(&self) -> u64 {
        self.data.len() as u64
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        self.meter.reads.lock().unwrap().push((offset, buf.len()));
        let start = offset as usize;
        let n = if start >= self.data.len() {
            0
        } else {
            (self.data.len() - start).min(buf.len())
        };
        buf[..n].copy_from_slice(&self.data[start..start + n]);
        Ok(n)
    }
}

/// 构造 N 页 CBZ（Stored，布局可预测）。
fn build_cbz(pages: usize, page_bytes: usize) -> Vec<u8> {
    use zip::write::SimpleFileOptions;
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for page in 0..pages {
        writer
            .start_file(format!("{page:04}.jpg"), options)
            .expect("start entry");
        writer
            .write_all(&vec![page as u8; page_bytes])
            .expect("write entry");
    }
    writer.finish().expect("finish").into_inner()
}

/// 前向扫描 central directory 起始偏移（本装置页面数据为常量字节，不含该签名）。
fn central_directory_start(data: &[u8]) -> u64 {
    let magic = b"PK\x01\x02";
    for index in 0..data.len().saturating_sub(4) {
        if &data[index..index + 4] == magic {
            return index as u64;
        }
    }
    data.len() as u64
}

fn open_metered(pages: usize) -> (Box<dyn Document>, Meter, Arc<Vec<u8>>) {
    let data = Arc::new(build_cbz(pages, PAGE_BYTES));
    let meter = Meter {
        reads: Arc::new(Mutex::new(Vec::new())),
        cd_start: central_directory_start(&data),
    };
    let document = open_document(
        MeteredSource {
            data: Arc::clone(&data),
            meter: meter.clone(),
        },
        "amplification.cbz",
    )
    .expect("open cbz");
    assert_eq!(document.page_count(), pages as u32);
    (document, meter, data)
}

// ---------------------------------------------------------------------------
// Range amplification
// ---------------------------------------------------------------------------

/// 打开 40 页：字节数不得保持 `40 × 256 KiB`；请求数不得保持 `~2N`。
#[test]
fn b2_open_metadata_traffic_is_not_n_times_read_ahead() {
    const PAGES: usize = 40;
    let (_document, meter, data) = open_metered(PAGES);
    let requests = meter.requests();
    let bytes = meter.bytes();
    let max_fetch = meter.max_fetch();
    let budget = PAGES as u64 * PER_ENTRY_METADATA_BYTES + FIXED_METADATA_BYTES;

    println!(
        "P0B2-METRIC open pages={PAGES} archive_bytes={} requests={requests} bytes={bytes} \
         max_fetch={max_fetch} cd_requests={} budget_bytes={budget}",
        data.len(),
        meter.central_directory_requests()
    );

    assert!(
        bytes <= budget,
        "open metadata traffic must not stay at N x 256 KiB: bytes={bytes} budget={budget}"
    );
    assert!(
        requests <= PAGES + REQUEST_SLACK,
        "request count must not grow as ~2N: requests={requests} allowed={}",
        PAGES + REQUEST_SLACK
    );
}

/// 字节数随 entry 数量的增长必须远低于"每 entry 一个 256 KiB"。
#[test]
fn b2_open_traffic_scales_with_entry_count_below_read_ahead() {
    let mut samples = Vec::new();
    for pages in [10_usize, 20, 40] {
        let (_document, meter, _data) = open_metered(pages);
        let bytes = meter.bytes();
        let requests = meter.requests();
        println!("P0B2-METRIC scale pages={pages} requests={requests} bytes={bytes}");
        samples.push((pages, requests, bytes));
        let budget = pages as u64 * PER_ENTRY_METADATA_BYTES + FIXED_METADATA_BYTES;
        assert!(
            bytes <= budget,
            "pages={pages} bytes={bytes} budget={budget}"
        );
        assert!(
            requests <= pages + REQUEST_SLACK,
            "pages={pages} requests={requests} allowed={}",
            pages + REQUEST_SLACK
        );
    }
    // 增长必须是"每 entry 千字节级"，而不是"每 entry 256 KiB"。
    let (small_pages, _, small_bytes) = samples[0];
    let (large_pages, _, large_bytes) = samples[2];
    let per_entry =
        (large_bytes.saturating_sub(small_bytes)) as f64 / (large_pages - small_pages) as f64;
    println!("P0B2-METRIC marginal_bytes_per_entry={per_entry:.1}");
    assert!(
        per_entry < 64.0 * 1024.0,
        "marginal metadata cost per entry must be far below one read-ahead: {per_entry:.1} B"
    );
}

/// central directory 不得被每个 local-header seek 反复重下。
#[test]
fn b2_central_directory_is_not_refetched_per_entry() {
    const PAGES: usize = 40;
    let (_document, meter, _data) = open_metered(PAGES);
    let cd_requests = meter.central_directory_requests();
    println!(
        "P0B2-METRIC cd_requests={cd_requests} of total={}",
        meter.requests()
    );
    assert!(
        cd_requests <= 3,
        "the central directory must be fetched once, not once per entry: {cd_requests} requests"
    );
}

// ---------------------------------------------------------------------------
// Single-page open / content read
// ---------------------------------------------------------------------------

/// 打开第 N 页：不得再解析其它页的 local header；且正文顺序读保留大窗口收益。
#[test]
fn b2_single_page_read_is_local_and_keeps_the_large_window() {
    const PAGES: usize = 40;
    const PAGE_INDEX: u32 = 20;
    let (document, meter, _data) = open_metered(PAGES);

    meter.reset();
    let bytes = document.page_bytes(PAGE_INDEX).expect("read one page");
    assert_eq!(bytes.len(), PAGE_BYTES);

    let requests = meter.requests();
    let transferred = meter.bytes();
    let snapshot = meter.snapshot();
    println!(
        "P0B2-METRIC single_page index={PAGE_INDEX} requests={requests} bytes={transferred} \
         offsets={:?}",
        snapshot.iter().map(|(o, _)| *o).collect::<Vec<_>>()
    );

    // 一页 300 KiB，256 KiB 窗口 → 约 2~3 次请求；这里给 5 的余量。
    assert!(
        requests <= 5,
        "reading one page must not walk other entries: {requests} requests"
    );
    // 传输量必须接近页大小，不能退化成一堆小块。
    assert!(
        transferred <= (PAGE_BYTES as u64) * 2 + 64 * 1024,
        "content read must keep the large sequential window: transferred={transferred}"
    );
    assert!(
        snapshot
            .iter()
            .all(|(offset, _)| *offset >= 20 * PAGE_BYTES as u64),
        "reading one page must not touch other entries' regions: {snapshot:?}"
    );
}

/// 顺序读完整本书：总传输量必须与内容规模同阶（大窗口仍然生效）。
#[test]
fn b2_sequential_book_read_stays_close_to_content_size() {
    const PAGES: usize = 8;
    let (document, meter, _data) = open_metered(PAGES);
    meter.reset();
    for index in 0..PAGES as u32 {
        let bytes = document.page_bytes(index).expect("page");
        assert_eq!(bytes.len(), PAGE_BYTES);
    }
    let transferred = meter.bytes();
    let content = (PAGES * PAGE_BYTES) as u64;
    println!(
        "P0B2-METRIC sequential pages={PAGES} requests={} bytes={transferred} content={content}",
        meter.requests()
    );
    assert!(
        transferred <= content * 2 + 64 * 1024,
        "sequential content read must not blow up: transferred={transferred} content={content}"
    );
}

// ---------------------------------------------------------------------------
// Compatibility（不得为了省请求而信任未经校验的 offset）
// ---------------------------------------------------------------------------

fn open_and_read_all(data: Vec<u8>, name: &str) -> Vec<Vec<u8>> {
    let document = open_document(
        MeteredSource {
            data: Arc::new(data),
            meter: Meter::default(),
        },
        name,
    )
    .expect("open");
    (0..document.page_count())
        .map(|index| document.page_bytes(index).expect("page"))
        .collect()
}

/// ZIP64（large_file 会写入 ZIP64 extra field）+ 非图片 entry + 嵌套路径。
#[test]
fn b2_compat_zip64_non_image_entries_and_nested_paths() {
    use zip::write::SimpleFileOptions;
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let stored = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let zip64 = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .large_file(true);

    writer
        .start_file("notes.txt", stored)
        .expect("non-image entry");
    writer.write_all(b"not an image").expect("write");
    writer
        .start_file("book/001.jpg", zip64)
        .expect("nested zip64 page");
    writer.write_all(&vec![1_u8; 4096]).expect("write");
    writer
        .start_file("book/002.jpg", zip64)
        .expect("nested zip64 page");
    writer.write_all(&vec![2_u8; 4096]).expect("write");
    let data = writer.finish().expect("finish").into_inner();

    let pages = open_and_read_all(data, "compat-zip64.cbz");
    assert_eq!(pages.len(), 2, "only image entries become pages");
    assert_eq!(pages[0], vec![1_u8; 4096]);
    assert_eq!(pages[1], vec![2_u8; 4096]);
}

/// Unicode 文件名 + **extra field** 不得被破坏。
///
/// extra field 的覆盖走 ZIP64（header id `0x0001`，本身就是一种 extra field）：
/// RCH 自己不解释 extra field，是 `zip` crate 解析的，本轮也没有改动这条路径；
/// 因此这里验证的是"带 extra field 的条目仍能被正确枚举与解压"。
#[test]
fn b2_compat_unicode_names_with_extra_fields() {
    use zip::write::SimpleFileOptions;
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .large_file(true); // 写入 ZIP64 extra field
    for name in ["第01话.jpg", "第02话.jpg"] {
        writer.start_file(name, options).expect("start");
        writer.write_all(&vec![7_u8; 8192]).expect("write");
    }
    let data = writer.finish().expect("finish").into_inner();
    let pages = open_and_read_all(data, "compat-unicode.cbz");
    assert_eq!(pages.len(), 2, "unicode names must still be recognised");
    assert_eq!(pages[0].len(), 8192);
    assert_eq!(pages[1].len(), 8192);
}

/// Deflate 内容的 CRC 与解压路径必须仍然正确（不得绕过校验）。
#[test]
fn b2_compat_deflate_content_roundtrips() {
    use zip::write::SimpleFileOptions;
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let payload: Vec<u8> = (0..50_000).map(|i| (i % 251) as u8).collect();
    for name in ["001.jpg", "002.jpg"] {
        writer.start_file(name, options).expect("start");
        writer.write_all(&payload).expect("write");
    }
    let data = writer.finish().expect("finish").into_inner();
    let pages = open_and_read_all(data, "compat-deflate.cbz");
    assert_eq!(pages.len(), 2);
    assert_eq!(pages[0], payload);
    assert_eq!(pages[1], payload);
}

/// 截断的 local header 必须 fail-closed，而不是返回垃圾数据。
#[test]
fn b2_compat_truncated_local_header_fails_closed() {
    let full = build_cbz(4, PAGE_BYTES);
    // 砍掉中部一大段：local header / 内容都变得不可信。
    let truncated = full[..full.len() / 3].to_vec();
    let outcome = std::panic::catch_unwind(|| {
        let document = open_document(
            MeteredSource {
                data: Arc::new(truncated),
                meter: Meter::default(),
            },
            "truncated.cbz",
        );
        match document {
            Ok(document) => (0..document.page_count())
                .map(|index| document.page_bytes(index).is_ok())
                .collect::<Vec<_>>(),
            Err(_) => Vec::new(),
        }
    });
    match outcome {
        Ok(results) => {
            println!("P0B2-METRIC truncated_open_results={results:?}");
            // 允许"打不开"或"某些页读失败"，但绝不允许所有页都"成功"：
            // 若全部成功，说明我们信任了被截断的 offset。
            assert!(
                results.iter().any(|ok| !ok) || results.is_empty(),
                "truncated archive must not read every page successfully: {results:?}"
            );
        }
        Err(_) => panic!("open_document must not panic on a truncated archive"),
    }
}
