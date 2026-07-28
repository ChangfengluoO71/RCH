//! CBR (RAR) 格式解析。
//!
//! 用 unrar crate 解压 RAR 压缩包。
//! 注意: unrar crate 依赖系统已安装的 unrar.dll (UnRAR 库)。
//! 许可: UnRAR 源码可用但不可分发修改版。

use super::{Document, DocumentMeta};
use crate::source::ByteSource;
use anyhow::{Context, Result};

pub struct RarBook {
    pages: Vec<Vec<u8>>,
    title: String,
}

impl RarBook {
    pub fn open(src: impl ByteSource, path: &str) -> Result<Self> {
        // unrar crate 只能从文件路径打开,不支持内存读取
        // 对于 ByteSource,需要先把数据写入临时文件
        let len = src.len() as usize;
        let mut data = vec![0u8; len];
        src.read_exact_at(0, &mut data)
            .context("读取 CBR 文件失败")?;

        let ext = if path.to_lowercase().ends_with(".cbr") {
            ".cbr"
        } else {
            ".rar"
        };
        let tmp = std::env::temp_dir().join(format!("rch_cbr_{}{}", std::process::id(), ext));
        std::fs::write(&tmp, &data).context("写入临时文件失败")?;

        let mut archive = unrar::Archive::new(&tmp)
            .open_for_processing()
            .context("打开 CBR/RAR 文件失败(需要系统安装 unrar.dll)")?;

        let mut raw_entries = Vec::new();

        loop {
            let maybe_header = archive.read_header()
                .context("读取 RAR 头部失败")?;
            archive = match maybe_header {
                Some(header) => {
                    if !header.entry().is_file() {
                        header.skip()?
                    } else {
                        let name = header.entry().filename.to_string_lossy().to_string();
                        if is_image_name(&name) {
                            match header.read() {
                                Ok((bytes, next)) => {
                                    raw_entries.push((name, bytes));
                                    next
                                }
                                Err(e) => {
                                    tracing::warn!("RAR 解压条目失败: {name} {e}");
                                    break;
                                }
                            }
                        } else {
                            header.skip()?
                        }
                    }
                }
                None => break,
            };
        }

        let _ = std::fs::remove_file(&tmp);

        if raw_entries.is_empty() {
            anyhow::bail!("CBR 包中没有图片文件: {path}");
        }

        raw_entries.sort_by(|a, b| crate::util::natural_cmp(&a.0, &b.0));
        let pages: Vec<Vec<u8>> = raw_entries.into_iter().map(|(_, d)| d).collect();

        let title = std::path::Path::new(path)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string());

        Ok(RarBook { pages, title })
    }
}

impl Document for RarBook {
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
