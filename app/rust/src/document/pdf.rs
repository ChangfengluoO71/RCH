//! PDF 格式解析。
//!
//! 用 pdfium-render (Google PDFium 的 Rust 绑定) 渲染 PDF 页面为位图。

use super::{Document, DocumentMeta};
use crate::source::ByteSource;
use anyhow::{Context, Result};
use pdfium_render::prelude::*;
use std::sync::OnceLock;

static PDFIUM: OnceLock<Result<Pdfium, String>> = OnceLock::new();

/// pdfium 原生库目录（Android：由 Dart 侧传入 `ApplicationInfo.nativeLibraryDir`）。
static NATIVE_LIB_DIR: OnceLock<String> = OnceLock::new();

/// 设置 pdfium 动态库所在目录。设置后打开 PDF 时优先从该目录加载
/// `libpdfium.so`（Android 打包进 jniLibs 后即位于 nativeLibraryDir）。
pub fn set_native_lib_dir(dir: String) {
    let _ = NATIVE_LIB_DIR.set(dir);
}

fn get_pdfium() -> Result<&'static Pdfium> {
    PDFIUM
        .get_or_init(|| {
            // 依次尝试：nativeLibraryDir(Android) → 进程工作目录 → RCH.exe 所在目录 → PATH → 系统目录。
            let mut dirs: Vec<String> = vec![];
            if let Some(dir) = NATIVE_LIB_DIR.get() {
                if !dir.is_empty() {
                    dirs.push(dir.clone());
                }
            }
            dirs.push("./".to_string());
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
    doc: PdfDocument<'static>,
    title: String,
}

impl PdfBook {
    pub fn open(src: impl ByteSource, path: &str) -> Result<Self> {
        let len = src.len() as usize;
        let mut data = vec![0u8; len];
        src.read_exact_at(0, &mut data)
            .context("读取 PDF 文件失败")?;

        let pdfium = get_pdfium()?;
        // 懒加载：只解析文档与页数，页面按需渲染（page_bytes），
        // 避免整本 PDF 在 open 阶段全量栅格化导致下载 100% 后长时间无响应。
        let doc = pdfium
            .load_pdf_from_byte_vec(data, None)
            .context("加载 PDF 失败(可能是加密或损坏)")?;

        let title = std::path::Path::new(path)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string());

        Ok(PdfBook { doc, title })
    }
}

impl Document for PdfBook {
    fn page_count(&self) -> u32 {
        self.doc.pages().len() as u32
    }

    fn metadata(&self) -> DocumentMeta {
        DocumentMeta {
            title: self.title.clone(),
            ..Default::default()
        }
    }

    fn page_bytes(&self, index: u32) -> Result<Vec<u8>> {
        let page = self
            .doc
            .pages()
            .get(index as i32)
            .with_context(|| format!("获取 PDF 第 {index} 页失败"))?;
        let render_width: Pixels = 1600;
        let h = page.height();
        let w = page.width();
        let height: Pixels = (h.value as f64 * 1600.0 / w.value as f64) as Pixels;
        let bitmap = page
            .render(render_width, height, None)
            .with_context(|| format!("渲染 PDF 第 {index} 页失败"))?;
        let img = bitmap
            .as_image()
            .with_context(|| format!("PDF 位图转图片失败: 第 {index} 页"))?;
        let mut buf = Vec::new();
        let mut cursor = std::io::Cursor::new(&mut buf);
        img.write_to(&mut cursor, image::ImageFormat::WebP)
            .with_context(|| format!("编码 PDF 第 {index} 页为 WebP 失败"))?;
        Ok(buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pdfium_ffi_gate_serializes_concurrent_calls() {
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc, Barrier,
        };
        use std::time::Duration;

        let active = Arc::new(AtomicUsize::new(0));
        let max_active = Arc::new(AtomicUsize::new(0));
        let barrier = Arc::new(Barrier::new(3));
        let mut handles = Vec::new();

        for _ in 0..2 {
            let active = Arc::clone(&active);
            let max_active = Arc::clone(&max_active);
            let barrier = Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                with_pdfium_lock(|| {
                    let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                    max_active.fetch_max(now, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(40));
                    active.fetch_sub(1, Ordering::SeqCst);
                });
            }));
        }

        barrier.wait();
        for handle in handles {
            handle.join().expect("worker should not panic");
        }
        assert_eq!(max_active.load(Ordering::SeqCst), 1);
    }

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
