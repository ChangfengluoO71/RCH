//! Folder 格式:枚举目录下的图片文件,按自然排序阅读。
//!
//! 适用于直接存放图片的文件夹(非压缩包)。
//! 不依赖 ByteSource——直接通过文件系统读取。

use super::{Document, DocumentMeta};
use super::comicinfo::read_comicinfo;
use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

const IMAGE_EXTS: &[&str] = &["jpg", "jpeg", "png", "webp", "gif", "bmp", "avif"];

/// 封面候选文件名（按优先级从高到低）。
const COVER_NAMES: &[&str] = &["cover.jpg", "cover.png", "cover.webp", "cover.jpeg"];

fn is_image_name(name: &str) -> bool {
    let lower = name.to_lowercase();
    if lower.contains("__macosx") || lower.ends_with(".ds_store") {
        return false;
    }
    lower
        .rsplit('.')
        .next()
        .map(|ext| IMAGE_EXTS.contains(&ext))
        .unwrap_or(false)
}

pub struct FolderBook {
    files: Vec<String>, // 图片文件绝对路径,自然排序
    title: String,
    meta: DocumentMeta,
}

impl FolderBook {
    pub fn open(dir_path: &str) -> Result<Self> {
        let p = Path::new(dir_path);
        if !p.is_dir() {
            anyhow::bail!("不是目录: {dir_path}");
        }
        let mut files = Vec::new();
        for entry in fs::read_dir(p).context("读取目录失败")? {
            let entry = entry.context("读取目录条目失败")?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || name.starts_with("__MACOSX") {
                continue;
            }
            let ft = entry.file_type().context("获取文件类型失败")?;
            if ft.is_dir() {
                continue;
            }
            if is_image_name(&name) {
                files.push(entry.path().to_string_lossy().to_string());
            }
        }
        if files.is_empty() {
            anyhow::bail!("目录下没有图片文件: {dir_path}");
        }
        files.sort_by(|a, b| crate::util::natural_cmp(a, b));
        let title = p
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| dir_path.to_string());

        // 尝试读取 ComicInfo.xml
        let mut meta = DocumentMeta {
            title: title.clone(),
            ..Default::default()
        };
        let ci_path = p.join("ComicInfo.xml");
        if ci_path.exists() {
            if let Ok(ci) = read_comicinfo(&ci_path) {
                meta = super::comicinfo::comicinfo_to_meta(&ci);
                if meta.title.is_empty() {
                    meta.title = title.clone();
                }
            }
        }

        Ok(FolderBook { files, title, meta })
    }

    /// 查找目录下的封面图片文件（返回绝对路径）。
    pub fn cover_path(dir_path: &str) -> Option<String> {
        let p = Path::new(dir_path);
        if !p.is_dir() {
            return None;
        }
        for name in COVER_NAMES {
            let candidate = p.join(name);
            if candidate.exists() {
                return Some(candidate.to_string_lossy().to_string());
            }
        }
        None
    }
}

impl Document for FolderBook {
    fn page_count(&self) -> u32 {
        self.files.len() as u32
    }

    fn metadata(&self) -> DocumentMeta {
        self.meta.clone()
    }

    fn page_bytes(&self, index: u32) -> Result<Vec<u8>> {
        let file_path = self
            .files
            .get(index as usize)
            .with_context(|| format!("页索引越界: {index}"))?;
        fs::read(file_path).context("读取图片文件失败")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn folder_book_works() {
        let tmp = std::env::temp_dir().join("rch_test_folder");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();

        // 逆序写入,验证自然排序
        for (name, data) in [
            ("page10.png", b"ten" as &[u8]),
            ("page2.jpg", b"two"),
            ("page1.png", b"one"),
        ] {
            let mut f = fs::File::create(tmp.join(name)).unwrap();
            f.write_all(data).unwrap();
        }

        let book = FolderBook::open(&tmp.to_string_lossy()).unwrap();
        assert_eq!(book.page_count(), 3);
        assert_eq!(book.metadata().title, tmp.file_name().unwrap().to_string_lossy());

        // 自然排序: page1, page2, page10
        let b0 = book.page_bytes(0).unwrap();
        assert_eq!(b0, b"one");
        let b1 = book.page_bytes(1).unwrap();
        assert_eq!(b1, b"two");
        let b2 = book.page_bytes(2).unwrap();
        assert_eq!(b2, b"ten");
        assert!(book.page_bytes(3).is_err());

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn folder_book_rejects_file() {
        assert!(FolderBook::open("nonexistent_path_12345").is_err());
    }
}
