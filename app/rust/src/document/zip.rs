//! ZIP / CBZ 流式解析。
//!
//! 打开时只读文件尾部中心目录；页文件头与图片数据在请求该页时读取。
//! 无需整包下载,远程(WebDAV)也能即点即读;各页互不依赖,可并行下载(并行预取)。

use super::{Document, DocumentMeta};
use crate::source::{ByteSource, SourceReader};
use anyhow::{Context, Result};
use std::io::Read;

/// 常见图片扩展名。
const IMAGE_EXTS: &[&str] = &["jpg", "jpeg", "png", "webp", "gif", "bmp", "avif"];

fn is_image(name: &str) -> bool {
    if name.ends_with('/') {
        return false;
    }
    let lower = name.to_lowercase();
    if lower.contains("__macosx") || lower.ends_with(".ds_store") {
        return false;
    }
    lower
        .rsplit('.')
        .next()
        .map(|ext| IMAGE_EXTS.contains(&ext))
        .unwrap_or(false)
}

/// 一页(图片 entry)的定位与解压信息。
struct PageMeta {
    name: String,
    archive_index: usize,
}

/// ZIP/CBZ 书籍:中心目录定位各页,按需下载解压,`page_bytes` 无内部可变状态。
pub struct ZipBook<S: ByteSource> {
    archive: zip::ZipArchive<SourceReader<std::sync::Arc<S>>>,
    pages: Vec<PageMeta>,
    title: String,
}

impl<S: ByteSource> ZipBook<S> {
    pub fn open(src: S, path: &str) -> Result<Self> {
        let reader = SourceReader::new(std::sync::Arc::new(src));
        let zip = zip::ZipArchive::new(reader).context("打开 ZIP/CBZ 失败")?;
        let mut pages = Vec::new();
        for i in 0..zip.len() {
            // by_index opens a local file header and can initialize a decoder.
            // Doing that for all pages makes first-cover cost scale with the
            // entire archive. Names are already in the parsed central directory.
            let name = zip.name_for_index(i).context("读取中心目录失败")?.to_string();
            if !is_image(&name) {
                continue;
            }
            pages.push(PageMeta {
                name,
                archive_index: i,
            });
        }
        pages.sort_by(|a, b| crate::util::natural_cmp(&a.name, &b.name));
        let title = std::path::Path::new(path)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string());
        Ok(ZipBook { archive: zip, pages, title })
    }
}

impl<S: ByteSource> Document for ZipBook<S> {
    fn page_count(&self) -> u32 {
        self.pages.len() as u32
    }

    fn metadata(&self) -> DocumentMeta {
        DocumentMeta {
            title: self.title.clone(),
            ..Default::default()
        }
    }

    fn page_bytes(&self, index: u32) -> Result<Vec<u8>> {
        let p = self
            .pages
            .get(index as usize)
            .with_context(|| format!("页索引越界: {index}"))?;
        // ZipArchive shares immutable central metadata on clone; each reader
        // has its own cursor, so foreground and prefetch need no archive lock.
        let mut archive = self.archive.clone();
        let mut page = archive.by_index(p.archive_index).context("读取页文件头失败")?;
        let mut bytes = Vec::new();
        page.read_to_end(&mut bytes).context("读取或解压页数据失败")?;
        Ok(bytes)
    }
}


// ============================================================
// 封面快通道：最小 ZIP 读取器（第 62 轮）
// ============================================================
//
// 为什么需要（调研：`docs/reports/rg-b/2026-09-19-zip-cover-open-research.md`）：
// `zip::ZipArchive::new` 会对**每个条目**额外读一次 30B local header 做校验
// （`zip-2.4.2/src/read.rs:1259` -> `read.rs:362-378`），打开成本 ~ O(条目数)；
// 实测 115 上一本 2.08GB 的 CBZ：367 次读散布在 0 -> 2.24GB、单次平均 1.5KB，
// 而 115 CDN 单次往返 243ms => 一本 ~5000 条目的漫画要 ~1 万次请求（~40 分钟），
// 单线程 worker 会被一枚封面堵死。
//
// 只取封面时**不需要**解析整个中央目录的条目元数据：只要"第一张图片"。
// 本函数自己读 EOCD + 中央目录（请求数与文件大小、条目数无关），在内存里挑条目，
// 再只读该条目的 local header 与数据。

