//! 格式解析:把一本漫画(ZIP/CBZ/PDF/EPUB/…)解析为"有序页字节流"。
//!
//! 新格式只需实现 [`Document`] 并在 [`open_document`] 注册。

pub mod comicinfo;
pub mod epub;
pub mod folder;
pub mod mobi;
pub mod pdf;
pub mod rar;
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
