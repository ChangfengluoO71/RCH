//! CB7 (7z) 格式解析。
//!
//! 7z 压缩包格式,常用于漫画分发。用 sevenz-rust 纯 Rust 解析。
//!
//! 注意:7z 不支持流式——需要完整解压到内存。

use super::{Document, DocumentMeta};
use crate::source::ByteSource;
use anyhow::{Context, Result};

pub struct SevenZBook {
    pages: Vec<Vec<u8>>,
    title: String,
}

impl SevenZBook {
    pub fn open(mut src: impl ByteSource, path: &str) -> Result<Self> {
        let len = src.len() as usize;
        let mut data = vec![0u8; len];
        src.read_exact_at(0, &mut data)
            .context("读取 7z 文件失败")?;

        let cursor = std::io::Cursor::new(data);
        // sevenz-rust 0.6 API: SevenZReader::open(impl AsRef<Path> + SevenZMethodDecoder)
        // 不支持从内存读取,需要用 decompress_file 系列函数
        // 先把数据写临时文件
        let tmp = std::env::temp_dir().join(format!("rch_7z_{}", std::process::id()));
        std::fs::write(&tmp, cursor.into_inner()).context("写入临时文件失败")?;

        let mut raw_entries = Vec::new();
        let dest = std::env::temp_dir().join(format!("rch_7z_out_{}", std::process::id()));
        std::fs::create_dir_all(&dest).ok();

        sevenz_rust::decompress_file_with_extract_fn(&tmp, &dest, |entry, reader, _out_path| {
            let name = entry.name().to_string();
            if is_image_name(&name) {
                let mut out = Vec::new();
                if reader.read_to_end(&mut out).is_ok() {
                    raw_entries.push((name, out));
                }
            }
            Ok(true)
        })
        .context("解压 7z 文件失败")?;

        let _ = std::fs::remove_file(&tmp);

        if raw_entries.is_empty() {
            anyhow::bail!("7z 包中没有图片文件: {path}");
        }

        raw_entries.sort_by(|a, b| crate::util::natural_cmp(&a.0, &b.0));
        let pages: Vec<Vec<u8>> = raw_entries.into_iter().map(|(_, data)| data).collect();

        let title = std::path::Path::new(path)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string());

        Ok(SevenZBook { pages, title })
    }
}

impl Document for SevenZBook {
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
