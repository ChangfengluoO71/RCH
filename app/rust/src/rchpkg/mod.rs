//! RCH 标准包格式（`.rchpkg`）——本地备份、云备份与设备间同步共用的版本化交换格式。
//!
//! 包 = zip：
//! ```text
//! manifest.json
//! chunks/tags.json
//! chunks/book_tags.json
//! chunks/metas.json
//! chunks/records.json
//! chunks/sources.json
//! chunks/settings.json
//! chunks/tombstones.json
//! ```
//!
//! 设计见 `.trellis/tasks/08-06-sync-p1-package/design.md`。
//! 敏感凭据（password / refresh_token / client_secret / cookie）永不进入包内；
//! sources 分块仅含非敏感字段，导入时目标端本地凭据不被覆盖。

use std::io::{Read, Seek, Write};
use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::db;

pub const FORMAT: &str = "rchpkg";
pub const SCHEMA_VERSION: i64 = 1;

const MANIFEST_PATH: &str = "manifest.json";
const CHUNKS: [&str; 7] = [
    "tags",
    "book_tags",
    "metas",
    "records",
    "sources",
    "settings",
    "tombstones",
];

/// 导出结果统计。
#[derive(Debug, Clone, Default)]
pub struct ExportInfo {
    pub device_id: String,
    pub created_at: i64,
    pub since: i64,
    pub tags: usize,
    pub book_tags: usize,
    pub metas: usize,
    pub records: usize,
    pub sources: usize,
    pub settings: usize,
    pub tombstones: usize,
}

/// 导入结果统计。
#[derive(Debug, Clone, Default)]
pub struct ImportStats {
    pub schema_version: i64,
    pub tags: usize,
    pub book_tags: usize,
    pub metas: usize,
    pub records: usize,
    pub sources: usize,
    pub settings: usize,
}

