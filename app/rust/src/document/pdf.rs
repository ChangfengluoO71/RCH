//! PDF 格式解析。
//!
//! 用 pdfium-render (Google PDFium 的 Rust 绑定) 渲染 PDF 页面为位图。

use super::{Document, DocumentMeta};
use crate::source::ByteSource;
use anyhow::{Context, Result};
use pdfium_render::prelude::*;
use std::sync::OnceLock;

static PDFIUM: OnceLock<Result<Pdfium, String>> = OnceLock::new();

fn get_pdfium() -> Result<&'static Pdfium> {
    PDFIUM
        .get_or_init(|| {
            // 依次尝试：进程工作目录 → RCH.exe 所在目录 → PATH → 系统目录。
            let mut dirs: Vec<String> = vec!["./".to_string()];
            if let Ok(exe) = std::env::current_exe() {
                if let Some(dir) = exe.parent() {
                    dirs.push(dir.to_string_lossy().into_owned());
                }
            }
            dirs.push(String::new());
            let mut last_err = String::new();
            for dir in &dirs {
                let name = Pdfium::pdfium_platform_library_name_at_path(dir);
                match Pdfium::bind_to_library(name) {
                    Ok(bindings) => return Ok(Pdfium::new(bindings)),
                    Err(e) => last_err = format!("{e}"),
                }
            }
            match Pdfium::bind_to_system_library() {
                Ok(bindings) => Ok(Pdfium::new(bindings)),
                Err(e) => Err(format!(
                    "无法加载 pdfium 动态库，请将 pdfium.dll 放在 RCH.exe 同目录（从 \
                     bblanchon/pdfium-binaries 下载 win-x64 版本）。{last_err} {e}"
                )),
            }
        })
        .as_ref()
        .map_err(|e| anyhow::anyhow!(e.clone()))
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

        let pdfium = get_pdfium()?;
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 开发机存在 pdfium.dll（cwd 或 PDFIUM_DLL_PATH）时验证可被 pdfium-render 加载。
    #[test]
    fn pdfium_dll_loads_when_present() {
        let dir = std::env::var("PDFIUM_DLL_PATH").unwrap_or_else(|_| "./".to_string());
        let name = Pdfium::pdfium_platform_library_name_at_path(&dir);
        if !name.exists() {
            return; // dll 缺失属部署问题，不在单测中失败
        }
        if let Err(e) = Pdfium::bind_to_library(&name) {
            panic!("pdfium.dll 应可加载: {e}");
        }
    }
}
