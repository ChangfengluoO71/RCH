//! PDF 格式解析。
//!
//! 用 pdfium-render (Google PDFium 的 Rust 绑定) 渲染 PDF 页面为位图。

use super::{Document, DocumentMeta};
use crate::diag::pdf_diag;
use crate::source::ByteSource;
use anyhow::{Context, Result};
use pdfium_render::prelude::*;
use std::sync::{Mutex, OnceLock};

static PDFIUM: OnceLock<Result<Pdfium, String>> = OnceLock::new();

/// PDFium 本身不是可重入的。pdfium-render 0.9.3 虽暴露 Send + Sync，
/// 但不会替调用方序列化 FFI，因此所有生产 PDFium 调用必须经过同一个进程级 gate。
static PDFIUM_FFI_LOCK: Mutex<()> = Mutex::new(());

fn with_pdfium_lock<T>(f: impl FnOnce() -> T) -> T {
    let _guard = PDFIUM_FFI_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    f()
}

const PDF_RENDER_TARGET_WIDTH: f64 = 1600.0;
const WEBP_MAX_DIMENSION: f64 = 16383.0;

fn fit_webp_render_dimensions(page_width: f64, page_height: f64) -> (Pixels, Pixels) {
    if !page_width.is_finite()
        || !page_height.is_finite()
        || page_width <= 0.0
        || page_height <= 0.0
    {
        return (1, 1);
    }

    let scale = (PDF_RENDER_TARGET_WIDTH / page_width)
        .min(WEBP_MAX_DIMENSION / page_width)
        .min(WEBP_MAX_DIMENSION / page_height);

    let width = (page_width * scale)
        .round()
        .clamp(1.0, WEBP_MAX_DIMENSION) as Pixels;
    let height = (page_height * scale)
        .round()
        .clamp(1.0, WEBP_MAX_DIMENSION) as Pixels;
    (width, height)
}

/// pdfium 原生库目录（Android：由 Dart 侧传入 `ApplicationInfo.nativeLibraryDir`）。
static NATIVE_LIB_DIR: OnceLock<String> = OnceLock::new();

/// 设置 pdfium 动态库所在目录。设置后打开 PDF 时优先从该目录加载
/// `libpdfium.so`（Android 打包进 jniLibs 后即位于 nativeLibraryDir）。
pub fn set_native_lib_dir(dir: String) {
    pdf_diag(format!("pdf set_native_lib_dir dir={dir}"));
    let _ = NATIVE_LIB_DIR.set(dir);
}

fn get_pdfium() -> Result<&'static Pdfium> {
    PDFIUM
        .get_or_init(|| {
            pdf_diag("pdf get_pdfium INIT");
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
                pdf_diag(format!("pdf bind attempt path={}", name.display()));
                match Pdfium::bind_to_library(name) {
                    Ok(bindings) => {
                        pdf_diag("pdf bind_to_library OK");
                        return Ok(Pdfium::new(bindings));
                    }
                    Err(e) => {
                        last_err = format!("{e}");
                        pdf_diag(format!("pdf bind_to_library ERR err={e}"));
                    }
                }
            }
            match Pdfium::bind_to_system_library() {
                Ok(bindings) => {
                    pdf_diag("pdf bind_to_system_library OK");
                    Ok(Pdfium::new(bindings))
                }
                Err(e) => {
                    pdf_diag(format!("pdf bind_to_system_library ERR err={e}"));
                    Err(format!(
                        "无法加载 pdfium 动态库，请将 pdfium.dll 放在 RCH.exe 同目录（从 \
                         bblanchon/pdfium-binaries 下载 win-x64 版本）。{last_err} {e}"
                    ))
                }
            }
        })
        .as_ref()
        .map_err(|e| anyhow::anyhow!(e.clone()))
}

pub struct PdfBook {
    // Option 允许 Drop 在持有全局 PDFium gate 时显式析构 PdfDocument，
    // 避免字段在锁释放后再次自动 drop。
    doc: Option<PdfDocument<'static>>,
    title: String,
}

impl PdfBook {
    pub fn open(src: impl ByteSource, path: &str) -> Result<Self> {
        let len = src.len() as usize;
        pdf_diag(format!("pdf open READ_SOURCE_START bytes={len}"));
        let mut data = vec![0u8; len];
        src.read_exact_at(0, &mut data)
            .context("读取 PDF 文件失败")?;
        pdf_diag(format!("pdf open READ_SOURCE_OK bytes={}", data.len()));

        let title = std::path::Path::new(path)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string());