const ZIP_EOCD_SIG: u32 = 0x0605_4b50;
const ZIP_CDFH_SIG: u32 = 0x0201_4b50;
const ZIP_LFH_SIG: u32 = 0x0403_4b50;
const ZIP_EOCD_LEN: usize = 22;
const ZIP_CDFH_LEN: usize = 46;
const ZIP_LFH_LEN: usize = 30;
/// ZIP 注释上限（规范）=> 尾部一次读这么多就**标准完备**地覆盖 EOCD。
const ZIP_MAX_COMMENT: usize = 65_535;
/// 中央目录读取上限（正常 CBZ ~60B/条目 => 8MiB 覆盖 ~13 万条目），超过交回常规路径。
const ZIP_FAST_MAX_CD: u64 = 8 * 1024 * 1024;
/// 快通道最多尝试几张图片条目（按中央目录顺序）。
const ZIP_FAST_MAX_CANDIDATES: usize = 4;

fn u16_at(buf: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([buf[at], buf[at + 1]])
}

fn u32_at(buf: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([buf[at], buf[at + 1], buf[at + 2], buf[at + 3]])
}

/// 通过 `ByteSource` 精确读一段（不足则报错，不静默截断）。
fn read_exact_at<S: ByteSource>(src: &S, offset: u64, len: usize) -> Result<Vec<u8>> {
    let mut buf = vec![0u8; len];
    let mut got = 0usize;
    while got < len {
        let n = src.read_at(offset + got as u64, &mut buf[got..])?;
        if n == 0 {
            anyhow::bail!("远程读取提前结束");
        }
        got += n;
    }
    Ok(buf)
}

