//! EPUB(漫画)格式解析。
//!
//! 漫画 EPUB 本质是 ZIP + OPF(spine 定义阅读顺序) + 图片文件。
//! 实现:打开 ZIP → 解析 container.xml 找到 OPF 路径 → 解析 OPF 的 manifest + spine →
//! 按 spine 顺序获取每页对应的图片字节。
//! 不依赖排版引擎(漫画 EPUB 不需要 HTML 排版)。

use super::{Document, DocumentMeta};
use crate::source::ByteSource;
use anyhow::{anyhow, Context, Result};
use std::collections::HashMap;
use std::io::Read;

/// EPUB 书籍。
pub struct EpubBook<S: ByteSource> {
    src: S,
    /// 按阅读顺序排列的图片在 ZIP 中的路径 + 它们在 ZIP 中的偏移/大小。
    page_entries: Vec<ZipEntryMeta>,
    title: String,
}

#[derive(Clone)]
struct ZipEntryMeta {
    path: String,
    data_start: u64,
    compressed_size: u64,
    deflated: bool,
}

impl<S: ByteSource> EpubBook<S> {
    pub fn open(src: S, path: &str) -> Result<Self> {
        // 先解析 ZIP 中心目录,收集所有需要的元数据
        let reader = crate::source::SourceReader::new(src);
        let mut zip = zip::ZipArchive::new(reader).context("打开 EPUB(ZIP)失败")?;

        // 1. 解析 container.xml
        let opf_path = find_opf_path(&mut zip)?;

        // 2. 解析 OPF
        let opf_xml = read_zip_entry(&mut zip, &opf_path)
            .with_context(|| format!("读取 OPF 失败: {opf_path}"))?;
        let (manifest, spine) = parse_opf(&opf_xml)?;

        // 3. 收集图片路径
        let opf_dir = opf_path
            .rsplit_once('/')
            .map(|(d, _)| format!("{}/", d))
            .unwrap_or_default();

        let mut image_paths = Vec::new();
        for idref in &spine {
            if let Some(href) = manifest.get(idref) {
                let full = resolve_path(&opf_dir, href);
                let lower = full.to_lowercase();
                if lower.ends_with(".xhtml") || lower.ends_with(".html") || lower.ends_with(".htm") {
                    if let Ok(html) = read_zip_entry(&mut zip, &full) {
                        if let Some(img_path) = extract_img_src(&html) {
                            let img_full = resolve_path(&opf_dir, &img_path);
                            if is_image_ext(&img_full) {
                                image_paths.push(img_full);
                            }
                        }
                    }
                } else if is_image_ext(&full) {
                    image_paths.push(full);
                }
            }
        }

        // 4. 退化:扫描 ZIP 中所有图片
        if image_paths.is_empty() {
            let mut all_images = Vec::new();
            for i in 0..zip.len() {
                if let Ok(f) = zip.by_index(i) {
                    let name = f.name().to_string();
                    if is_image_ext(&name) && !name.contains("__MACOSX") && !name.ends_with(".DS_Store") {
                        all_images.push(name);
                    }
                }
            }
            all_images.sort_by(|a, b| crate::util::natural_cmp(a, b));
            image_paths = all_images;
        }

        if image_paths.is_empty() {
            return Err(anyhow!("EPUB 中没有找到图片"));
        }

        // 5. 保存每页的偏移/大小信息
        let mut page_entries = Vec::new();
        for img_path in &image_paths {
            let idx = index_for_name_ignore_case(&mut zip, img_path)
                .with_context(|| format!("EPUB 中找不到图片: {img_path}"))?;
            let f = zip.by_index(idx)?;
            page_entries.push(ZipEntryMeta {
                path: img_path.clone(),
                data_start: f.data_start(),
                compressed_size: f.compressed_size(),
                deflated: matches!(f.compression(), zip::CompressionMethod::Deflated),
            });
        }

        // 收回原始字节源
        let src = zip.into_inner().into_inner();

        let title = std::path::Path::new(path)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string());

        Ok(EpubBook { src, page_entries, title })
    }
}

impl<S: ByteSource> Document for EpubBook<S> {
    fn page_count(&self) -> u32 {
        self.page_entries.len() as u32
    }

    fn metadata(&self) -> DocumentMeta {
        DocumentMeta {
            title: self.title.clone(),
            ..Default::default()
        }
    }

    fn page_bytes(&self, index: u32) -> Result<Vec<u8>> {
        let entry = self
            .page_entries
            .get(index as usize)
            .with_context(|| format!("页索引越界: {index}"))?;
        let mut buf = vec![0u8; entry.compressed_size as usize];
        self.src
            .read_exact_at(entry.data_start, &mut buf)
            .context("下载页数据失败")?;
        if entry.deflated {
            let mut out = Vec::new();
            flate2::read::DeflateDecoder::new(&buf[..])
                .read_to_end(&mut out)
                .context("Deflate 解压失败")?;
            Ok(out)
        } else {
            Ok(buf)
        }
    }
}

// ---------- helpers ----------

fn is_image_ext(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.ends_with(".jpg")
        || lower.ends_with(".jpeg")
        || lower.ends_with(".png")
        || lower.ends_with(".webp")
        || lower.ends_with(".gif")
        || lower.ends_with(".bmp")
        || lower.ends_with(".avif")
}