        pdf_diag("pdf open WAIT_GATE");
        with_pdfium_lock(|| {
            pdf_diag("pdf open ACQUIRED_GATE");
            let pdfium = get_pdfium()?;
            pdf_diag("pdf open LOAD_DOCUMENT_START");
            // 懒加载：只解析文档与页数，页面按需渲染（page_bytes），
            // 避免整本 PDF 在 open 阶段全量栅格化导致下载 100% 后长时间无响应。
            let doc = pdfium
                .load_pdf_from_byte_vec(data, None)
                .context("加载 PDF 失败(可能是加密或损坏)")?;
            pdf_diag("pdf open LOAD_DOCUMENT_OK");

            Ok(PdfBook {
                doc: Some(doc),
                title,
            })
        })
    }

    fn doc(&self) -> &PdfDocument<'static> {
        self.doc.as_ref().expect("PDF document already closed")
    }
}

impl Drop for PdfBook {
    fn drop(&mut self) {
        if let Some(doc) = self.doc.take() {
            // PdfDocument 的 Drop 会回到 PDFium；必须和打开、页访问、渲染使用同一把锁。
            pdf_diag("pdf drop WAIT_GATE");
            with_pdfium_lock(|| {
                pdf_diag("pdf drop ACQUIRED_GATE");
                drop(doc);
                pdf_diag("pdf drop DONE");
            });
        }
    }
}

impl Document for PdfBook {
    fn page_count(&self) -> u32 {
        pdf_diag("pdf page_count WAIT_GATE");
        with_pdfium_lock(|| {
            pdf_diag("pdf page_count ACQUIRED_GATE");
            let count = self.doc().pages().len() as u32;
            pdf_diag(format!("pdf page_count OK count={count}"));
            count
        })
    }

    fn metadata(&self) -> DocumentMeta {
        DocumentMeta {
            title: self.title.clone(),
            ..Default::default()
        }
    }

    fn page_bytes(&self, index: u32) -> Result<Vec<u8>> {
        pdf_diag(format!("pdf page_bytes START index={index}"));
        // 将所有 PDFium 对象的访问和位图复制限制在同一临界区；
        // DynamicImage 已拥有自己的像素数据，WebP 编码可以在锁外并行执行。
        pdf_diag(format!("pdf page_bytes WAIT_GATE index={index}"));
        let img = with_pdfium_lock(|| -> Result<image::DynamicImage> {
            pdf_diag(format!("pdf page_bytes ACQUIRED_GATE index={index}"));
            pdf_diag(format!("pdf load_page START index={index}"));
            let page = self
                .doc()
                .pages()
                .get(index as i32)
                .with_context(|| format!("获取 PDF 第 {index} 页失败"))?;
            pdf_diag(format!("pdf load_page OK index={index}"));
            let h = page.height();
            let w = page.width();
            let (render_width, render_height) =
                fit_webp_render_dimensions(w.value as f64, h.value as f64);
            pdf_diag(format!(
                "pdf render START index={index} width={render_width} height={render_height} source_width={} source_height={}",
                w.value, h.value
            ));
            let bitmap = page
                .render(render_width, render_height, None)
                .with_context(|| format!("渲染 PDF 第 {index} 页失败"))?;
            pdf_diag(format!("pdf render OK index={index}"));
            let image = bitmap
                .as_image()
                .with_context(|| format!("PDF 位图转图片失败: 第 {index} 页"))?;
            pdf_diag(format!("pdf bitmap_copy OK index={index}"));
            Ok(image)
        })?;
        pdf_diag(format!("pdf page_bytes RELEASED_GATE index={index}"));

        let mut buf = Vec::new();
        let mut cursor = std::io::Cursor::new(&mut buf);
        pdf_diag(format!("pdf webp START index={index}"));
        img.write_to(&mut cursor, image::ImageFormat::WebP)
            .with_context(|| format!("编码 PDF 第 {index} 页为 WebP 失败"))?;
        pdf_diag(format!(
            "pdf webp OK index={index} bytes={}",
            buf.len()
        ));
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

    #[test]
    fn webp_render_dimensions_keep_normal_pages_at_target_width() {
        assert_eq!(fit_webp_render_dimensions(1000.0, 1500.0), (1600, 2400));
    }

    #[test]
    fn webp_render_dimensions_cap_ultra_tall_pages() {
        for source_height in [16826.0, 18864.0, 20066.0, 25672.0] {
            let (width, height) = fit_webp_render_dimensions(1600.0, source_height);
            assert!(width <= 1600, "width={width}");
            assert!(height <= WEBP_MAX_DIMENSION as Pixels, "height={height}");

            let source_ratio = source_height / 1600.0;
            let rendered_ratio = height as f64 / width as f64;
            let relative_error = ((rendered_ratio - source_ratio) / source_ratio).abs();
            assert!(
                relative_error < 0.002,
                "ratio drift too large: source={source_ratio} rendered={rendered_ratio}"
            );
        }
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
