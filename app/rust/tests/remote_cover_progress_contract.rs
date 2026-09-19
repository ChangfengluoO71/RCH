//! P1-F：cover 进度**语义字段**与**不变量**契约。
//!
//! 通过公开 API `remote_scan_status` 驱动（`refresh_status_counts` 为私有实现细节）。
//!
//! 冻结语义：
//! * `available_books` = `ready` **且字节真的可用**（缓存/文件系统校验；锁外计算）；
//! * `stale_ready = ready_books − available_books` ⇒ 计入 `waiting_books`；
//! * `no_job = discovered_books − trackedDistinctAssets`（按 **asset** 去重）；
//! * `waiting_books = pending + retry + stale_ready + no_job`；
//! * `other_books` 只计真实未知 durable state（默认 0，不用于修补数学差额）；
//! * 不变量：`available + waiting + active + failed + unsupported + blocked + other
//!   == discovered_books`。

use rusqlite::{params, Connection};
use rust_lib_app::api::remote_scan::remote_scan_status;
use rust_lib_app::remote_scan::cover_model::{CoverJobKey, CoverJobState};
use rust_lib_app::remote_scan::cover_state::CoverJobUpsertCause;
use rust_lib_app::remote_scan::{cover_store, persistence};
use rust_lib_app::{cache, db};

const SELECTION: &str = "page:2|crop:|asset:";
const PROFILE: &str = "170x240@1";
const GENERATION: i64 = 1;

struct Guard;
impl Drop for Guard {
    fn drop(&mut self) {
        cache::set_custom_cache_root("");
    }
}

fn key(source: &str, asset: &str) -> CoverJobKey {
    CoverJobKey {
        source_id: source.into(),
        asset_id: asset.into(),
        content_revision: "content".into(),
        selection_revision: SELECTION.into(),
        profile: PROFILE.into(),
    }
}

/// 全局 DB + 自定义 cache root；`indexed` 为本 generation 的 eligible 漫画数。
fn prepare(source: &str, indexed: usize) -> Guard {
    let root = std::env::temp_dir().join(format!("rch_f_prog_{source}"));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    cache::set_custom_cache_root(root.to_str().unwrap());

    let conn = db::get().lock().unwrap();
    persistence::migrate(&conn).unwrap();
    cover_store::migrate(&conn).unwrap();
    for table in [
        "remote_cover_job",
        "remote_cover_variant",
        "remote_scan_epoch",
        "remote_view_revision",
        "remote_scan_state",
        "library_index",
    ] {
        conn.execute(&format!("DELETE FROM {table} WHERE source_id=?1"), [source])
            .unwrap();
    }
    conn.execute(
        "INSERT OR REPLACE INTO remote_scan_state(source_id,status,mode,generation,checkpoint,last_success_at,error_code)
         VALUES(?1,'Succeeded','Full',?2,NULL,NULL,NULL)",
        params![source, GENERATION],
    )
    .unwrap();
    conn.execute(
        "INSERT OR REPLACE INTO remote_scan_epoch(
             source_id,generation,source_fingerprint,root_path,session_epoch,session_token)
         VALUES(?1,?2,'fp','/','e1',42)",
        params![source, GENERATION],
    )
    .unwrap();
    for i in 0..indexed {
        conn.execute(
            "INSERT OR REPLACE INTO library_index(
                 id,source_id,parent_id,name,path,entry_type,asset_kind,content_fingerprint,
                 scan_generation,listing_complete,deleted,updated_at)
             VALUES(?1,?2,'/root',?3,?4,'file','ArchiveFile','cf',?5,1,0,1)",
            params![
                format!("{source}-idx-{i}"),
                source,
                format!("book{i}.cbz"),
                format!("/book{i}.cbz"),
                GENERATION
            ],
        )
        .unwrap();
    }
    drop(conn);
    Guard
}

fn seed_job(source: &str, asset: &str, state: CoverJobState) {
    let conn = db::get().lock().unwrap();
    cover_store::upsert_job_on(
        &conn,
        &key(source, asset),
        state,
        "background",
        10,
        GENERATION,
        "e1",
        0,
        CoverJobUpsertCause::Demand,
    )
    .unwrap();
}

fn write_material(source: &str, asset: &str) {
    cache::remote_cover_cache_write(
        source,
        asset,
        "content",
        SELECTION,
        PROFILE,
        4,
        4,
        &vec![5_u8; 4 * 4 * 4],
    )
    .unwrap();
}

fn status_of(source: &str) -> rust_lib_app::api::remote_scan::RemoteScanStatusDto {
    remote_scan_status(source.to_string()).expect("status must exist for a persisted source")
}

