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
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

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

/// RG-B 性能修复（方案 B）：`available_books` 的**按 revision 记忆化**。
///
/// 依据 P1 冻结语义：`remote_view_revision` = 原子 durable view change token ⇒
/// **同一 revision 内 ready 集合不变**，因此"存在且非空"的判定结果可在同一 revision 内复用。
/// 键还包含**缓存根**（缓存根变更/清理 ⇒ 立即失效），避免跨根的陈旧命中。
///
/// 已知语义边界（方案 B 的取舍，已记录于报告）：若字节在**无** revision 变化时消失
/// （例如系统清理缓存目录），`available_books` 会保持上次结果，直到
/// ① 下一次 durable cover 变化（revision 前进）或 ② 缓存根变更。
static AVAILABILITY_MEMO: OnceLock<Mutex<HashMap<String, (i64, String, u64)>>> = OnceLock::new();

fn availability_memo() -> &'static Mutex<HashMap<String, (i64, String, u64)>> {
    AVAILABILITY_MEMO.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 锁**外**回填 `available_books` / `waiting_books` / `other_books` 并校验不变量。
///
/// 只读缓存/文件系统：无 DB mutation、无 revision bump、无 wake、无 session/provider。
pub(crate) fn apply_cover_availability(status: &mut RemoteScanStatusDto) {
    let inputs = INPUTS.with(|slot| std::mem::take(&mut *slot.borrow_mut()));
    // 命中条件：同一 source + 同一 revision + 同一缓存根 ⇒ 不触文件系统。
    let cache_root = crate::cache::cache_root().to_string_lossy().to_string();
    let revision = status.view_revision;
    let cached = availability_memo()
        .lock()
        .ok()
        .and_then(|guard| guard.get(&status.source_id).cloned())
        .filter(|(rev, root, _)| *rev == revision && *root == cache_root)
        .map(|(_, _, available)| available);

    let available = match cached {
        Some(available) => available,
        None => {
            let mut counted = 0_u64;
            for (asset_id, content_revision, selection_revision, profile) in &inputs.ready {
                if super::cover_service::cover_material_present(
                    &status.source_id,
                    asset_id,
                    content_revision,
                    selection_revision,
                    profile,
                ) {
                    counted = counted.saturating_add(1);
                }
            }
            if let Ok(mut guard) = availability_memo().lock() {
                guard.insert(status.source_id.clone(), (revision, cache_root, counted));
            }
            counted
        }
    };
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
