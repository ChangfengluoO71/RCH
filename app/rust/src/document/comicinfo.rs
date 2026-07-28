//! ComicInfo.xml 解析（Folder 格式元数据源）。
//!
//! 按 Anansi/ComicRack ComicInfo.xml schema 解析常见字段，
//! 映射到 DocumentMeta。

use super::DocumentMeta;
use anyhow::Result;
use serde::Deserialize;
use std::path::Path;

/// ComicInfo.xml 中我们关心的字段。
/// 使用 quick-xml + serde 反序列化。
#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct ComicInfo {
    pub title: Option<String>,
    pub series: Option<String>,
    pub writer: Option<String>,
    pub genre: Option<String>,
    pub summary: Option<String>,
    pub publisher: Option<String>,
    pub language_iso: Option<String>,
    pub number: Option<String>,
    pub count: Option<String>,
    pub volume: Option<String>,
    pub year: Option<String>,
    pub month: Option<String>,
    pub day: Option<String>,
    /// 分号分隔的标签字符串（非标准扩展，部分工具支持）。
    #[serde(rename = "Tags")]
    pub tags: Option<String>,
}

/// 从 ComicInfo.xml 文件解析。
pub fn read_comicinfo<P: AsRef<Path>>(path: P) -> Result<ComicInfo> {
    let content = std::fs::read_to_string(path.as_ref())?;
    let info: ComicInfo = quick_xml::de::from_str(&content)?;
    Ok(info)
}

/// 将 ComicInfo 映射到 DocumentMeta。
/// 优先 ComicInfo 字段，空白时保留为空字符串。
pub fn comicinfo_to_meta(info: &ComicInfo) -> DocumentMeta {
    DocumentMeta {
        title: info.title.clone().unwrap_or_default(),
        author: info.writer.clone().unwrap_or_default(),
        genre: info.genre.clone().unwrap_or_default(),
        series: info.series.clone().unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn parse_basic_comicinfo() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<ComicInfo xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"
           xmlns:xsd="http://www.w3.org/2001/XMLSchema">
  <Title>火影忍者</Title>
  <Series>Naruto</Series>
  <Writer>岸本齐史</Writer>
  <Genre>少年漫画</Genre>
  <Summary>一个关于忍者的故事。</Summary>
</ComicInfo>"#;
        let info: ComicInfo = quick_xml::de::from_str(xml).unwrap();
        assert_eq!(info.title.as_deref(), Some("火影忍者"));
        assert_eq!(info.series.as_deref(), Some("Naruto"));
        assert_eq!(info.writer.as_deref(), Some("岸本齐史"));
        assert_eq!(info.genre.as_deref(), Some("少年漫画"));
        assert_eq!(info.summary.as_deref(), Some("一个关于忍者的故事。"));
    }

    #[test]
    fn map_to_document_meta() {
        let info = ComicInfo {
            title: Some("测试标题".into()),
            writer: Some("测试作者".into()),
            genre: Some("测试类型".into()),
            series: Some("测试系列".into()),
            ..Default::default()
        };
        let meta = comicinfo_to_meta(&info);
        assert_eq!(meta.title, "测试标题");
        assert_eq!(meta.author, "测试作者");
        assert_eq!(meta.genre, "测试类型");
        assert_eq!(meta.series, "测试系列");
    }

    #[test]
    fn empty_comicinfo_yields_default_meta() {
        let xml = r#"<?xml version="1.0"?>
<ComicInfo>
</ComicInfo>"#;
        let info: ComicInfo = quick_xml::de::from_str(xml).unwrap();
        let meta = comicinfo_to_meta(&info);
        assert_eq!(meta.title, "");
        assert_eq!(meta.author, "");
        assert_eq!(meta.genre, "");
        assert_eq!(meta.series, "");
    }

    #[test]
    fn read_from_temp_file() {
        let xml = r#"<?xml version="1.0"?>
<ComicInfo>
  <Title>测试</Title>
</ComicInfo>"#;
        let tmp = std::env::temp_dir().join("rch_test_comicinfo.xml");
        {
            let mut f = std::fs::File::create(&tmp).unwrap();
            f.write_all(xml.as_bytes()).unwrap();
        }
        let info = read_comicinfo(&tmp).unwrap();
        assert_eq!(info.title.as_deref(), Some("测试"));
        let _ = std::fs::remove_file(&tmp);
    }
}