fn assert_invariant(s: &rust_lib_app::api::remote_scan::RemoteScanStatusDto, what: &str) {
    let sum = s.available_books
        + s.waiting_books
        + s.active_books
        + s.failed_books
        + s.unsupported_books
        + s.blocked_books
        + s.other_books;
    assert_eq!(
        sum, s.discovered_books,
        "{what}: available+waiting+active+failed+unsupported+blocked+other must equal discovered"
    );
}

// --------------------------------------------------- F-1 / F-2：available 与 stale-ready
#[test]
fn f_availability_and_stale_ready_split_the_ready_bucket() {
    let source = "f-prog-ready";
    let _guard = prepare(source, 2);
    // A：ready 且字节可用；B：ready 但字节缺失（stale-ready）
    seed_job(source, "a", CoverJobState::Ready);
    seed_job(source, "b", CoverJobState::Ready);
    write_material(source, "a");

    let s = status_of(source);
    assert_eq!(s.ready_books, 2, "raw ready count stays truthful");
    assert_eq!(s.available_books, 1, "only the asset with real bytes is available");
    // stale_ready = 1 ⇒ 至少计入 waiting
    assert!(
        s.waiting_books >= 1,
        "stale-ready must be counted as waiting, got {}",
        s.waiting_books
    );
    assert_eq!(s.discovered_books, 2);
    assert_eq!(s.other_books, 0, "no unknown durable state here");
    assert_invariant(&s, "F-1/F-2");
}

// ------------------------------------------------------------- F-3：pending / retry
#[test]
fn f_waiting_includes_pending_and_retry() {
    let source = "f-prog-waiting";
    let _guard = prepare(source, 2);
    seed_job(source, "a", CoverJobState::Pending);
    seed_job(source, "b", CoverJobState::RetryWait);

    let s = status_of(source);
    assert_eq!(s.pending_books, 1);
    assert_eq!(s.retry_books, 1);
    assert_eq!(s.available_books, 0);
    assert_eq!(s.waiting_books, 2, "pending + retry both wait");
    assert_invariant(&s, "F-3");
}

// --------------------------------------------------------- F-4：no-job（discovered > jobs）
#[test]
fn f_no_job_comics_are_counted_as_waiting_not_lost() {
    let source = "f-prog-nojob";
    let _guard = prepare(source, 3); // discovered = 3
    seed_job(source, "a", CoverJobState::Pending); // tracked distinct = 1

    let s = status_of(source);
    assert_eq!(s.discovered_books, 3);
    assert_eq!(s.pending_books, 1);
    // 1 pending + 2 no-job
    assert_eq!(
        s.waiting_books, 3,
        "comics with no job row must surface as waiting, not silently disappear"
    );
    assert_invariant(&s, "F-4");
}

// ------------------------------------------------- F-5：failed / unsupported / blocked
#[test]
fn f_terminal_failures_are_not_waiting() {
    let source = "f-prog-terminal";
    let _guard = prepare(source, 3);
    seed_job(source, "a", CoverJobState::Failed);
    seed_job(source, "b", CoverJobState::Unsupported);
    seed_job(source, "c", CoverJobState::Blocked);

    let s = status_of(source);
    assert_eq!(s.failed_books, 1);
    assert_eq!(s.unsupported_books, 1);
    assert_eq!(s.blocked_books, 1);
    assert_eq!(
        s.waiting_books, 0,
        "failed / unsupported / blocked must NOT be counted as waiting"
    );
    assert_eq!(s.available_books, 0);
    assert_invariant(&s, "F-5");
}

// ------------------------------------------------- F-6：真实未知 durable state ⇒ other
#[test]
fn f_unknown_durable_state_is_surfaced_as_other() {
    let source = "f-prog-unknown";
    let _guard = prepare(source, 1);
    seed_job(source, "a", CoverJobState::Ready);
    // 直接写入一个当前代码不认识的 state 值
    {
        let conn = db::get().lock().unwrap();
        conn.execute(
            "UPDATE remote_cover_job SET state='quantum' WHERE source_id=?1 AND asset_id='a'",
            [source],
        )
        .unwrap();
    }

    let s = status_of(source);
    assert_eq!(
        s.other_books, 1,
        "an unknown durable state must be surfaced, not silently dropped"
    );
    assert_eq!(s.ready_books, 0, "it is no longer a known ready row");
    assert_invariant(&s, "F-6");
}

