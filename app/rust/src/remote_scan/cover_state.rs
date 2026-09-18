//! Cover job 状态机的**唯一**语义来源（P1-A）。
//!
//! # 为什么需要这个模块
//!
//! 改造前，`upsert_job_on` 用 `state=remote_cover_job.state` 无条件保留旧状态。
//! 这条规则让"目录重扫 / 全量重扫 / 人工重试 / 阻塞原因解除 / 能力变化"**全部**
//! 都无法把任务从终态拉回来 —— 封面一旦进入 `failed`/`unsupported`/`blocked`
//! 就再也无法恢复，而失败原因早已消失。
//!
//! 但"保留状态"本身并不全是错的：普通 demand（卡片可见、目录重扫）**必须**保留
//! 退避排期与终态，否则每次 UI 出现都会重开一轮请求风暴。既有契约测试
//! `remote_cover_queue_contract::ordinary_visible_demand_does_not_bypass_backoff_or_terminal_errors`
//! 锁定的就是这条。
//!
//! 所以要消灭的不是"保留"，而是**隐式**：把触发原因显式化成 [`CoverJobUpsertCause`]，
//! 由 [`resolve_upsert_state`] 的唯一规则表决定结果。
//!
//! # 冻结的 transition matrix
//!
//! 状态：`pending` / `running` / `ready` / `retry_wait` / `failed` / `unsupported`
//! / `blocked` / `cancelled`。
//!
//! 事件 → 新状态（行 = 现有状态，列 = 事件）：
//!
//! | 现有 \\ 事件 | Demand | FullRescan | ManualRetry | CauseCleared | CapabilityChanged |
//! |---|---|---|---|---|---|
//! | （无记录） | requested | requested | requested | requested | requested |
//! | pending | pending | pending | pending | pending | pending |
//! | running | running | running | running | running | running |
//! | ready | ready | ready | ready | ready | ready |
//! | retry_wait | retry_wait | **pending** | **pending** | **pending** | **pending** |
//! | failed | failed | **pending** | **pending** | failed | failed |
//! | unsupported | unsupported | unsupported | **pending** | unsupported | **pending** |
//! | blocked | blocked | blocked | **pending** | **pending** | blocked |
//! | cancelled | **pending** | **pending** | **pending** | cancelled | cancelled |
//!
//! 各状态语义（进入原因 / 退出事件 / 自动重试 / 需要登录 / 需要网络 /
//! 受 provider cooldown 约束 / 需要能力变化 / 可被 full rescan 重置）：
//!
//! | 状态 | 进入原因 | 允许退出的事件 | 自动重试 | 需登录 | 需网络 | 受 cooldown | 需能力变化 | full rescan 重置 |
//! |---|---|---|---|---|---|---|---|---|
//! | `pending` | 新需求 / 缺口 / 原因解除 | claim | — | 否 | 是 | 是 | 否 | 是（保持） |
//! | `running` | worker claim | ready / retry_wait / failed / lease 过期回收 | — | 是 | 是 | 是 | 否 | 否（不打断在途） |
//! | `ready` | 取得并发布封面 | 文件丢失对账 | — | 否 | 否 | 否 | 否 | 否（保留） |
//! | `retry_wait` | 可重试失败（含 429/cooldown） | 到期 claim | **是**（有界） | 是 | 是 | 是 | 否 | 是 |
//! | `failed` | 自动补偿预算耗尽 / 永久失败 | ManualRetry / FullRescan | **否** | 视原因 | 是 | 是 | 否 | 是 |
//! | `unsupported` | provider/归档能力不支持 | CapabilityChanged / ManualRetry | **否**（禁止按时间周期重试） | 否 | 是 | 否 | **是** | **否** |
//! | `blocked` | 环境阻塞（未登录 / 无网络 / cooldown / 权限） | CauseCleared / ManualRetry | **否** | 视原因 | 视原因 | 视原因 | 否 | **否** |
//! | `cancelled` | 需求消失（卡片卸载） | 新的 demand | 否 | 否 | 否 | 否 | 否 | 是 |
//!
//! 冻结原则（来自 P1 审阅）：
//!
//! - `unsupported` **不按时间周期重试**，只有 provider / 归档 capability 或
//!   source metadata 等相关能力变化才重新评估。
//! - `blocked` **不按普通 failed 重试**，只有阻塞条件实际解除（重新登录 /
//!   account changed / 网络重新开启 / cooldown 到期 / 权限恢复）才回到候选队列。
//! - `failed` 区分 retryable / permanent；只有 retryable 可自动补偿；默认 6 小时后
//!   **最多自动补偿一次**；再次失败后停止自动循环，等待明确状态变化或人工 retry。
//!   禁止用"所有 failed 每 6 小时扫一次"实现。
//! - `ready` 若文件不存在或损坏，允许重新进入 `pending`；该转换后必须真正唤醒 worker。

