//! CBT (tar) 格式解析。
//!
//! tar 格式,常用于漫画分发。用 tar crate 解析。
//! tar 不支持流式——需要读取整个包到内存后逐条解压。

use super::{Document, DocumentMeta};
use crate::source::ByteSource;
use anyhow::{Context, Result};
use std::io::Read;

pub struct TarBook {
    pages: Vec<Vec<u8>>,
    title: String,
}

impl TarBook {
    pub fn open(src: impl ByteSource, path: &str) -> Result<Self> {
        let len = src.len() as usize;
        let mut data = vec![0u8; len];
        src.read_exact_at(0, &mut data)
            .context("读取 tar 文件失败")?;

        let cursor = std::io::Cursor::new(data);
        let mut archive = tar::Archive::new(cursor);

        let mut raw_entries = Vec::new();
        for entry in archive.entries().context("读取 tar 条目失败")? {
            let mut entry = entry.context("读取 tar 条目失败")?;
            let name = entry.path()?.to_string_lossy().to_string();
            if is_image_name(&name) {
                let mut out = Vec::new();
                entry
                    .read_to_end(&mut out)
                    .context("读取 tar 条目数据失败")?;
                raw_entries.push((name, out));
            }
        }

        if raw_entries.is_empty() {
            anyhow::bail!("tar 包中没有图片文件: {path}");
        }

        raw_entries.sort_by(|a, b| crate::util::natural_cmp(&a.0, &b.0));
        let pages: Vec<Vec<u8>> = raw_entries.into_iter().map(|(_, data)| data).collect();

        let title = std::path::Path::new(path)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string());

        Ok(TarBook { pages, title })
    }
}

impl Document for TarBook {
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

fn is_image_name(name: &str) -> bool {
    let lower = name.to_lowercase();
    if lower.contains("__macosx") || lower.ends_with(".ds_store") {
        return false;
    }
    lower.ends_with(".jpg")
        || lower.ends_with(".jpeg")
        || lower.ends_with(".png")
        || lower.ends_with(".webp")
        || lower.ends_with(".gif")
        || lower.ends_with(".bmp")
        || lower.ends_with(".avif")
}