// --------------------------------------------------------------- F-7：staged > indexed
#[test]
fn f_staged_greater_than_indexed_still_balances() {
    let source = "f-prog-staged";
    let _guard = prepare(source, 1); // indexed = 1
    // staged cover tasks 比 indexed 多 ⇒ discovered 取较大者
    {
        let conn = db::get().lock().unwrap();
        for i in 0..3 {
            conn.execute(
                "INSERT OR REPLACE INTO remote_cover_stage(
                     source_id,generation,book_key,dependency_path,dependency_fingerprint,profile,session_epoch)
                 VALUES(?1,?2,?3,?4,'cf','340x480@1','e1')",
                params![
                    source,
                    GENERATION,
                    format!("bk{i}"),
                    format!("/staged{i}.cbz")
                ],
            )
            .unwrap();
        }
    }

    let s = status_of(source);
    assert_eq!(s.discovered_books, 3, "discovered = max(indexed, staged)");
    assert_invariant(&s, "F-7 staged > indexed");
}


// ---------------------------------------------------------------------------
// RG-B 性能修复（方案 B）：`available_books` 按 revision 记忆化 + 廉价校验
// ---------------------------------------------------------------------------

/// 删除自定义缓存根下的**所有**文件（模拟"字节消失但无 durable 变化"）。
fn delete_all_material(root: &std::path::Path) {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    let _ = std::fs::remove_file(p);
                }
            }
        }
    }
}

/// 同一 revision 内：判定结果被复用（**不再触文件系统**）；revision 前进后必须重算。
///
/// 这条同时把方案 B 的**陈旧边界**写成断言（字节消失但无 durable 变化 ⇒ 保持上次结果），
/// 避免它成为未记录的隐含行为。
#[test]
fn f_availability_is_memoized_within_a_revision_and_recomputes_after_a_transition() {
    let source = "f-prog-memo";
    let guard_root = std::env::temp_dir().join(format!("rch_f_prog_{source}"));
    // indexed = 2：后面要加第二个 job，tracked 不得超过 discovered（否则触发不变量断言）。
    let _guard = prepare(source, 2);
    seed_job(source, "a", CoverJobState::Ready);
    write_material(source, "a");

    let first = status_of(source);
    assert_eq!(first.available_books, 1, "material present => available");
    let revision_before = first.view_revision;

    // 字节消失，但**不**产生 durable 变化（不 bump revision）。
    delete_all_material(&guard_root);

    let second = status_of(source);
    assert_eq!(
        second.view_revision, revision_before,
        "deleting cache bytes is not a durable cover change"
    );
    assert_eq!(
        second.available_books, 1,
        "within the same revision the availability answer is memoized (documented bound)"
    );

    // 触发一次 durable cover 变化 ⇒ revision 前进 ⇒ 必须重算 ⇒ 字节已不在 ⇒ available 归零。
    seed_job(source, "b", CoverJobState::Failed);
    let third = status_of(source);
    assert!(
        third.view_revision > revision_before,
        "a durable cover transition must bump the revision"
    );
    assert_eq!(
        third.available_books, 0,
        "after a revision change the sweep re-runs and sees the missing material"
    );
}

/// RG-B 回归：真实数据里 `tracked > discovered` 是**常态**（封面 job 跨代际/列表变更累积，
/// 实测 267 > 9）。该形状**绝不允许 panic** —— 修复前它会经 FRB 变成 Dart 异常，
/// 使 `restoreStatuses` 每次启动都失败（errors.log 反复记录 PanicException）。
///
/// 注意：tracked > discovered 时"各桶之和 == discovered"在数学上不可满足
/// （job 桶本身就超过 discovered），因此该恒等式只在 tracked <= discovered 时成立；
/// 超出的部分由 `other_books` **可见地**吸收，这里只断言"不 panic + 超出可见"。
#[test]
fn f_tracked_greater_than_discovered_never_panics_and_is_absorbed_into_other() {
    let source = "f-prog-tracked-over";
    let _guard = prepare(source, 1); // discovered = 1
    seed_job(source, "a", CoverJobState::Ready);
    seed_job(source, "b", CoverJobState::Pending); // tracked = 2 > discovered = 1

    // 修复前：此处直接 panic（cover progress invariant violation）。
    let status = status_of(source);

    assert_eq!(status.discovered_books, 1);
    assert!(
        status.other_books >= 1,
        "超出 discovered 的部分必须在 other 中可见（不得静默丢失）"
    );
    // 不变量在 tracked <= discovered 时才可能成立；此处只要求各桶之和 >= discovered。
    let sum = status.available_books
        + status.waiting_books
        + status.active_books
        + status.failed_books
        + status.unsupported_books
        + status.blocked_books
        + status.other_books;
    assert!(
        sum >= status.discovered_books,
        "sum {} must be at least discovered {}",
        sum,
        status.discovered_books
    );
}
