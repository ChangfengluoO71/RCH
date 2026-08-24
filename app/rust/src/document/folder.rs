//! Folder 格式:枚举目录下的图片文件,按自然排序阅读。
//!
//! 适用于直接存放图片的文件夹(非压缩包)。
//! 不依赖 ByteSource——直接通过文件系统读取。
//!
//! 元数据源优先级: ComicInfo.xml > metadata.json > 目录名

use super::comicinfo::read_comicinfo;
use super::{Document, DocumentMeta};
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

/// metadata.json 中我们关心的字段。
/// 非标准格式，各发布者定义不同，这里采用宽松解析：所有字段可选。
#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
struct MetadataJson {
    title: Option<String>,
    #[serde(alias = "author")]
    writer: Option<String>,
    genre: Option<String>,
    series: Option<String>,
    summary: Option<String>,
    description: Option<String>,
    tags: Option<Vec<String>>,
}

/// 从 metadata.json 读取元数据。
fn read_metadata_json(dir: &Path) -> Option<MetadataJson> {
    let path = dir.join("metadata.json");
    if !path.exists() {
        return None;
    }
    let content = fs::read_to_string(&path).ok()?;
    serde_json::from_str::<MetadataJson>(&content).ok()
}

/// 将 metadata.json 映射到 DocumentMeta。
fn metadata_json_to_meta(mj: &MetadataJson) -> DocumentMeta {
    DocumentMeta {
        title: mj.title.clone().unwrap_or_default(),
        author: mj.writer.clone().unwrap_or_default(),
        genre: mj.genre.clone().unwrap_or_default(),
        series: mj.series.clone().unwrap_or_default(),
    }
}

/// 检测目录是否可当作漫画文件夹（包含至少一张图片）。
pub fn is_comic_folder(dir_path: &str) -> bool {
    let p = Path::new(dir_path);
    if !p.is_dir() {
        return false;
    }
    fs::read_dir(p)
        .ok()
        .map(|mut entries| {
            entries.any(|e| {
                e.ok()
                    .map(|entry| {
                        let name = entry.file_name().to_string_lossy().into_owned();
                        !name.starts_with('.')
                            && !name.starts_with("__MACOSX")
                            && !entry.file_type().map(|ft| ft.is_dir()).unwrap_or(true)
                            && is_image_name(&name)
                    })
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

pub struct FolderBook {
    files: Vec<String>, // 图片文件绝对路径,自然排序
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

        // 元数据: ComicInfo.xml > metadata.json > 目录名
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
        } else if let Some(mj) = read_metadata_json(p) {
            meta = metadata_json_to_meta(&mj);
            if meta.title.is_empty() {
                meta.title = title.clone();
            }
        }

        Ok(FolderBook { files, meta })
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
        assert_eq!(
            book.metadata().title,
            tmp.file_name().unwrap().to_string_lossy()
        );

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

    #[test]
    fn metadata_json_parsing_works() {
        let tmp = std::env::temp_dir().join("rch_test_meta_json");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();

        // 写入一张测试图片
        fs::write(tmp.join("001.jpg"), b"fake").unwrap();

        // 写入 metadata.json
        let json = r#"{"title": "Test Title", "author": "Test Author", "genre": "Action", "series": "Test Series", "description": "A test comic."}"#;
        fs::write(tmp.join("metadata.json"), json.as_bytes()).unwrap();

        let book = FolderBook::open(&tmp.to_string_lossy()).unwrap();
        let meta = book.metadata();
        assert_eq!(meta.title, "Test Title");
        assert_eq!(meta.author, "Test Author");
        assert_eq!(meta.genre, "Action");
        assert_eq!(meta.series, "Test Series");

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn cover_path_works() {
        let tmp = std::env::temp_dir().join("rch_test_cover");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();

        // 无封面时返回 None
        assert!(FolderBook::cover_path(&tmp.to_string_lossy()).is_none());

        // 写入 cover.jpg → 应检测到
        fs::write(tmp.join("cover.jpg"), b"fake").unwrap();
        let found = FolderBook::cover_path(&tmp.to_string_lossy());
        assert!(found.is_some());
        assert!(found.unwrap().ends_with("cover.jpg"));

        // cover.png 优先级低于 cover.jpg，但 cover.jpg 先被找到
        fs::write(tmp.join("cover.png"), b"fake").unwrap();
        let found2 = FolderBook::cover_path(&tmp.to_string_lossy());
        assert!(found2.is_some());
        assert!(found2.unwrap().ends_with("cover.jpg"));

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn is_comic_folder_works() {
        let tmp = std::env::temp_dir().join("rch_test_detect");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();

        // 空目录 → false
        assert!(!is_comic_folder(&tmp.to_string_lossy()));

        // 只有非图片文件 → false
        fs::write(tmp.join("readme.txt"), b"hello").unwrap();
        assert!(!is_comic_folder(&tmp.to_string_lossy()));

        // 加入图片 → true
        fs::write(tmp.join("001.jpg"), b"fake").unwrap();
        assert!(is_comic_folder(&tmp.to_string_lossy()));

        // 非目录 → false
        assert!(!is_comic_folder("nonexistent_path_12345"));

        let _ = fs::remove_dir_all(&tmp);
    }
}
