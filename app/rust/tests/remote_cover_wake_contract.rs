//! P1-B：source-level 唤醒的 session 解析契约。
//!
//! 目标（用户冻结语义）：调用方只提供**稳定的 source identity**，由 coordinator
//! 自己从持久状态解析出"真正能 claim 这些工作"的 runtime session token。
//! 解析不到就必须返回 `None` —— 不得伪造 session、不得远程访问，
//! pending 工作保持持久化等待后续 attach。

use rusqlite::{params, Connection};
use rust_lib_app::remote_scan::cover_model::{CoverJobKey, CoverJobState};
use rust_lib_app::remote_scan::cover_state::CoverJobUpsertCause;
use rust_lib_app::remote_scan::{cover_store, persistence};

const NOW: i64 = 1_000;

fn connection() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    // `persistence::migrate` 会在 `library_index` / `book_sources` 上加列，
    // 因此必须先建这两张基础表（与既有 remote_cover_store_contract 夹具一致）。
    conn.execute_batch(
        "CREATE TABLE book_sources(id TEXT PRIMARY KEY,type TEXT NOT NULL,fingerprint TEXT NOT NULL,path TEXT,root_id TEXT);
         CREATE TABLE library_index(
           id TEXT PRIMARY KEY,source_id TEXT,parent_id TEXT,name TEXT,path TEXT,entry_type TEXT,
           size INTEGER,modified_at INTEGER,asset_kind TEXT,content_fingerprint TEXT,
           scan_generation INTEGER,listing_complete INTEGER NOT NULL DEFAULT 0,
           deleted INTEGER NOT NULL DEFAULT 0,updated_at INTEGER);",
    )
    .unwrap();
    persistence::migrate(&conn).unwrap();
    cover_store::migrate(&conn).unwrap();
    conn
}

fn bind_epoch(
    conn: &Connection,
    source_id: &str,
    generation: i64,
    session_epoch: &str,
    token: i64,
) {
    conn.execute(
        "INSERT OR REPLACE INTO remote_scan_epoch(
             source_id,generation,source_fingerprint,root_path,session_epoch,session_token)
         VALUES(?1,?2,'fp','/',?3,?4)",
        params![source_id, generation, session_epoch, token],
    )
    .unwrap();
}

fn job_key(source_id: &str, asset_id: &str) -> CoverJobKey {
    CoverJobKey {
        source_id: source_id.into(),
        asset_id: asset_id.into(),
        content_revision: "v1".into(),
        selection_revision: "default".into(),
        profile: "340x480@1".into(),
    }
}

/// 落一条 job；`state` 直接按请求状态建立（新记录路径）。
fn seed_job(
    conn: &Connection,
    key: &CoverJobKey,
    state: CoverJobState,
    generation: i64,
    session_epoch: &str,
) {
    cover_store::upsert_job_on(
        conn,
        key,
        state,
        "background",
        10,
        generation,
        session_epoch,
        NOW,
        CoverJobUpsertCause::Demand,
    )
    .unwrap();
}

fn set_next_attempt(conn: &Connection, key: &CoverJobKey, at: i64) {
    conn.execute(
        "UPDATE remote_cover_job SET next_attempt_at=?1 WHERE job_key=?2",
        params![at, key.encode()],
    )
    .unwrap();
}

