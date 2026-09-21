//! PDF 格式解析。
//!
//! 用 pdfium-render (Google PDFium 的 Rust 绑定) 渲染 PDF 页面为位图。

use super::{Document, DocumentMeta};
use crate::source::{ByteSource, SourceReader};
use anyhow::{Context, Result};
use pdfium_render::prelude::*;
use std::io::{Read, Seek};
use std::sync::{Arc, Mutex, OnceLock};

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

/// 交给 pdfium 的**按需**读取器（`FPDF_FILEACCESS` 回调的落地实现）。
///
/// 为什么不直接把 `SourceReader` 交给 `load_pdf_from_reader`：
/// 1. **语义**：`FPDF_FILEACCESS.m_GetBlock` 的返回值是"成功/失败"（非零即成功），
///    而 pdfium-render 把 `Read::read` 的返回值直接透传（`utils.rs` 的
///    `read_block_from_callback`：`reader.read(..).unwrap_or(0) as c_int`）⇒
///    **短读会被 pdfium 当成整块成功**，缓冲区尾部留下未初始化字节（解析错误或静默错页）。
///    所以这里的 `read` 必须是"填满或报错"。
/// 2. **成本**：pdfium 按随机小块取数，必须靠 `SourceReader` 的元数据小窗口 / 顺序预读
///    把远端请求摊薄，否则一页就是几十次往返。
struct PdfFetchReader<S: ByteSource> {
    inner: SourceReader<Arc<S>>,
    meter: PdfReadMeter,
}

/// pdfium 回调的读数计量表（现场诊断用，见 [`diag`]）。
#[derive(Clone, Default)]
struct PdfReadMeter {
    reads: Arc<std::sync::atomic::AtomicUsize>,
    bytes: Arc<std::sync::atomic::AtomicU64>,
}

impl PdfReadMeter {
    fn counts(&self) -> (usize, u64) {
        (
            self.reads.load(std::sync::atomic::Ordering::Relaxed),
            self.bytes.load(std::sync::atomic::Ordering::Relaxed),
        )
    }
}

impl<S: ByteSource> PdfFetchReader<S> {
    fn new(src: Arc<S>, meter: PdfReadMeter) -> Self {
        PdfFetchReader {
            inner: SourceReader::new(src),
            meter,
        }
    }
}

impl<S: ByteSource> Read for PdfFetchReader<S> {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        // 填满或报错：pdfium-render 会把 Err 映射成 0（= 失败），把短读当成成功。
        self.inner.read_exact(out)?;
        // 计的是"pdfium 向底层要了多少"，下面再换算成真实远端读（SourceReader 会合并）。
        self.meter
            .reads
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.meter
            .bytes
            .fetch_add(out.len() as u64, std::sync::atomic::Ordering::Relaxed);
        Ok(out.len())
    }
}

impl<S: ByteSource> Seek for PdfFetchReader<S> {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(pos)
    }
}

/// 现场诊断：把 PDF 打开/取页的关键读数追加到 `<cache_root>/pdf_diag.log`。
///
/// 为什么需要：Android 上拿不到 `RCH_PERF_LOG` 环境变量（从桌面图标启动的应用继承不到），
/// 而"惰性按需读到底有没有生效"必须在真机上可见 —— 与既有的 `scan_diag.log` 同一套路子。
/// 只记**打开结果**与**每次取页**的一行摘要；文件超过上限时自动截断，绝不影响打开流程。
fn diag(line: &str) {
    use std::io::Write;
    const PDF_DIAG_MAX_BYTES: u64 = 1024 * 1024;
    let path = crate::cache::cache_root().join("pdf_diag.log");
    if std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0) > PDF_DIAG_MAX_BYTES {
        let _ = std::fs::remove_file(&path);
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let _ = writeln!(file, "{now} {line}");
    }
}

const PDF_RENDER_TARGET_WIDTH: f64 = 1600.0;
const WEBP_MAX_DIMENSION: f64 = 16383.0;

/// 按目标宽度算渲染尺寸（保持长宽比，长边受 WEBP 上限约束）。
///
/// 封面只显示 340×480，却按 1600 宽渲染的话，一张 1600×2 万像素的长条页要栅格化
/// 3200 万像素 ⇒ 真机实测每页 0.7–6.8 s、输出 WebP 最大 7.3 MB。按目标宽度渲染把
/// 栅格化面积按宽度平方降下来（1600 → 340；长条页因高度上限实测 ≈15×）。
fn fit_render_dimensions(page_width: f64, page_height: f64, target_width: f64) -> (Pixels, Pixels) {
    if !page_width.is_finite()
        || !page_height.is_finite()
        || page_width <= 0.0
        || page_height <= 0.0
    {
        return (1, 1);
    }

    let target_width = target_width.clamp(1.0, WEBP_MAX_DIMENSION);
    let scale = (target_width / page_width)
        .min(WEBP_MAX_DIMENSION / page_width)
        .min(WEBP_MAX_DIMENSION / page_height);

    let width = (page_width * scale).round().clamp(1.0, WEBP_MAX_DIMENSION) as Pixels;
    let height = (page_height * scale).round().clamp(1.0, WEBP_MAX_DIMENSION) as Pixels;
    (width, height)
}

