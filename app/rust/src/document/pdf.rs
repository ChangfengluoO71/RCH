//! PDF 格式解析。
//!
//! 用 pdfium-render (Google PDFium 的 Rust 绑定) 渲染 PDF 页面为位图。

use super::{Document, DocumentMeta};
use crate::source::ByteSource;
use anyhow::{Context, Result};
use pdfium_render::prelude::*;
use std::sync::OnceLock;

static PDFIUM: OnceLock<Pdfium> = OnceLock::new();

fn get_pdfium() -> &'static Pdfium {
    PDFIUM.get_or_init(|| {
        Pdfium::new(
            Pdfium::bind_to_library(
                Pdfium::pdfium_platform_library_name_at_path("./"),
            )
            .or_else(|_| {
                Pdfium::bind_to_library(
                    Pdfium::pdfium_platform_library_name_at_path(""),
                )
            })
            .or_else(|_| Pdfium::bind_to_system_library())
            .expect("无法加载 pdfium 动态库,请将 pdfium.dll 放在 app.exe 同目录"),
        )
    })
}

pub struct PdfBook {
    pages: Vec<Vec<u8>>,
    title: String,
}

impl PdfBook {
    pub fn open(src: impl ByteSource, path: &str) -> Result<Self> {
        let len = src.len() as usize;
        let mut data = vec![0u8; len];
        src.read_exact_at(0, &mut data)
            .context("读取 PDF 文件失败")?;

        let pdfium = get_pdfium();
        let doc = pdfium
            .load_pdf_from_byte_vec(data, None)
            .context("加载 PDF 失败(可能是加密或损坏)")?;

        let page_count = doc.pages().len() as usize;
        let mut pages = Vec::with_capacity(page_count);

        for i in 0..page_count {
            let page = doc
                .pages()
                .get(i as i32)
                .with_context(|| format!("获取 PDF 第 {i} 页失败"))?;

            let render_width: Pixels = 1600;
            let h = page.height();
            let w = page.width();
            let height: Pixels = (h.value as f64 * 1600.0 / w.value as f64) as Pixels;
            let bitmap = page
                .render(render_width, height, None)
                .with_context(|| format!("渲染 PDF 第 {i} 页失败"))?;

            let img = bitmap
                .as_image()
                .with_context(|| format!("PDF 位图转图片失败: 第 {i} 页"))?;

            let mut buf = Vec::new();
            let mut cursor = std::io::Cursor::new(&mut buf);
            img.write_to(&mut cursor, image::ImageFormat::WebP)
                .with_context(|| format!("编码 PDF 第 {i} 页为 WebP 失败"))?;
            pages.push(buf);
        }

        if pages.is_empty() {
            anyhow::bail!("PDF 没有页面: {path}");
        }

        let title = std::path::Path::new(path)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string());

        Ok(PdfBook { pages, title })
    }
}

impl Document for PdfBook {
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
        self.pages
            .get(index as usize)
            .cloned()
            .with_context(|| format!("页索引越界: {index}"))
    }
}