fn index_for_name_ignore_case<R: Read + std::io::Seek>(
    zip: &mut zip::ZipArchive<R>,
    name: &str,
) -> Option<usize> {
    if let Some(idx) = zip.index_for_name(name) {
        return Some(idx);
    }
    for i in 0..zip.len() {
        if let Ok(f) = zip.by_index(i) {
            if f.name().eq_ignore_ascii_case(name) {
                return Some(i);
            }
        }
    }
    None
}

fn find_opf_path<R: Read + std::io::Seek>(
    zip: &mut zip::ZipArchive<R>,
) -> Result<String> {
    let xml = read_zip_entry(zip, "META-INF/container.xml")
        .context("EPUB 缺少 META-INF/container.xml")?;
    let s = String::from_utf8_lossy(&xml);
    let mut pos = 0;
    while pos < s.len() {
        if let Some(start) = s[pos..].find("full-path") {
            let abs = pos + start;
            if let Some(q_start) = s[abs..].find('"') {
                let after_quote = abs + q_start + 1;
                if let Some(q_end) = s[after_quote..].find('"') {
                    return Ok(s[after_quote..after_quote + q_end].to_string());
                }
            }
            pos = abs + 1;
        } else {
            break;
        }
    }
    Err(anyhow!("container.xml 中未找到 rootfile full-path"))
}

fn parse_opf(xml: &[u8]) -> Result<(HashMap<String, String>, Vec<String>)> {
    let s = String::from_utf8_lossy(xml);
    let mut manifest = HashMap::new();
    let mut spine = Vec::new();
    let mut in_manifest = false;
    let mut in_spine = false;

    let mut pos = 0;
    let bytes = s.as_bytes();
    while pos < bytes.len() {
        if bytes[pos] != b'<' {
            pos += 1;
            continue;
        }
        let tag_end = match bytes[pos..].iter().position(|&b| b == b'>') {
            Some(i) => pos + i + 1,
            None => break,
        };
        let tag = &s[pos..tag_end];

        if tag.starts_with("<manifest") {
            in_manifest = true;
        } else if tag.starts_with("</manifest") {
            in_manifest = false;
        } else if tag.starts_with("<spine") {
            in_spine = true;
        } else if tag.starts_with("</spine") {
            in_spine = false;
        } else if in_manifest && tag.starts_with("<item") {
            let id = extract_attr(tag, "id");
            let href = extract_attr(tag, "href");
            if let (Some(i), Some(h)) = (id, href) {
                manifest.insert(i, h);
            }
        } else if in_spine && tag.starts_with("<itemref") {
            if let Some(idref) = extract_attr(tag, "idref") {
                spine.push(idref);
            }
        }
        pos = tag_end;
    }

    if manifest.is_empty() || spine.is_empty() {
        let mut imgs: Vec<_> = manifest
            .values()
            .filter(|h| is_image_ext(h))
            .cloned()
            .collect();
        imgs.sort_by(|a, b| crate::util::natural_cmp(a, b));
        return Ok((manifest, imgs));
    }

    Ok((manifest, spine))
}

fn extract_attr(tag: &str, attr: &str) -> Option<String> {
    let pat = format!("{}=\"", attr);
    let start = tag.find(&pat)? + pat.len();
    let rest = &tag[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

fn extract_img_src(html: &[u8]) -> Option<String> {
    let s = String::from_utf8_lossy(html);
    let lower = s.to_lowercase();
    let img_start = lower.find("<img")?;
    let tag_slice = &s[img_start..];
    let close = tag_slice.find('>')?;
    let img_tag = &tag_slice[..=close];
    extract_attr(img_tag, "src")
}

fn resolve_path(base: &str, path: &str) -> String {
    if path.starts_with('/') || path.starts_with("http") {
        return path.to_string();
    }
    let mut parts: Vec<&str> = base.split('/').filter(|s| !s.is_empty()).collect();
    for seg in path.split('/') {
        match seg {
            "." => {}
            ".." => {
                parts.pop();
            }
            _ => parts.push(seg),
        }
    }
    parts.join("/")
}

fn read_zip_entry<R: Read + std::io::Seek>(
    zip: &mut zip::ZipArchive<R>,
    path: &str,
) -> Result<Vec<u8>> {
    let idx = index_for_name_ignore_case(zip, path)
        .with_context(|| format!("ZIP 中找不到: {path}"))?;
    let mut f = zip.by_index(idx)?;
    let mut buf = Vec::with_capacity(f.size() as usize);
    f.read_to_end(&mut buf)?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_path() {
        assert_eq!(
            resolve_path("OEBPS/", "images/001.jpg"),
            "OEBPS/images/001.jpg"
        );
        assert_eq!(
            resolve_path("OEBPS/", "../META-INF/container.xml"),
            "META-INF/container.xml"
        );
        assert_eq!(
            resolve_path("OEBPS/", "/absolute/path.jpg"),
            "/absolute/path.jpg"
        );
    }

    #[test]
    fn test_extract_attr() {
        let tag = r#"<item id="cover" href="images/cover.jpg" media-type="image/jpeg"/>"#;
        assert_eq!(extract_attr(tag, "id"), Some("cover".to_string()));
        assert_eq!(
            extract_attr(tag, "href"),
            Some("images/cover.jpg".to_string())
        );
    }

    #[test]
    fn test_extract_img_src() {
        let html = br#"<html><body><img src="page001.jpg" alt="page"/></body></html>"#;
        assert_eq!(extract_img_src(html), Some("page001.jpg".to_string()));
    }
}