/// pdfium 原生库目录（Android：由 Dart 侧传入 `ApplicationInfo.nativeLibraryDir`）。
static NATIVE_LIB_DIR: OnceLock<String> = OnceLock::new();

/// 设置 pdfium 动态库所在目录。设置后打开 PDF 时优先从该目录加载
/// `libpdfium.so`（Android 打包进 jniLibs 后即位于 nativeLibraryDir）。
pub fn set_native_lib_dir(dir: String) {
    let _ = NATIVE_LIB_DIR.set(dir);
}

/// **加载失败**的文案标记（单一事实来源）。
///
/// 为什么必须共用同一份：上游 `cover_open_reason` 只能靠文案把"部署缺库"与"文件打不开"
/// 分开；如果两边各写一份，任何一次文案改动都会让分类**静默失效**（真机教训见
/// `cover_open_reason` 的注释：pdfium-render 的库内错误文案里也含 "pdfium"）。
pub const PDFIUM_LOAD_FAILURE_MARKER: &str = "无法加载 pdfium 动态库";

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
                    "{PDFIUM_LOAD_FAILURE_MARKER}，请将 pdfium.dll 放在 RCH.exe 同目录（从 \
                     bblanchon/pdfium-binaries 下载 win-x64 版本）。{last_err} {e}"
                )),
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
    /// pdfium 回调读数计量（现场诊断用；整份读入路径恒为 0）。
    meter: PdfReadMeter,
}

impl PdfBook {
    /// 打开 PDF：**惰性按需读** —— pdfium 通过 `FPDF_FILEACCESS` 回调向我们取字节
    /// （`pdfium-render` 的 `load_pdf_from_reader` 只装 `m_GetBlock`，不预先缓冲整份文件），
    /// 外面套 `SourceReader`（顺序预读 + 元数据小窗口）把远端请求摊薄。
    ///
    /// 第 79 轮：过去这里是"整份读进内存再 `load_pdf_from_byte_vec`"，于是
    /// **远端 PDF（含封面）等于整包下载**（现场库里 609 个远端 PDF，封面实测每枚 17–34 MB），
    /// 还叠了一条 128 MB 的`cover_pdf_bytes_limit`硬拒。惰性加载后 pdfium 只取它真正需要
    /// 的对象/xref/首页数据；读不动时**回退**整份读入，保证不劣化。
    pub fn open(src: impl ByteSource + 'static, path: &str) -> Result<Self> {
        let title = pdf_title(path);
        let shared = Arc::new(src);
        let size = shared.len();
        with_pdfium_lock(|| {
            let pdfium = get_pdfium()?;
            let meter = PdfReadMeter::default();
            let started = std::time::Instant::now();
            let doc = match pdfium.load_pdf_from_reader(
                PdfFetchReader::new(Arc::clone(&shared), meter.clone()),
                None,
            ) {
                Ok(doc) => {
                    let (reads, bytes) = meter.counts();
                    diag(&format!(
                        "pdf_open mode=lazy ms={} lazy_reads={reads} lazy_bytes={bytes} size={size} name={title}",
                        started.elapsed().as_millis()
                    ));
                    doc
                }
                Err(lazy_error) => {
                    let reason = lazy_error.to_string().replace(['\n', '\r'], " ");
                    tracing::warn!("PDF 惰性加载失败，回退整份读入: {reason}");
                    let doc = load_eager(pdfium, shared.as_ref())
                        .context("加载 PDF 失败(可能是加密或损坏)")?;
                    diag(&format!(
                        "pdf_open mode=eager reason=lazy_failed:{reason} ms={} size={size} name={title}",
                        started.elapsed().as_millis()
                    ));
                    doc
                }
            };
            Ok(PdfBook {
                doc: Some(doc),
                title,
                meter,
            })
        })
    }

    /// **历史行为**：整份读入内存再交给 pdfium（逐字节等价于第 78 轮之前的实现）。
    ///
    /// 只为 A/B 量化而存在（惰性加载失败时的回退也走同一条 `load_eager`）：
    /// 同一份文件上对比"改前 vs 改后"的读取字节数，不必切换二进制。
    pub fn open_eager(src: impl ByteSource, path: &str) -> Result<Self> {
        let title = pdf_title(path);
        let size = src.len();
        with_pdfium_lock(|| {
            let pdfium = get_pdfium()?;
            let started = std::time::Instant::now();
            let doc = load_eager(pdfium, &src).context("加载 PDF 失败(可能是加密或损坏)")?;
            diag(&format!(
                "pdf_open mode=eager_ab ms={} size={size} name={title}",
                started.elapsed().as_millis()
            ));
            Ok(PdfBook {
                doc: Some(doc),
                title,
                meter: PdfReadMeter::default(),
            })
        })
    }