#[test]
fn wake_session_resolution_matrix() {
    // (说明, source, generation, session_epoch, token 绑定, job 状态, 期望)
    struct Row {
        label: &'static str,
        session_epoch: &'static str,
        bind: Option<(&'static str, i64)>, // (session_epoch, token)
        state: Option<CoverJobState>,
        next_attempt: Option<i64>,
        expected: Option<u64>,
    }
    let rows = [
        Row {
            label: "pending + 匹配 epoch(token=42)",
            session_epoch: "e1",
            bind: Some(("e1", 42)),
            state: Some(CoverJobState::Pending),
            next_attempt: None,
            expected: Some(42),
        },
        Row {
            label: "pending + epoch token=0（无有效 session）",
            session_epoch: "e1",
            bind: Some(("e1", 0)),
            state: Some(CoverJobState::Pending),
            next_attempt: None,
            expected: None,
        },
        Row {
            label: "pending + 完全没有 epoch 行",
            session_epoch: "e1",
            bind: None,
            state: Some(CoverJobState::Pending),
            next_attempt: None,
            expected: None,
        },
        Row {
            label: "pending + epoch 的 session_epoch 不匹配",
            session_epoch: "e1",
            bind: Some(("other-epoch", 42)),
            state: Some(CoverJobState::Pending),
            next_attempt: None,
            expected: None,
        },
        Row {
            label: "ready（无待办）",
            session_epoch: "e1",
            bind: Some(("e1", 42)),
            state: Some(CoverJobState::Ready),
            next_attempt: None,
            expected: None,
        },
        Row {
            label: "retry_wait 未到期",
            session_epoch: "e1",
            bind: Some(("e1", 42)),
            state: Some(CoverJobState::RetryWait),
            next_attempt: Some(NOW + 60_000),
            expected: None,
        },
        Row {
            label: "retry_wait 已到期",
            session_epoch: "e1",
            bind: Some(("e1", 42)),
            state: Some(CoverJobState::RetryWait),
            next_attempt: Some(NOW - 1),
            expected: Some(42),
        },
        Row {
            label: "完全没有任何 job 行",
            session_epoch: "e1",
            bind: Some(("e1", 42)),
            state: None,
            next_attempt: None,
            expected: None,
        },
    ];

    let mut failures = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let conn = connection();
        let source_id = format!("source-{index}");
        if let Some((epoch, token)) = row.bind {
            bind_epoch(&conn, &source_id, 1, epoch, token);
        }
        if let Some(state) = row.state {
            let key = job_key(&source_id, "asset");
            seed_job(&conn, &key, state, 1, row.session_epoch);
            if let Some(at) = row.next_attempt {
                set_next_attempt(&conn, &key, at);
            }
        }
        let resolved = cover_store::resolve_cover_wake_session(&conn, &source_id, NOW).unwrap();
        if resolved != row.expected {
            failures.push(format!(
                "{}: got {:?} want {:?}",
                row.label, resolved, row.expected
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "wake session resolution violated in {} of {} rows:\n  {}",
        failures.len(),
        rows.len(),
        failures.join("\n  ")
    );
}

/// 跨 source 不得串：A 有待办但没有自己的 epoch，B 有 epoch —— A 必须解析不到。
#[test]
fn wake_session_never_borrows_another_sources_session() {
    let conn = connection();
    bind_epoch(&conn, "source-b", 1, "e1", 42);
    seed_job(
        &conn,
        &job_key("source-a", "asset"),
        CoverJobState::Pending,
        1,
        "e1",
    );
    seed_job(
        &conn,
        &job_key("source-b", "asset"),
        CoverJobState::Pending,
        1,
        "e1",
    );

    assert_eq!(
        cover_store::resolve_cover_wake_session(&conn, "source-a", NOW).unwrap(),
        None,
        "source-a must not borrow source-b's session token"
    );
    assert_eq!(
        cover_store::resolve_cover_wake_session(&conn, "source-b", NOW).unwrap(),
        Some(42)
    );
}

/// 同一 source 多个 generation：必须能解析出**能 claim 待办**的那个 token，
/// 而不是 blindly 取最新 generation。
#[test]
fn wake_session_targets_the_epoch_that_owns_the_pending_work() {
    let conn = connection();
    // gen 1 是旧代际（token 7），gen 2 是最新代际（token 9）；待办挂在 gen 1。
    bind_epoch(&conn, "source", 1, "e1", 7);
    bind_epoch(&conn, "source", 2, "e2", 9);
    seed_job(
        &conn,
        &job_key("source", "asset"),
        CoverJobState::Pending,
        1,
        "e1",
    );

    assert_eq!(
        cover_store::resolve_cover_wake_session(&conn, "source", NOW).unwrap(),
        Some(7),
        "must resolve the token whose epoch actually owns the pending job"
    );
}
