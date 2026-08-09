//! 同步层稳定身份（ADR-024 §4）。
//!
//! - source_fingerprint 由 `db::compute_source_fingerprint` 统一计算（db 层，供 upsert/回填/迁移）。
//! - 同步层 book_id = `sha256(fingerprint + "|" + normalized_path)`，与 `db::library_index_id` 同规则。
//! - 本地 SQLite key 保持 `type|source_id|path`（与 Dart `bookKeyOf` 同构），由本模块映射层转换，
//!   不做本地主键迁移；重命名/重建书源不破坏同步身份（fingerprint 稳定）。

use rusqlite::Connection;

use crate::db;

/// 同步层稳定 book_id（跨设备一致，不依赖本机 source_id）。
pub fn book_id(fingerprint: &str, path: &str) -> String {
    db::library_index_id(fingerprint, path)
}

/// 本地书 key（与 Dart `bookKeyOf` 同构：`type|source_id|path`）。
pub fn local_book_key(r#type: &str, source_id: &str, path: &str) -> String {
    format!("{}|{}|{}", r#type, source_id, path)
}

/// remote 稳定身份（fingerprint + path）→ 本地 key；本机无该 fingerprint 书源时返回 None。
pub fn resolve_local_book_key(conn: &Connection, fingerprint: &str, path: &str) -> Option<String> {
    let (id, r#type) = db::find_source_with_type_by_fingerprint_on(conn, fingerprint)?;
    Some(local_book_key(&r#type, &id, path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use rusqlite::Connection;

    fn schema_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        db::init_tables(&conn).unwrap();
        conn
    }

    fn insert_source(conn: &Connection, id: &str, r#type: &str, path: &str, url: Option<&str>) {
        let fp = db::compute_source_fingerprint(r#type, url, path, None);
        conn.execute(
            "INSERT INTO book_sources (id, type, name, path, url, fingerprint, updated_at, deleted)
             VALUES (?1, ?2, 's', ?3, ?4, ?5, 1, 0)",
            rusqlite::params![id, r#type, path, url, fp],
        )
        .unwrap();
    }

    #[test]
    fn book_id_is_stable_and_distinct() {
        let fp = db::compute_source_fingerprint(
            "webdav",
            Some("https://dav.example.com/dav"),
            "/books",
            None,
        );
        assert_eq!(
            book_id(&fp, "/books/a.cbz"),
            book_id(&fp, "/books/a.cbz")
        );
        assert_ne!(book_id(&fp, "/books/a.cbz"), book_id(&fp, "/books/b.cbz"));
        let fp2 = db::compute_source_fingerprint(
            "webdav",
            Some("https://dav.example.com/dav2"),
            "/books",
            None,
        );
        assert_ne!(book_id(&fp, "/books/a.cbz"), book_id(&fp2, "/books/a.cbz"));
    }

    #[test]
    fn resolve_local_book_key_maps_by_fingerprint() {
        let conn = schema_conn();
        insert_source(
            &conn,
            "s1",
            "webdav",
            "/books",
            Some("https://dav.example.com/dav"),
        );
        let fp = db::compute_source_fingerprint(
            "webdav",
            Some("https://dav.example.com/dav"),
            "/books",
            None,
        );
        assert_eq!(
            resolve_local_book_key(&conn, &fp, "/books/a.cbz"),
            Some("webdav|s1|/books/a.cbz".into())
        );
        // 本机没有该 fingerprint → None
        let fp2 = db::compute_source_fingerprint(
            "webdav",
            Some("https://other.example.com/dav"),
            "/books",
            None,
        );
        assert!(resolve_local_book_key(&conn, &fp2, "/books/a.cbz").is_none());
    }
}
