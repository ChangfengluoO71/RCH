//! ZIP / CBZ 流式解析。
//!
//! 打开时只读文件尾部中心目录,拿到每页(图片 entry)在文件中的偏移与大小;
//! 之后每页按需用一次 Range 读取下载该页压缩数据并解压——
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
    data_start: u64,
    compressed_size: u64,
    deflated: bool,
}

/// ZIP/CBZ 书籍:中心目录定位各页,按需下载解压,`page_bytes` 无内部可变状态。
pub struct ZipBook<S: ByteSource> {
    src: S,
    pages: Vec<PageMeta>,
    title: String,
}

impl<S: ByteSource> ZipBook<S> {
    pub fn open(src: S, path: &str) -> Result<Self> {
        let reader = SourceReader::new(src);
        let mut zip = zip::ZipArchive::new(reader).context("打开 ZIP/CBZ 失败")?;
        let mut pages = Vec::new();
        for i in 0..zip.len() {
            let f = zip.by_index(i).context("读取中心目录失败")?;
            let name = f.name().to_string();
            if !is_image(&name) {
                continue;
            }
            pages.push(PageMeta {
                name,
                data_start: f.data_start(),
                compressed_size: f.compressed_size(),
                deflated: matches!(f.compression(), zip::CompressionMethod::Deflated),
            });
        }
        pages.sort_by(|a, b| crate::util::natural_cmp(&a.name, &b.name));
        let src = zip.into_inner().into_inner();
        let title = std::path::Path::new(path)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string());
        Ok(ZipBook { src, pages, title })
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
        let mut buf = vec![0u8; p.compressed_size as usize];
        self.src
            .read_exact_at(p.data_start, &mut buf)
            .context("下载页数据失败")?;
        if p.deflated {
            let mut out = Vec::new();
            flate2::read::DeflateDecoder::new(&buf[..])
                .read_to_end(&mut out)
                .context("Deflate 解压失败")?;
            Ok(out)
        } else {
            Ok(buf) // Stored:原样
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::document::open_document;
    use crate::decode;
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
}