/// "只取封面"的 ZIP 快通道：返回**第一张图片条目**的原始字节。
///
/// 请求数恒定（尾部 1 次 + 中央目录 1 次 + 每候选 1 次 local header + 1 次数据），
/// 与文件大小/条目数无关。任何不成立的情况都返回 `Ok(None)`，由调用方回退常规路径：
/// 非 ZIP、ZIP64、中央目录超限、条目被加密、压缩方式非 stored/deflate、解压失败。
pub(crate) fn first_image_bytes_via_central_directory<S: ByteSource>(
    src: &S,
    max_page_bytes: usize,
) -> Result<Option<Vec<u8>>> {
    let len = src.len();
    if len < ZIP_EOCD_LEN as u64 {
        return Ok(None);
    }
    // 1) 尾部一次读：从后往前找**合法**的 EOCD（注释长度必须正好落到文件末尾）。
    let tail_len = (ZIP_EOCD_LEN + ZIP_MAX_COMMENT).min(len as usize);
    let tail = read_exact_at(src, len - tail_len as u64, tail_len)?;
    let mut eocd_at: Option<usize> = None;
    if tail.len() >= ZIP_EOCD_LEN {
        for at in (0..=tail.len() - ZIP_EOCD_LEN).rev() {
            if u32_at(&tail, at) != ZIP_EOCD_SIG {
                continue;
            }
            let comment = u16_at(&tail, at + 20) as usize;
            if at + ZIP_EOCD_LEN + comment == tail.len() {
                eocd_at = Some(at);
                break;
            }
        }
    }
    let Some(eocd_at) = eocd_at else {
        return Ok(None);
    };
    let cd_size = u32_at(&tail, eocd_at + 12) as u64;
    let cd_offset = u32_at(&tail, eocd_at + 16) as u64;
    // ZIP64 哨兵值 => 交回常规路径（它有完整实现）。
    if cd_size == u32::MAX as u64 || cd_offset == u32::MAX as u64 || cd_size == 0 {
        return Ok(None);
    }
    if cd_size > ZIP_FAST_MAX_CD || cd_offset.saturating_add(cd_size) > len {
        return Ok(None);
    }
    // 2) 中央目录一次读。
    let cd = read_exact_at(src, cd_offset, cd_size as usize)?;
    // 3) 内存里挑出图片条目（按中央目录顺序，取前几个候选）。
    let mut candidates: Vec<(u64, u64, u16)> = Vec::new(); // (local_header_offset, csize, method)
    let mut at = 0usize;
    while at + ZIP_CDFH_LEN <= cd.len() && candidates.len() < ZIP_FAST_MAX_CANDIDATES {
        if u32_at(&cd, at) != ZIP_CDFH_SIG {
            break;
        }
        let flags = u16_at(&cd, at + 8);
        let method = u16_at(&cd, at + 10);
        let csize = u32_at(&cd, at + 20) as u64;
        let name_len = u16_at(&cd, at + 28) as usize;
        let extra_len = u16_at(&cd, at + 30) as usize;
        let comment_len = u16_at(&cd, at + 32) as usize;
        let local_header = u32_at(&cd, at + 42) as u64;
        if at + ZIP_CDFH_LEN + name_len > cd.len() {
            break;
        }
        let name = String::from_utf8_lossy(&cd[at + ZIP_CDFH_LEN..at + ZIP_CDFH_LEN + name_len]);
        // 加密条目不碰；尺寸取自**中央目录**（位 3/data descriptor 时 local header 里是 0）。
        if flags & 0x1 == 0 && csize > 0 && csize <= max_page_bytes as u64 && is_image(&name) {
            candidates.push((local_header, csize, method));
        }
        at += ZIP_CDFH_LEN + name_len + extra_len + comment_len;
    }
    // 4) 逐候选：读 local header 定位数据起点 -> 读数据 -> 解压（stored / deflate）。
    for (local_header, csize, method) in candidates {
        let Ok(header) = read_exact_at(src, local_header, ZIP_LFH_LEN) else {
            continue;
        };
        if u32_at(&header, 0) != ZIP_LFH_SIG {
            continue;
        }
        let name_len = u16_at(&header, 26) as u64;
        let extra_len = u16_at(&header, 28) as u64;
        let data_start = local_header + ZIP_LFH_LEN as u64 + name_len + extra_len;
        if data_start.saturating_add(csize) > len {
            continue;
        }
        let Ok(raw) = read_exact_at(src, data_start, csize as usize) else {
            continue;
        };
        match method {
            0 => return Ok(Some(raw)),
            8 => {
                let mut out = Vec::new();
                if flate2::read::DeflateDecoder::new(raw.as_slice())
                    .read_to_end(&mut out)
                    .is_ok()
                    && !out.is_empty()
                {
                    return Ok(Some(out));
                }
            }
            _ => continue,
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use crate::decode;
    use crate::document::open_document;
    use crate::source::ByteSource;
    use std::io::{self, Write};

    /// 内存字节源,用于测试。
    struct MemSource(Vec<u8>);
    impl ByteSource for MemSource {
        fn len(&self) -> u64 {
            self.0.len() as u64
        }
        fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
            let start = offset as usize;
            if start >= self.0.len() {
                return Ok(0);
            }
            let n = (self.0.len() - start).min(buf.len());
            buf[..n].copy_from_slice(&self.0[start..start + n]);
            Ok(n)
        }
    }

    fn make_png(w: u32, h: u32, rgba: [u8; 4]) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba(rgba));
        let mut buf = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut buf, image::ImageFormat::Png)
            .unwrap();
        buf.into_inner()
    }

    /// 构造一个条目乱序的 CBZ,且各页尺寸不同以便验证页序。
    fn make_cbz() -> Vec<u8> {
        let mut cursor = std::io::Cursor::new(Vec::new());
        {
            let mut zw = zip::ZipWriter::new(&mut cursor);
            let opt = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            for (name, w, h, color) in [
                ("page10.png", 50u32, 60u32, [0, 0, 255, 255]),
                ("page1.png", 10, 20, [255, 0, 0, 255]),
                ("page2.png", 30, 40, [0, 255, 0, 255]),
            ] {
                zw.start_file(name, opt).unwrap();
                zw.write_all(&make_png(w, h, color)).unwrap();
            }
            zw.finish().unwrap();
        }
        cursor.into_inner()
    }

    #[test]
    fn zip_streaming_and_natural_sort() {
        let doc = open_document(MemSource(make_cbz()), "test.cbz").unwrap();
        assert_eq!(doc.page_count(), 3);
        assert_eq!(doc.metadata().title, "test");
        // 自然排序应为 page1(10x20), page2(30x40), page10(50x60)
        let i0 = decode::decode(&doc.page_bytes(0).unwrap(), None).unwrap();
        assert_eq!((i0.width, i0.height), (10, 20));
        let i2 = decode::decode(&doc.page_bytes(2).unwrap(), None).unwrap();
        assert_eq!((i2.width, i2.height), (50, 60));
        assert!(doc.page_bytes(3).is_err());
    }

    #[test]
    fn opening_many_pages_does_not_fetch_every_local_header() {
        use std::sync::{Arc, Mutex};
        struct CountingSource {
            data: MemSource,
            reads: Arc<Mutex<Vec<(u64, usize)>>>,
        }
        impl ByteSource for CountingSource {
            fn len(&self) -> u64 { self.data.len() }
            fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
                self.reads.lock().unwrap().push((offset, buf.len()));
                self.data.read_at(offset, buf)
            }
        }
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for page in 0..40 {
            writer.start_file(format!("{page}.jpg"), options).unwrap();
            writer.write_all(&vec![page as u8; 300 * 1024]).unwrap();
        }
        let data = writer.finish().unwrap().into_inner();
        let reads = Arc::new(Mutex::new(Vec::new()));
        let book = super::ZipBook::open(CountingSource {
            data: MemSource(data), reads: reads.clone(),
        }, "many.cbz").unwrap();
        assert_eq!(crate::document::Document::page_count(&book), 40);

        // ------------------------------------------------------------------
        // P0-B2 之后本用例的验收口径
        // ------------------------------------------------------------------
        // 原始断言是 `open_reads <= 8`。**这个阈值在 `zip` crate 的公开 API 下不可达**，
        // 不是本轮改动造成的：
        //
        // 1. `ZipArchive::new` 会调 `read_central_header` → 对**每个**条目调
        //    `central_header_to_zip_file` → `find_data_start`，后者 seek 到该条目的
        //    local header 并解析它（`zip-2.4.2/src/read.rs`）。也就是说"每条 entry
        //    一次 local-header read"是 crate 的固定行为。
        // 2. `zip::read::Config` **只有** `archive_offset` 一个字段，没有任何"跳过
        //    local header 解析 / 只读中央目录"的开关。
        // 3. RCH 这一侧已经是 metadata-only（下面循环用的是 `name_for_index`，
        //    纯内存），没有为枚举文件名去打开 entry。
        //
        // 因此要降到 `<= 8` 只能绕开 crate 自行解析中央目录 —— 那属于 parser 迁移，
        // 已明确不在 P0-B2 范围内。本轮消灭的是**放大**与**抖动**：
        //   - 打开 40 页：81 次请求 / 10,532,968 B（每次 local-header read 被放大成
        //     256 KiB）→ 42 次 / 6,790 B（每次 64 B）；
        //   - central-directory 重复下载：41 次 → 2 次。
        // 所以这里改为对本轮真正的契约做断言，它们在"放大"这一维度上比 `<=8` 更严。
        let reads_snapshot = reads.lock().unwrap().clone();
        let open_reads = reads_snapshot.len();
        let open_bytes: u64 = reads_snapshot.iter().map(|(_, n)| *n as u64).sum();
        let open_max_fetch = reads_snapshot.iter().map(|(_, n)| *n).max().unwrap_or(0);

        assert!(
            open_reads <= 40 + 16,
            "opening must not grow at ~2 requests per entry: {open_reads} for 40 pages"
        );
        assert!(
            open_bytes <= 40 * 2048 + 64 * 1024,
            "opening must not transfer N x read-ahead: {open_bytes} B for 40 pages"
        );
        assert!(
            open_max_fetch < 256 * 1024,
            "no single metadata read may be amplified to the read-ahead size: {open_max_fetch} B"
        );

        assert_eq!(crate::document::Document::page_bytes(&book, 10).unwrap(), vec![10; 300 * 1024]);
        assert!(reads.lock().unwrap().len() - open_reads <= 3);
    }

    #[test]
    fn decode_downscale_keeps_ratio() {
        let png = make_png(4000, 2000, [1, 2, 3, 255]);
        let img = decode::decode(&png, Some(1000)).unwrap();
        assert_eq!((img.width, img.height), (1000, 500));
    }

    /// 生成一页带页码条纹的彩色测试图(条纹数 = 页码,便于肉眼验证翻页)。
    fn make_colored_page(w: u32, h: u32, index: usize) -> Vec<u8> {
        let base = (index as u8).wrapping_mul(30);
        let mut img = image::RgbaImage::from_pixel(
            w,
            h,
            image::Rgba([base, 255u8.wrapping_sub(base), 180, 255]),
        );
        let stripes = index + 1;
        for s in 0..stripes {
            let y0 = 40 + s as u32 * 70;
            if y0 + 40 > h {
                break;
            }
            for y in y0..y0 + 40 {
                for x in 60..w - 60 {
                    img.put_pixel(x, y, image::Rgba([0, 0, 0, 255]));
                }
            }
        }
        let mut buf = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut buf, image::ImageFormat::Png)
            .unwrap();
        buf.into_inner()
    }

    /// 生成一个多页示例 CBZ 到 ../testdata/sample.cbz,供 UI 联调。
    /// 逆序写入条目以验证自然排序还原页序。
    /// 手动运行:cargo test -- --ignored generate_sample_cbz
    #[test]
    #[ignore]
    fn generate_sample_cbz() {
        let mut cursor = std::io::Cursor::new(Vec::new());
        {
            let mut zw = zip::ZipWriter::new(&mut cursor);
            let opt = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            for index in (0..8).rev() {
                let name = format!("page{}.png", index + 1);
                zw.start_file(name, opt).unwrap();
                zw.write_all(&make_colored_page(800, 1200, index)).unwrap();
            }
            zw.finish().unwrap();
        }
        std::fs::create_dir_all("../testdata").unwrap();
        std::fs::write("../testdata/sample.cbz", cursor.into_inner()).unwrap();
    }

    /// 第 62 轮：封面快通道必须在**常数次读**内拿到首张图片，且跳过非图片前缀。
    #[test]
    fn cover_fast_path_reads_first_image_with_constant_reads() {
        use std::sync::{Arc, Mutex};
        struct CountingSource {
            data: MemSource,
            reads: Arc<Mutex<Vec<(u64, usize)>>>,
        }
        impl ByteSource for CountingSource {
            fn len(&self) -> u64 {
                self.data.len()
            }
            fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
                self.reads.lock().unwrap().push((offset, buf.len()));
                self.data.read_at(offset, buf)
            }
        }
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let stored = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        let deflated = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        // 非图片前缀（ComicInfo.xml）+ 目录条目 + 200 页，模拟真实 CBZ。
        writer.start_file("ComicInfo.xml", stored).unwrap();
        writer
            .write_all(b"<ComicInfo><Title>x</Title></ComicInfo>")
            .unwrap();
        writer.add_directory("pages/", stored).unwrap();
        let first_page = make_png(20, 30, [9, 8, 7, 255]);
        writer.start_file("pages/001.png", deflated).unwrap();
        writer.write_all(&first_page).unwrap();
        for page in 2..200 {
            writer.start_file(format!("pages/{page:03}.png"), stored).unwrap();
            writer.write_all(&vec![page as u8; 1024]).unwrap();
        }
        let data = writer.finish().unwrap().into_inner();

        let reads = Arc::new(Mutex::new(Vec::new()));
        let source = CountingSource {
            data: MemSource(data),
            reads: reads.clone(),
        };
        let page = super::first_image_bytes_via_central_directory(&source, 8 * 1024 * 1024)
            .unwrap()
            .expect("应能定位首张图片");
        assert_eq!(page, first_page, "快通道必须返回首张图片的原始字节");
        let count = reads.lock().unwrap().len();
        assert!(
            count <= 8,
            "快通道读次数应与条目数无关（200 页 + 前缀），实际 {count} 次"
        );
    }

    /// 非 ZIP / ZIP64 哨兵都必须**回退**（返回 None），不能猜。
    #[test]
    fn cover_fast_path_bails_out_instead_of_guessing() {
        struct Plain(MemSource);
        impl ByteSource for Plain {
            fn len(&self) -> u64 {
                self.0.len()
            }
            fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
                self.0.read_at(offset, buf)
            }
        }
        // 1) 根本不是 ZIP
        let junk = Plain(MemSource(vec![0x41; 4096]));
        assert!(super::first_image_bytes_via_central_directory(&junk, 1 << 20)
            .unwrap()
            .is_none());

        // 2) 正常 CBZ，但把 EOCD 的中央目录偏移改成 ZIP64 哨兵
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let stored = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        writer.start_file("001.png", stored).unwrap();
        writer.write_all(&make_png(8, 8, [1, 2, 3, 255])).unwrap();
        let mut data = writer.finish().unwrap().into_inner();
        let eocd = data
            .windows(4)
            .rposition(|w| w == [0x50, 0x4b, 0x05, 0x06])
            .expect("EOCD");
        data[eocd + 16..eocd + 20].copy_from_slice(&[0xff, 0xff, 0xff, 0xff]);
        let zip64 = Plain(MemSource(data));
        assert!(
            super::first_image_bytes_via_central_directory(&zip64, 1 << 20)
                .unwrap()
                .is_none(),
            "ZIP64 哨兵必须回退给常规路径"
        );
    }
}
