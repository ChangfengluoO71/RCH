//! P1-F：cover 进度的**语义层**（锁内收集 → 锁外校验 → 回填 + 不变量）。
//!
//! 为什么单独成模块：FRB 只扫描 `crate::api`，把这一层放在 `remote_scan` 下
//! 可以避免为内部辅助类型生成绑定（也符合"api 只做薄表面"的分层）。
//!
//! 锁纪律（本仓冻结规则）：**No file I/O under the database lock**。
//! 因此分两步：
//!   1. `publish_cover_availability_inputs` 在**锁内**只做纯 DB 聚合并把结果带出；
//!   2. `apply_cover_availability` 在**锁已释放后**才做缓存/文件系统校验并回填。

use crate::api::remote_scan::RemoteScanStatusDto;

/// 锁内收集、锁外校验所需的输入。
#[derive(Default)]
pub(crate) struct CoverAvailabilityInputs {
    /// 当前 generation 内**不同 asset** 的 job 数（latest-per-asset 语义）。
    pub tracked_distinct: u64,
    /// 真实未知 durable state 的数量（不得静默丢弃）。
    pub other_unknown: u64,
    /// ready job 的 cover 身份：asset、content_revision、selection_revision、profile。
    pub ready: Vec<(String, String, String, String)>,
    /// 已被 `pending_books` 代表的 staged-only 漫画数（P1-D 兼容 fallback）。
    ///
    /// 这些漫画**没有 job 行**，因此也会落入 `no_job`；必须在此扣除，
    /// 否则同一批漫画会被重复计数并破坏不变量。
    pub staged_pending_represented: u64,
}

thread_local! {
    static INPUTS: std::cell::RefCell<CoverAvailabilityInputs> =
        const { std::cell::RefCell::new(CoverAvailabilityInputs {
            tracked_distinct: 0,
            other_unknown: 0,
            ready: Vec::new(),
            staged_pending_represented: 0,
        }) };
}

/// 锁内调用：把聚合结果交给随后的锁外回填（同一线程同步传递）。
pub(crate) fn publish_cover_availability_inputs(
    tracked_distinct: u64,
    other_unknown: u64,
    ready: Vec<(String, String, String, String)>,
) {
    INPUTS.with(|slot| {
        *slot.borrow_mut() = CoverAvailabilityInputs {
            tracked_distinct,
            other_unknown,
            ready,
            staged_pending_represented: 0,
        };
    });
}

/// 锁内调用：记录"已被 pending 代表的 staged-only 漫画数"（P1-D 兼容 fallback）。
///
/// 它必须从 `no_job` 中扣除，否则 staged-only 漫画会被 pending 与 no_job 重复计数。
pub(crate) fn publish_staged_pending_represented(count: u64) {
    INPUTS.with(|slot| {
        slot.borrow_mut().staged_pending_represented = count;
    });
}

/// 锁**外**回填 `available_books` / `waiting_books` / `other_books` 并校验不变量。
///
/// 只读缓存/文件系统：无 DB mutation、无 revision bump、无 wake、无 session/provider。
pub(crate) fn apply_cover_availability(status: &mut RemoteScanStatusDto) {
    let inputs = INPUTS.with(|slot| std::mem::take(&mut *slot.borrow_mut()));
    let mut available = 0_u64;
    for (asset_id, content_revision, selection_revision, profile) in &inputs.ready {
        if super::cover_service::cover_material_available(
            &status.source_id,
            asset_id,
            content_revision,
            selection_revision,
            profile,
        ) {
            available = available.saturating_add(1);
        }
    }
    status.available_books = available;
    // stale ready = 状态为 ready 但字节不可用 ⇒ 归入 waiting（不计入 available）。
    let stale_ready = status.ready_books.saturating_sub(available);
    // staged-only 漫画已由 `pending_books`（P1-D 兼容 fallback）代表 ⇒ 从 no_job 扣除，
    // 否则同一批漫画会被重复计数。
    let no_job_signed = status.discovered_books as i64
        - inputs.tracked_distinct as i64
        - inputs.staged_pending_represented as i64;
    let mut other = inputs.other_unknown;
    let no_job = if no_job_signed < 0 {
        // 不变量违例：tracked > discovered。**不得静默 clamp 后报告正常** ——
        // 显式暴露（debug 断言 + 计入 other，使 other > 0 可见）。
        debug_assert!(
            false,
            "cover progress invariant violation: tracked {} > discovered {}",
            inputs.tracked_distinct, status.discovered_books
        );
        other = other.saturating_add(no_job_signed.unsigned_abs());
        0
    } else {
        no_job_signed as u64
    };
    status.waiting_books = status
        .pending_books
        .saturating_add(status.retry_books)
        .saturating_add(stale_ready)
        .saturating_add(no_job);
    status.other_books = other;
    debug_assert_eq!(
        status.available_books
            + status.waiting_books
            + status.active_books
            + status.failed_books
            + status.unsupported_books
            + status.blocked_books
            + status.other_books,
        status.discovered_books,
        "P1-F invariant: available+waiting+active+failed+unsupported+blocked+other == discovered"
    );
}
