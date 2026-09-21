//! 格式解析:把一本漫画(ZIP/CBZ/PDF/EPUB/…)解析为"有序页字节流"。
//!
//! 新格式只需实现 [`Document`] 并在 [`open_document`] 注册。

pub mod comicinfo;
pub mod epub;
pub mod folder;
pub mod mobi;
pub mod pdf;
pub mod rar;
pub mod remote_folder;
pub mod sevenz;
pub mod tar;
pub mod zip;

use crate::source::ByteSource;
use anyhow::Result;

/// 一本书的元数据。
#[derive(Debug, Clone, Default)]
pub struct DocumentMeta {
    pub title: String,
    pub author: String,
    pub genre: String,
    pub series: String,
}

/// 一本书的解析结果:页列表 + 按需取页 + 元数据。
/// 实现必须 `Send + Sync` 且 `page_bytes` 无内部可变状态,以便并发调用(并行预取)。
pub trait Document: Send + Sync {
    fn page_count(&self) -> u32;
    fn metadata(&self) -> DocumentMeta {
        DocumentMeta::default()
    }
    /// 读取第 `index` 页的原始图片字节(流式,按需,无内部可变状态)。
    fn page_bytes(&self, index: u32) -> Result<Vec<u8>>;

    /// 按**显示尺寸**渲染一页（封面等"只看小图"的场景）。
    ///
    /// 默认实现回退到 [`Document::page_bytes`]：大多数格式的页本身就是一张现成图片，
    /// 缩小/裁剪由调用方完成。**PDF 覆盖它**：让 pdfium 直接按目标宽度栅格化 ——
    /// 栅格化成本随宽度平方下降（1600 → 340；长条页因高度上限实测 ≈15×），封面不必为一张 340×480 的图
    /// 渲染 1600×2 万像素的长条页（真机实测：1600px 单页 0.7–6.8 s、输出最大 7.3 MB）。
    fn page_bytes_for_display(&self, index: u32, _target_width: u32) -> Result<Vec<u8>> {
        self.page_bytes(index)
    }
}

/// 打开本地目录为书籍(Folder 格式,不经过 ByteSource)。
pub fn open_folder_document(dir_path: &str) -> Result<Box<dyn Document>> {
    Ok(Box::new(folder::FolderBook::open(dir_path)?))
}

/// 按文件扩展名打开一本书(需要 ByteSource)。
pub fn open_document<S: ByteSource + 'static>(src: S, path: &str) -> Result<Box<dyn Document>> {
    let lower = path.to_lowercase();
    if lower.ends_with(".zip") || lower.ends_with(".cbz") {
        Ok(Box::new(zip::ZipBook::open(src, path)?))
    } else if lower.ends_with(".epub") {
        Ok(Box::new(epub::EpubBook::open(src, path)?))
    } else if lower.ends_with(".cb7") || lower.ends_with(".7z") {
        Ok(Box::new(sevenz::SevenZBook::open(src, path)?))
    } else if lower.ends_with(".cbt") || lower.ends_with(".tar") {
        Ok(Box::new(tar::TarBook::open(src, path)?))
    } else if lower.ends_with(".pdf") {
        Ok(Box::new(pdf::PdfBook::open(src, path)?))
    } else if lower.ends_with(".cbr") || lower.ends_with(".rar") {
        Ok(Box::new(rar::RarBook::open(src, path)?))
    } else if lower.ends_with(".mobi") || lower.ends_with(".azw") || lower.ends_with(".azw3") {
        Ok(Box::new(mobi::MobiBook::open(src, path)?))
    } else {
        anyhow::bail!("暂不支持的格式(本期支持 ZIP/CBZ/EPUB/CB7/CBT/PDF/CBR/MOBI): {path}")
    }
}

/// 封面专用打开（2026-09-21）：只保证 **page 0** 可读且是可解码图片。
///
/// 为什么需要单独的入口：封面抓取有 **30 s 挂钟预算**（`COVER_READ_BUDGET_MS`），
/// 而 MOBI 的惰性打开要**逐条探测候选记录的魔数**——远端每条一次 Range 往返。
/// 一本 300 页的漫画 MOBI 因此要 200–300 次往返（实测平均每次仅 28.8 字节、合计约 30 s）
/// ⇒ 必然撞穿预算（真机 `cover_read_budget_exceeded` 累计 281 条，MOBI 封面长期不显示）。
/// 封面只用第一张图，所以 MOBI 走"探测到第一张就停"；**其它格式行为完全不变**
/// （它们的 page 0 本来就不需要逐条探测）。
pub fn open_cover_document<S: ByteSource + 'static>(
    src: S,
    path: &str,
) -> Result<Box<dyn Document>> {
    let lower = path.to_lowercase();
    if lower.ends_with(".mobi") || lower.ends_with(".azw") || lower.ends_with(".azw3") {
        Ok(Box::new(mobi::MobiBook::open_cover(src, path)?))
    } else {
        open_document(src, path)
    }
}
