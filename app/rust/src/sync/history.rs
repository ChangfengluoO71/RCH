//! 同步历史可观测性（P1-9）。
//!
//! 每次同步记录一条：起止时间、revision 前后、pull/push/merge/conflict 计数、
//! 错误与实体变更摘要（JSON）。用户可据此排查"为什么没同步"。

use anyhow::Result;
use rusqlite::{params, Connection};

use crate::db;

#[derive(Debug, Clone)]
pub struct SyncHistoryRow {
    pub id: i64,
    pub start_time: i64,
    pub end_time: i64,
    pub revision_before: i64,
    pub revision_after: i64,
    pub pull_count: i64,
    pub push_count: i64,
    pub merge_count: i64,
    pub conflict_count: i64,
    pub error: String,
    pub summary: String,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn record_on(
    conn: &Connection,
    start_time: i64,
    end_time: i64,
    revision_before: i64,
    revision_after: i64,
    pull_count: i64,
    push_count: i64,
    merge_count: i64,
    conflict_count: i64,
    error: &str,
    changed: &[(String, usize)],
) -> Result<()> {
    let mut map = serde_json::Map::new();
    for (e, n) in changed {
        map.insert(e.clone(), serde_json::json!(n));
    }
    let summary = serde_json::Value::Object(map).to_string();
    conn.execute(
        "INSERT INTO sync_history
         (start_time, end_time, revision_before, revision_after, pull_count, push_count, merge_count, conflict_count, error, summary)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            start_time,
            end_time,
            revision_before,
            revision_after,
            pull_count,
            push_count,
            merge_count,
            conflict_count,
            error,
            summary,
        ],
    )?;
    Ok(())
}

pub fn record(
    start_time: i64,
    end_time: i64,
    revision_before: i64,
    revision_after: i64,
    pull_count: i64,
    push_count: i64,
    merge_count: i64,
    conflict_count: i64,
    error: &str,
    changed: &[(String, usize)],
) -> Result<()> {
    let conn = db::get().lock().unwrap();
    record_on(
        &conn,
        start_time,
        end_time,
        revision_before,
        revision_after,
        pull_count,
        push_count,
        merge_count,
        conflict_count,
        error,
        changed,
    )
}

/// 最近 N 条历史（倒序）。
pub fn recent(limit: i64) -> Vec<SyncHistoryRow> {
    let conn = db::get().lock().unwrap();
    recent_on(&conn, limit)
}

pub(crate) fn recent_on(conn: &Connection, limit: i64) -> Vec<SyncHistoryRow> {
    let mut stmt = conn
        .prepare(
            "SELECT id, start_time, end_time, revision_before, revision_after,
                    pull_count, push_count, merge_count, conflict_count, error, summary
             FROM sync_history ORDER BY id DESC LIMIT ?1",
        )
        .unwrap();
    stmt.query_map([limit], |r| {
        Ok(SyncHistoryRow {
            id: r.get(0)?,
            start_time: r.get(1)?,
            end_time: r.get(2)?,
            revision_before: r.get(3)?,
            revision_after: r.get(4)?,
            pull_count: r.get(5)?,
            push_count: r.get(6)?,
            merge_count: r.get(7)?,
            conflict_count: r.get(8)?,
            error: r.get(9)?,
            summary: r.get(10)?,
        })
    })
    .unwrap()
    .filter_map(|r| r.ok())
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn schema_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        db::init_tables(&conn).unwrap();
        conn
    }

    #[test]
    fn history_records_and_lists_desc() {
        let conn = schema_conn();
        record_on(&conn, 1, 2, 0, 5, 3, 12, 1, 0, "", &[("metas".into(), 12)]).unwrap();
        record_on(&conn, 3, 4, 5, 5, 0, 0, 0, 0, "网络中断", &[]).unwrap();
        let rows = recent_on(&conn, 10);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].error, "网络中断");
        assert_eq!(rows[0].revision_before, 5);
        assert!(rows[1].summary.contains("metas"));
        assert!(rows[1].summary.contains("12"));
    }
}
