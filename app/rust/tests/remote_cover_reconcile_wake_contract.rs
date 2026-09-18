//! P1-B：reconcile（ready 但字节缺失）之后**必须真的存在消费者**。
//!
//! 这是 P1-B 要修的**那一个**缺失 wake 的写入点的端到端契约：
//!
//! 1. 一条 `ready` 的封面记录，其磁盘字节已经丢失；
//! 2. 只读路径 `remote_cover_read` 探测到缺失 → 把记录对账回 `pending`；
//! 3. 对账之后**必须**有消费者真正把它领走 —— 否则这本漫画的封面会永久停在
//!    `pending`，直到下一次全量扫描才可能被重试。
//!
//! 端到端驱动真实生产路径（FRB `remote_cover_read` → `cover_service` 对账 →
//! source-level wake → worker claim），不需要任何 provider：测试源的 route 表为空，
//! worker 的 route-missing 分支是纯 DB 的，因此**零网络 I/O**。

use rust_lib_app::api::remote_cover::{remote_cover_read, CoverProfileDto, CoverSelectionDto};
use rust_lib_app::remote_scan::cover_model::{CoverJobKey, CoverJobState};
use rust_lib_app::remote_scan::cover_state::CoverJobUpsertCause;
use rust_lib_app::remote_scan::{cover_store, persistence};
use rust_lib_app::{cache, db};

// `selection_key` 的真实格式：page:{page}|crop:{crop}|asset:{explicit}
const SELECTION: &str = "page:2|crop:|asset:";
const PROFILE: &str = "170x240@1";

fn prepare(source_id: &str) {
    let root = std::env::temp_dir().join(format!("rch_p1b_reconcile_{source_id}"));
    std::fs::create_dir_all(&root).unwrap();
    cache::set_custom_cache_root(root.to_str().unwrap());

    let conn = db::get().lock().unwrap();
    persistence::migrate(&conn).unwrap();
    cover_store::migrate(&conn).unwrap();
    for table in [
        "remote_cover_job",
        "remote_cover_variant",
        "remote_scan_epoch",
    ] {
        conn.execute(
            &format!("DELETE FROM {table} WHERE source_id=?1"),
            [source_id],
        )
        .unwrap();
    }
    conn.execute(
        "INSERT OR REPLACE INTO book_sources(id,type,name) VALUES(?1,'115','p1b-reconcile')",
        [source_id],
    )
    .unwrap();
    conn.execute(
        "INSERT OR REPLACE INTO remote_scan_epoch(
             source_id,generation,source_fingerprint,root_path,session_epoch,session_token)
         VALUES(?1,1,'fp','/','epoch',42)",
        [source_id],
    )
    .unwrap();

    // ready 的 job + ready 的 variant，updated_at 一致；磁盘上没有对应字节。
    cover_store::upsert_job_on(
        &conn,
        &CoverJobKey {
            source_id: source_id.into(),
            asset_id: "asset".into(),
            content_revision: "content".into(),
            selection_revision: SELECTION.into(),
            profile: PROFILE.into(),
        },
        CoverJobState::Ready,
        "background",
        10,
        1,
        "epoch",
        1,
        CoverJobUpsertCause::Demand,
    )
    .unwrap();
    conn.execute(
        "INSERT OR REPLACE INTO remote_cover_variant(
             source_id,asset_id,content_revision,selection_revision,profile,state,revision,updated_at)
         VALUES(?1,'asset','content',?2,?3,'ready',1,1)",
        rusqlite::params![source_id, SELECTION, PROFILE],
    )
    .unwrap();
}

fn job_state(source_id: &str) -> Option<(String, i64)> {
    let conn = db::get().lock().unwrap();
    conn.query_row(
        "SELECT state,attempt FROM remote_cover_job WHERE source_id=?1 AND asset_id='asset'",
        [source_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .ok()
}

fn wait_until<F: Fn() -> bool>(predicate: F, timeout_ms: u64) -> bool {
    let started = std::time::Instant::now();
    loop {
        if predicate() {
            return true;
        }
        if started.elapsed().as_millis() >= u128::from(timeout_ms) {
            return predicate();
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[test]
fn a_byte_less_ready_cover_is_reconciled_to_pending_and_then_actually_consumed() {
    let source = "p1b-reconcile-source";
    prepare(source);

    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("tokio runtime");
    let read = runtime.block_on(remote_cover_read(
        source.to_string(),
        "asset".to_string(),
        CoverSelectionDto {
            page: 2,
            crop: None,
            explicit_asset_id: None,
            revision: String::new(),
        },
        CoverProfileDto {
            width: 170,
            height: 240,
            decoder_version: 1,
        },
    ));
    let image = read.expect("read must not error");
    assert!(image.is_none(), "no bytes => no image");

    // 对账已经发生（既有契约）：记录必须离开 `ready`。
    //
    // 这里不断言恰好是 `pending` —— 本修复接上 wake 之后，worker 可能在同一次
    // 调用内就把这条 pending 领走（变为 `running`）。两者都是"对账已发生"。
    assert_ne!(
        job_state(source).map(|(state, _)| state),
        Some("ready".to_string()),
        "a byte-less ready cover must be reconciled away from ready"
    );

    // 关键新增契约：对账之后必须真的有消费者把它领走。
    assert!(
        wait_until(
            || job_state(source).map(|(state, _)| state != "pending") == Some(true),
            3_000
        ),
        "the reconcile must leave a live consumer behind, not a stranded pending job"
    );
    assert_eq!(
        job_state(source).map(|(_, attempt)| attempt),
        Some(1),
        "the stranded job must be claimed exactly once"
    );
}
