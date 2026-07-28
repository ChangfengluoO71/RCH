//! MOBI 格式解析。
//!
//! 用 mobi crate(v0.8.0) 纯 Rust 解析 MOBI 文件,
//! 提取其中的图片记录作为书页。不需要 Calibre CLI。

use super::{Document, DocumentMeta};
use crate::source::ByteSource;
use anyhow::{Context, Result};

pub struct MobiBook {
    pages: Vec<Vec<u8>>,
    title: String,
}

impl MobiBook {
    pub fn open(src: impl ByteSource, _path: &str) -> Result<Self> {
        let len = src.len() as usize;
        let mut data = vec![0u8; len];
        src.read_exact_at(0, &mut data)
            .context("读取 MOBI 文件失败")?;

        let mobi = mobi::Mobi::new(data).context("解析 MOBI 文件失败")?;

        let title = mobi.title();

        // 提取所有图片记录
        let image_records = mobi.image_records();

        if image_records.is_empty() {
            // 纯文字 MOBI: 尝试从 HTML 内容中解析图片引用 (data: URI)
            anyhow::bail!(
                "MOBI 中没有图片。标题: {title}。此 MOBI 可能是纯文字小说, 暂不支持。"
            );
        }

        let mut pages = Vec::with_capacity(image_records.len());
        for record in image_records {
            pages.push(record.content.to_vec());
        }

        Ok(MobiBook { pages, title })
    }
}

impl Document for MobiBook {
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