use super::cover_model::CoverJobState;

/// 触发一次 job 记录写入的原因。
///
/// 显式化它是 P1-A 的核心：状态迁移由原因决定，而不是由"upsert 一律保留"这种
/// 隐式规则决定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoverJobUpsertCause {
    /// 普通需求：目录重扫发现、卡片可见、后台补齐。**不解除退避与终态**。
    Demand,
    /// 用户主动发起的全量重扫：允许重置 `failed` 与 `retry_wait`。
    FullRescan,
    /// 明确的人工重试。
    ManualRetry,
    /// 阻塞原因已实际解除（重新登录 / account changed / 网络重新开启 /
    /// provider cooldown 到期 / 权限恢复）。
    CauseCleared,
    /// provider / 归档 capability 或相关 source metadata 发生变化。
    CapabilityChanged,
}

/// 由**唯一规则表**决定一次 upsert 之后的状态。
///
/// `existing` 为 `None` 表示这是新记录。`requested` 是调用方希望的状态
/// （通常为 `pending`）。规则见模块文档的 transition matrix。
pub fn resolve_upsert_state(
    existing: Option<CoverJobState>,
    requested: CoverJobState,
    cause: CoverJobUpsertCause,
) -> CoverJobState {
    use CoverJobState as S;
    let Some(existing) = existing else {
        return requested;
    };
    // 在途任务永不因一次记录写入而改变状态：worker 正在持有 lease。
    if existing == S::Running {
        return S::Running;
    }
    match cause {
        CoverJobUpsertCause::Demand => match existing {
            // 普通需求保留排期与终态，避免"UI 每次出现就重开一轮请求"。
            S::Failed | S::Unsupported | S::Blocked | S::RetryWait => existing,
            // 需求回来了：被取消的任务重新排队。
            S::Cancelled => requested,
            _ => existing,
        },
        CoverJobUpsertCause::FullRescan => match existing {
            // 封面仍在：不回退。
            S::Ready => S::Ready,
            // 用户主动全量重扫 = 一次明确的重评估，允许重置放弃态与排期。
            S::Failed | S::RetryWait | S::Cancelled => requested,
            // 能力未变、阻塞原因未解除：全量重扫**不是**它们的解除条件。
            S::Unsupported | S::Blocked => existing,
            _ => existing,
        },
        CoverJobUpsertCause::ManualRetry => match existing {
            S::Ready => S::Ready,
            _ => requested,
        },
        CoverJobUpsertCause::CauseCleared => match existing {
            // 只有受环境阻塞或等待重试的任务适用。
            S::Blocked | S::RetryWait => requested,
            _ => existing,
        },
        CoverJobUpsertCause::CapabilityChanged => match existing {
            S::Unsupported | S::RetryWait => requested,
            _ => existing,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::CoverJobState as S;
    use super::*;

    /// 表驱动：把冻结的 matrix 逐行钉住。任何一行被改动都必须是有意的。
    #[test]
    fn frozen_upsert_transition_matrix() {
        use CoverJobUpsertCause as C;
        let all = [
            S::Pending,
            S::Running,
            S::Ready,
            S::RetryWait,
            S::Failed,
            S::Unsupported,
            S::Blocked,
            S::Cancelled,
        ];
        // (cause, 现有状态 -> 期望状态)
        let rows: &[(C, S, S)] = &[
            // Demand：保留排期与终态，仅把 cancelled 拉回队列
            (C::Demand, S::Pending, S::Pending),
            (C::Demand, S::Running, S::Running),
            (C::Demand, S::Ready, S::Ready),
            (C::Demand, S::RetryWait, S::RetryWait),
            (C::Demand, S::Failed, S::Failed),
            (C::Demand, S::Unsupported, S::Unsupported),
            (C::Demand, S::Blocked, S::Blocked),
            (C::Demand, S::Cancelled, S::Pending),
            // FullRescan：可重置 failed / retry_wait / cancelled，不可重置
            // unsupported / blocked（它们的解除条件不是"重扫"）
            (C::FullRescan, S::Pending, S::Pending),
            (C::FullRescan, S::Running, S::Running),
            (C::FullRescan, S::Ready, S::Ready),
            (C::FullRescan, S::RetryWait, S::Pending),
            (C::FullRescan, S::Failed, S::Pending),
            (C::FullRescan, S::Unsupported, S::Unsupported),
            (C::FullRescan, S::Blocked, S::Blocked),
            (C::FullRescan, S::Cancelled, S::Pending),
            // ManualRetry：除 ready / running 外一律重新排队
            (C::ManualRetry, S::Pending, S::Pending),
            (C::ManualRetry, S::Running, S::Running),
            (C::ManualRetry, S::Ready, S::Ready),
            (C::ManualRetry, S::RetryWait, S::Pending),
            (C::ManualRetry, S::Failed, S::Pending),
            (C::ManualRetry, S::Unsupported, S::Pending),
            (C::ManualRetry, S::Blocked, S::Pending),
            (C::ManualRetry, S::Cancelled, S::Pending),
            // CauseCleared：只解除 blocked / retry_wait
            (C::CauseCleared, S::Blocked, S::Pending),
            (C::CauseCleared, S::RetryWait, S::Pending),
            (C::CauseCleared, S::Failed, S::Failed),
            (C::CauseCleared, S::Unsupported, S::Unsupported),
            (C::CauseCleared, S::Ready, S::Ready),
            (C::CauseCleared, S::Running, S::Running),
            // CapabilityChanged：只解除 unsupported / retry_wait
            (C::CapabilityChanged, S::Unsupported, S::Pending),
            (C::CapabilityChanged, S::RetryWait, S::Pending),
            (C::CapabilityChanged, S::Failed, S::Failed),
            (C::CapabilityChanged, S::Blocked, S::Blocked),
            (C::CapabilityChanged, S::Ready, S::Ready),
            (C::CapabilityChanged, S::Running, S::Running),
        ];

        for (cause, existing, expected) in rows {
            assert_eq!(
                resolve_upsert_state(Some(*existing), S::Pending, *cause),
                *expected,
                "cause={cause:?} existing={} must resolve to {}",
                existing.as_str(),
                expected.as_str()
            );
        }

        // 新记录：一律按请求状态落库。
        for cause in [
            C::Demand,
            C::FullRescan,
            C::ManualRetry,
            C::CauseCleared,
            C::CapabilityChanged,
        ] {
            assert_eq!(
                resolve_upsert_state(None, S::Pending, cause),
                S::Pending,
                "a brand new job must be created as pending for {cause:?}"
            );
        }

        // 组合完备性：matrix 必须对每个 (状态, 原因) 组合都有确定结果。
        for cause in [
            C::Demand,
            C::FullRescan,
            C::ManualRetry,
            C::CauseCleared,
            C::CapabilityChanged,
        ] {
            for state in all {
                let resolved = resolve_upsert_state(Some(state), S::Pending, cause);
                assert!(
                    all.contains(&resolved),
                    "matrix must resolve {cause:?} x {}",
                    state.as_str()
                );
            }
        }
    }

    /// 冻结原则：`unsupported` 与 `blocked` 不得被"重扫/普通需求"解除。
    #[test]
    fn unsupported_and_blocked_are_not_reset_by_ordinary_causes() {
        use CoverJobUpsertCause as C;
        for state in [S::Unsupported, S::Blocked] {
            for cause in [
                C::Demand,
                C::FullRescan,
                C::CauseCleared,
                C::CapabilityChanged,
            ] {
                let resolved = resolve_upsert_state(Some(state), S::Pending, cause);
                if resolved != state {
                    // 只有"该状态自己的解除原因"才允许退出。
                    let allowed = matches!(
                        (state, cause),
                        (S::Blocked, C::CauseCleared) | (S::Unsupported, C::CapabilityChanged)
                    );
                    assert!(
                        allowed,
                        "{} must not be cleared by {cause:?}",
                        state.as_str()
                    );
                }
            }
        }
    }
}