    fn doc(&self) -> &PdfDocument<'static> {
        self.doc.as_ref().expect("PDF document already closed")
    }

    /// 按目标宽度渲染一页为 WebP（`page_bytes` 与 `page_bytes_for_display` 的共用实现）。
    ///
    /// PDFium 对象访问与位图复制在同一临界区；`DynamicImage` 已拥有像素数据，
    /// WebP 编码可以在锁外并行执行。
    fn render_page(&self, index: u32, target_width: f64) -> Result<Vec<u8>> {
        let started = std::time::Instant::now();
        let (before_reads, before_bytes) = self.meter.counts();
        let img = with_pdfium_lock(|| -> Result<image::DynamicImage> {
            let page = self
                .doc()
                .pages()
                .get(index as i32)
                .with_context(|| format!("获取 PDF 第 {index} 页失败"))?;
            let h = page.height();
            let w = page.width();
            let (render_width, render_height) =
                fit_render_dimensions(w.value as f64, h.value as f64, target_width);
            let bitmap = page
                .render(render_width, render_height, None)
                .with_context(|| format!("渲染 PDF 第 {index} 页失败"))?;
            bitmap
                .as_image()
                .with_context(|| format!("PDF 位图转图片失败: 第 {index} 页"))
        })?;

        let mut buf = Vec::new();
        let mut cursor = std::io::Cursor::new(&mut buf);
        img.write_to(&mut cursor, image::ImageFormat::WebP)
            .with_context(|| format!("编码 PDF 第 {index} 页为 WebP 失败"))?;
        let (reads, bytes) = self.meter.counts();
        diag(&format!(
            "pdf_page index={index} width={} ms={} ask_reads={} ask_bytes={} out_bytes={} name={}",
            target_width as u32,
            started.elapsed().as_millis(),
            reads - before_reads,
            bytes - before_bytes,
            buf.len(),
            self.title
        ));
        Ok(buf)
    }
}

fn pdf_title(path: &str) -> String {
    std::path::Path::new(path)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

/// 整份读入 + `load_pdf_from_byte_vec`（历史路径，也是惰性加载失败时的回退）。
fn load_eager<'a>(pdfium: &'a Pdfium, src: &impl ByteSource) -> Result<PdfDocument<'a>> {
    let len = src.len() as usize;
    let mut data = vec![0u8; len];
    src.read_exact_at(0, &mut data)
        .context("读取 PDF 文件失败")?;
    Ok(pdfium.load_pdf_from_byte_vec(data, None)?)
}

impl Drop for PdfBook {
    fn drop(&mut self) {
        if let Some(doc) = self.doc.take() {
            // PdfDocument 的 Drop 会回到 PDFium；必须和打开、页访问、渲染使用同一把锁。
            with_pdfium_lock(|| drop(doc));
        }
    }
}

impl Document for PdfBook {
    fn page_count(&self) -> u32 {
        with_pdfium_lock(|| self.doc().pages().len() as u32)
    }

    fn metadata(&self) -> DocumentMeta {
        DocumentMeta {
            title: self.title.clone(),
            ..Default::default()
        }
    }

    fn page_bytes(&self, index: u32) -> Result<Vec<u8>> {
        self.render_page(index, PDF_RENDER_TARGET_WIDTH)
    }

    /// 第 79 轮：封面等"只看小图"的场景按目标宽度渲染（阅读仍走 1600px 的 `page_bytes`）。
    fn page_bytes_for_display(&self, index: u32, target_width: u32) -> Result<Vec<u8>> {
        self.render_page(index, target_width as f64)
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
        assert_eq!(
            fit_render_dimensions(1000.0, 1500.0, PDF_RENDER_TARGET_WIDTH),
            (1600, 2400)
        );
    }

    #[test]
    fn webp_render_dimensions_cap_ultra_tall_pages() {
        for source_height in [16826.0, 18864.0, 20066.0, 25672.0] {
            let (width, height) =
                fit_render_dimensions(1600.0, source_height, PDF_RENDER_TARGET_WIDTH);
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

    /// 第 79 轮：封面按显示宽度渲染 —— 栅格化面积随宽度平方下降（1600 → 340），
    /// 这是"手机上封面把 2 万像素高的长条页整张栅格化"的直接解药。
    ///
    /// 注意真实倍率是 **≈15×** 而不是 22×：1600px 那条路本身已被 `WEBP_MAX_DIMENSION`
    /// 截断（1600×20000 → 1311×16383），所以分母比理论值小。
    #[test]
    fn cover_render_dimensions_shrink_long_strip_raster_area() {
        let full = fit_render_dimensions(1600.0, 20000.0, PDF_RENDER_TARGET_WIDTH);
        let cover = fit_render_dimensions(1600.0, 20000.0, 340.0);
        assert_eq!(cover.0, 340);
        assert_eq!(cover.1, 4250);
        assert_eq!(full.1, WEBP_MAX_DIMENSION as Pixels, "1600px 路线受高度上限");
        let full_px = full.0 as u64 * full.1 as u64;
        let cover_px = cover.0 as u64 * cover.1 as u64;
        assert!(
            full_px / cover_px >= 14,
            "封面栅格化面积应至少降 14×：{full_px} → {cover_px}"
        );
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
