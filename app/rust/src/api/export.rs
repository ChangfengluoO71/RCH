//! 本地漫画导出 CBZ。
use std::fs;
use std::io::Write;
use std::path::Path;

use zip::write::SimpleFileOptions;

const IMAGE_EXTS: &[&str] = &["jpg", "jpeg", "png", "webp", "gif", "bmp", "avif"];

fn is_image_name(name: &str) -> bool {
    let lower = name.to_lowercase();
    if lower.starts_with('.') || lower.contains("__macosx") || lower.ends_with(".ds_store") {
        return false;
    }
    lower
        .rsplit('.')
        .next()
        .map(|ext| IMAGE_EXTS.contains(&ext))
        .unwrap_or(false)
}

/// 文件夹 → CBZ：按自然排序打包顶层图片，附带 ComicInfo.xml / metadata.json。
pub fn export_folder_to_cbz(src_dir: String, out_path: String) -> Result<(), String> {
    export_folder_to_cbz_impl(&src_dir, &out_path).map_err(|e| format!("{e}"))
}

fn export_folder_to_cbz_impl(src_dir: &str, out_path: &str) -> anyhow::Result<()> {
    let p = Path::new(src_dir);
    if !p.is_dir() {
        anyhow::bail!("不是目录: {src_dir}");
    }

    let mut images = Vec::new();
    for entry in fs::read_dir(p)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if entry.file_type()?.is_dir() {
            continue;
        }
        if is_image_name(&name) {
            images.push(name);
        }
    }
    if images.is_empty() {
        anyhow::bail!("目录下没有图片文件: {src_dir}");
    }
    images.sort_by(|a, b| crate::util::natural_cmp(a, b));

    let out = Path::new(out_path);
    if let Some(parent) = out.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    let file = fs::File::create(out)?;
    let mut zip = zip::ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    for name in &images {
        zip.start_file(name.clone(), options)?;
        zip.write_all(&fs::read(p.join(name))?)?;
    }
    for meta_name in ["ComicInfo.xml", "metadata.json"] {
        let mp = p.join(meta_name);
        if mp.exists() {
            zip.start_file(meta_name.to_string(), options)?;
            zip.write_all(&fs::read(mp)?)?;
        }
    }
    zip.finish()?;
    Ok(())
}

/// ZIP/CBZ → CBZ：同容器，直接复制字节即可。
pub fn export_zip_as_cbz(src_path: String, out_path: String) -> Result<(), String> {
    fs::copy(&src_path, &out_path)
        .map(|_| ())
        .map_err(|e| format!("{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn folder_export_orders_pages_naturally() {
        let tmp = std::env::temp_dir().join("rch_export_test");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        for (name, data) in [
            ("page10.jpg", b"ten" as &[u8]),
            ("page2.jpg", b"two"),
            ("page1.jpg", b"one"),
        ] {
            fs::write(tmp.join(name), data).unwrap();
        }
        fs::write(tmp.join("ComicInfo.xml"), b"<ComicInfo/>").unwrap();

        let out = tmp.join("out.cbz");
        export_folder_to_cbz(
            tmp.to_string_lossy().into_owned(),
            out.to_string_lossy().into_owned(),
        )
        .unwrap();

        let f = fs::File::open(&out).unwrap();
        let mut zip = zip::ZipArchive::new(f).unwrap();
        let names: Vec<String> = (0..zip.len())
            .map(|i| zip.by_index(i).unwrap().name().to_string())
            .collect();
        assert_eq!(names, vec!["page1.jpg", "page2.jpg", "page10.jpg", "ComicInfo.xml"]);

        // 内容可读回
        let mut first = String::new();
        zip.by_name("page1.jpg").unwrap().read_to_string(&mut first).unwrap();
        assert_eq!(first, "one");

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn folder_export_rejects_empty_dir() {
        let tmp = std::env::temp_dir().join("rch_export_empty");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        let out = tmp.join("out.cbz");
        assert!(export_folder_to_cbz(
            tmp.to_string_lossy().into_owned(),
            out.to_string_lossy().into_owned()
        )
        .is_err());
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn zip_copy_export_works() {
        let tmp = std::env::temp_dir().join("rch_export_copy");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        let src = tmp.join("a.zip");
        fs::write(&src, b"zip-bytes").unwrap();
        let out = tmp.join("a.cbz");
        export_zip_as_cbz(
            src.to_string_lossy().into_owned(),
            out.to_string_lossy().into_owned(),
        )
        .unwrap();
        assert_eq!(fs::read(&out).unwrap(), b"zip-bytes");
        let _ = fs::remove_dir_all(&tmp);
    }
}