/// 同步目录约定：`<root>/RCH/sync`。
pub fn default_sync_dir(root: &Path) -> std::path::PathBuf {
    root.join("RCH").join("sync")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    format: String,
    schema_version: i64,
    device_id: String,
    device_name: String,
    created_at: i64,
    incremental: bool,
    since: i64,
    chunks: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TombstoneRow {
    entity: String,
    key: String,
    updated_at: i64,
}

/// 导出标准包到已打开的 zip writer（增量导出读 `cursor_export` 游标）。
pub fn export_package<W: Write + Seek>(
    conn: &Connection,
    zip: &mut ZipWriter<W>,
    incremental: bool,
) -> Result<ExportInfo> {
    let device_id = db::get_or_create_device_id_on(conn)?;
    let since = if incremental {
        db::get_sync_state_on(conn, "cursor_export")
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or(0)
    } else {
        0
    };
    let created_at = db::now_ms();

    let tags = db::load_tags_for_sync_on(conn, since);
    let book_tags = db::load_book_tags_for_sync_on(conn, since);
    let metas = db::load_metas_for_sync_on(conn, since);
    let records = db::load_records_for_sync_on(conn, since);
    let sources = db::load_sources_for_sync_on(conn, since);
    let settings = db::load_settings_for_sync_on(conn, since);

    let tombstones = build_tombstones(&tags, &book_tags, &metas, &records, &sources, &settings);

    let manifest = Manifest {
        format: FORMAT.to_string(),
        schema_version: SCHEMA_VERSION,
        device_id: device_id.clone(),
        device_name: default_device_name(),
        created_at,
        incremental,
        since,
        chunks: CHUNKS.iter().map(|s| s.to_string()).collect(),
    };

    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    write_json_entry(zip, MANIFEST_PATH, &manifest, options)?;
    write_json_entry(zip, "chunks/tags.json", &tags, options)?;
    write_json_entry(zip, "chunks/book_tags.json", &book_tags, options)?;
    write_json_entry(zip, "chunks/metas.json", &metas, options)?;
    write_json_entry(zip, "chunks/records.json", &records, options)?;
    write_json_entry(zip, "chunks/sources.json", &sources, options)?;
    write_json_entry(zip, "chunks/settings.json", &settings, options)?;
    write_json_entry(zip, "chunks/tombstones.json", &tombstones, options)?;

    // 导出成功后推进游标（备份即同步：后续增量只含本次之后变更）。
    db::set_sync_state_on(conn, "cursor_export", &created_at.to_string())?;

    Ok(ExportInfo {
        device_id,
        created_at,
        since,
        tags: tags.len(),
        book_tags: book_tags.len(),
        metas: metas.len(),
        records: records.len(),
        sources: sources.len(),
        settings: settings.len(),
        tombstones: tombstones.len(),
    })
}

/// 导出标准包到文件（供 FRB/P2 传输层调用）。
pub fn export_package_to_file(path: &str, incremental: bool) -> Result<ExportInfo> {
    let conn = db::get().lock().unwrap();
    let p = Path::new(path);
    if let Some(parent) = p.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let file = std::fs::File::create(p)?;
    let mut zip = ZipWriter::new(file);
    export_package(&conn, &mut zip, incremental)
}

/// 从打开的 zip archive 导入标准包（按行 upsert，保留目标端书源凭据）。
pub fn import_package<R: Read + Seek>(conn: &Connection, reader: R) -> Result<ImportStats> {
    let mut archive = ZipArchive::new(reader).context("无法打开 .rchpkg 包")?;
    let manifest: Manifest = read_json_entry(&mut archive, MANIFEST_PATH)?;
    if manifest.format != FORMAT {
        anyhow::bail!("不是 RCH 标准包（format={}）", manifest.format);
    }
    if manifest.schema_version > SCHEMA_VERSION {
        anyhow::bail!(
            "包 schema 版本 {} 高于当前支持版本 {}，请升级应用后再导入",
            manifest.schema_version,
            SCHEMA_VERSION
        );
    }

    let tags: Vec<db::TagSyncRow> = read_json_entry(&mut archive, "chunks/tags.json")?;
    let book_tags: Vec<db::BookTagSyncRow> = read_json_entry(&mut archive, "chunks/book_tags.json")?;
    let metas: Vec<db::MetaSyncRow> = read_json_entry(&mut archive, "chunks/metas.json")?;
    let records: Vec<db::RecordSyncRow> = read_json_entry(&mut archive, "chunks/records.json")?;
    let sources: Vec<db::SourceSyncRow> = read_json_entry(&mut archive, "chunks/sources.json")?;
    let settings: Vec<db::SettingSyncRow> = read_json_entry(&mut archive, "chunks/settings.json")?;

    for row in &tags {
        db::apply_tag_sync_on(conn, row)?;
    }
    for row in &book_tags {
        db::apply_book_tag_sync_on(conn, row)?;
    }
    for row in &metas {
        db::apply_meta_sync_on(conn, row)?;
    }
    for row in &records {
        db::apply_record_sync_on(conn, row)?;
    }
    for row in &sources {
        db::apply_source_sync_on(conn, row)?;
    }
    for row in &settings {
        db::apply_setting_sync_on(conn, row)?;
    }

    Ok(ImportStats {
        schema_version: manifest.schema_version,
        tags: tags.len(),
        book_tags: book_tags.len(),
        metas: metas.len(),
        records: records.len(),
        sources: sources.len(),
        settings: settings.len(),
    })
}

/// 从文件导入标准包（供 FRB/P2 传输层调用）。
pub fn import_package_from_file(path: &str) -> Result<ImportStats> {
    let conn = db::get().lock().unwrap();
    let file = std::fs::File::open(path).with_context(|| format!("无法打开 {path}"))?;
    import_package(&conn, file)
}

fn build_tombstones(
    tags: &[db::TagSyncRow],
    book_tags: &[db::BookTagSyncRow],
    metas: &[db::MetaSyncRow],
    records: &[db::RecordSyncRow],
    sources: &[db::SourceSyncRow],
    settings: &[db::SettingSyncRow],
) -> Vec<TombstoneRow> {
    let mut out = Vec::new();
    for r in tags.iter().filter(|r| r.deleted) {
        out.push(TombstoneRow { entity: "tags".into(), key: r.id.clone(), updated_at: r.updated_at });
    }
    for r in book_tags.iter().filter(|r| r.deleted) {
        out.push(TombstoneRow { entity: "book_tags".into(), key: format!("{}|{}", r.book_key, r.tag_id), updated_at: r.updated_at });
    }
    for r in metas.iter().filter(|r| r.deleted) {
        out.push(TombstoneRow { entity: "metas".into(), key: r.key.clone(), updated_at: r.updated_at });
    }
    for r in records.iter().filter(|r| r.deleted) {
        out.push(TombstoneRow { entity: "records".into(), key: r.key.clone(), updated_at: r.updated_at });
    }
    for r in sources.iter().filter(|r| r.deleted) {
        out.push(TombstoneRow { entity: "sources".into(), key: r.id.clone(), updated_at: r.updated_at });
    }
    for r in settings.iter().filter(|r| r.deleted) {
        out.push(TombstoneRow { entity: "settings".into(), key: r.key.clone(), updated_at: r.updated_at });
    }
    out
}

fn write_json_entry<W: Write + Seek, T: Serialize>(
    zip: &mut ZipWriter<W>,
    name: &str,
    value: &T,
    options: SimpleFileOptions,
) -> Result<()> {
    zip.start_file(name.to_string(), options)?;
    serde_json::to_writer(&mut *zip, value)?;
    Ok(())
}

fn read_json_entry<R: Read + Seek, T: serde::de::DeserializeOwned>(
    archive: &mut ZipArchive<R>,
    name: &str,
) -> Result<T> {
    let mut file = archive.by_name(name).with_context(|| format!("包内缺少 {name}"))?;
    let value = serde_json::from_reader(&mut file).with_context(|| format!("解析 {name} 失败"))?;
    Ok(value)
}

fn default_device_name() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "本机".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn schema_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::init_tables(&conn).unwrap();
        conn
    }

    fn seed(conn: &Connection) {
        // 直接写库模拟真实书源（含凭据），导出层必须剔除。
        conn.execute(
            "INSERT INTO book_sources (id, type, name, path, url, username, password, note, capability_label, fingerprint, updated_at, deleted)
             VALUES ('s1', 'webdav', 'NAS', '/books', 'https://dav.example.com', 'alice', 'secret-pass', '', 'webdav_range', 'fp1', 1000, 0)",
            [],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO tags (id, name, created_at, updated_at, deleted) VALUES ('日漫', '日漫', 100, 1000, 0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO book_tags (book_key, tag_id, updated_at, deleted) VALUES ('webdav|s1|/books/a.cbz', '日漫', 1000, 0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO book_metas (key, stable_id, title, summary, comment, rotations, updated_at, deleted)
             VALUES ('webdav|s1|/books/a.cbz', 'sid-a', 'A', '简介', '感想', '{}', 1000, 0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO read_records (key, stable_id, source_id, source_type, path, title, last_page, read_count, last_read_at, updated_at, deleted)
             VALUES ('webdav|s1|/books/a.cbz', 'sid-a', 's1', 'webdav', '/books/a.cbz', 'A', 12, 3, 2000, 1000, 0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO app_settings (key, value, updated_at, deleted) VALUES ('themeMode', 'dark', 1000, 0)",
            [],
        )
        .unwrap();
    }

    fn export_bytes(conn: &Connection, incremental: bool) -> Vec<u8> {
        let cursor = Cursor::new(Vec::new());
        let mut zip = ZipWriter::new(cursor);
        export_package(conn, &mut zip, incremental).unwrap();
        zip.finish().unwrap().into_inner()
    }

    #[test]
    fn round_trip_import_matches_export() {
        let a = schema_conn();
        seed(&a);
        let bytes = export_bytes(&a, false);

        let b = schema_conn();
        let stats = import_package(&b, Cursor::new(bytes)).unwrap();
        assert_eq!(stats.tags, 1);
        assert_eq!(stats.book_tags, 1);
        assert_eq!(stats.metas, 1);
        assert_eq!(stats.records, 1);
        assert_eq!(stats.sources, 1);
        assert_eq!(stats.settings, 1);

        let meta: (String, String, String) = b
            .query_row(
                "SELECT stable_id, summary, comment FROM book_metas WHERE key='webdav|s1|/books/a.cbz'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(meta, ("sid-a".into(), "简介".into(), "感想".into()));
    }

    #[test]
    fn package_contains_no_credentials() {
        let a = schema_conn();
        seed(&a);
        let bytes = export_bytes(&a, false);
        let mut archive = ZipArchive::new(Cursor::new(bytes)).unwrap();
        let sources: Vec<serde_json::Value> =
            read_json_entry(&mut archive, "chunks/sources.json").unwrap();
        assert_eq!(sources.len(), 1);
        let obj = sources[0].as_object().unwrap();
        for key in ["password", "refreshToken", "clientSecret", "cookie"] {
            assert!(!obj.contains_key(key), "sources 分块不应包含 {key}");
        }
    }

    #[test]
    fn import_preserves_local_source_credentials() {
        let a = schema_conn();
        seed(&a);
        let bytes = export_bytes(&a, false);

        let b = schema_conn();
        // 目标端已配置同 id 书源（含凭据）
        b.execute(
            "INSERT INTO book_sources (id, type, name, path, url, username, password, note, capability_label, updated_at, deleted)
             VALUES ('s1', 'webdav', 'NAS', '/books', 'https://dav.example.com', 'alice', 'local-pass', '', 'webdav_range', 500, 0)",
            [],
        )
        .unwrap();
        import_package(&b, Cursor::new(bytes)).unwrap();
        let pw: String = b
            .query_row("SELECT password FROM book_sources WHERE id='s1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(pw, "local-pass");
    }

    #[test]
    fn incremental_export_only_since_cursor() {
        let a = schema_conn();
        seed(&a);
        let mut cursor1 = Cursor::new(Vec::new());
        let mut zip1 = ZipWriter::new(&mut cursor1);
        let info1 = export_package(&a, &mut zip1, true).unwrap();
        zip1.finish().unwrap();
        assert_eq!(info1.since, 0);
        assert_eq!(info1.tags, 1); // since=0 全量

        // 第一次增量导出后游标已推进；新加一行再导出，只含新行。
        a.execute(
            "INSERT INTO tags (id, name, created_at, updated_at, deleted) VALUES ('新标签', '新标签', ?1, ?1, 0)",
            rusqlite::params![info1.created_at + 1],
        )
        .unwrap();
        let mut cursor2 = Cursor::new(Vec::new());
        let mut zip2 = ZipWriter::new(&mut cursor2);
        let info2 = export_package(&a, &mut zip2, true).unwrap();
        zip2.finish().unwrap();
        assert_eq!(info2.since, info1.created_at);
        assert_eq!(info2.tags, 1);
        let mut archive = ZipArchive::new(cursor2).unwrap();
        let tags2: Vec<db::TagSyncRow> = read_json_entry(&mut archive, "chunks/tags.json").unwrap();
        assert_eq!(tags2.len(), 1);
        assert_eq!(tags2[0].name, "新标签");
    }

    #[test]
    fn import_rejects_newer_schema() {
        let cursor = Cursor::new(Vec::new());
        let mut zip = ZipWriter::new(cursor);
        let options = SimpleFileOptions::default();
        let manifest = Manifest {
            format: FORMAT.into(),
            schema_version: SCHEMA_VERSION + 1,
            device_id: "dev_x".into(),
            device_name: "t".into(),
            created_at: 1,
            incremental: false,
            since: 0,
            chunks: CHUNKS.iter().map(|s| s.to_string()).collect(),
        };
        write_json_entry(&mut zip, MANIFEST_PATH, &manifest, options).unwrap();
        for c in CHUNKS {
            let empty: Vec<serde_json::Value> = vec![];
            write_json_entry(&mut zip, &format!("chunks/{c}.json"), &empty, options).unwrap();
        }
        let bytes = zip.finish().unwrap().into_inner();

        let conn = schema_conn();
        let err = import_package(&conn, Cursor::new(bytes)).unwrap_err();
        assert!(err.to_string().contains("高于当前支持版本"));
    }
}
