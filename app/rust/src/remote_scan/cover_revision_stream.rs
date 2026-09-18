//! P1-E：cover durable truth 变化的**唯一** wake-up 传输层。
//!
//! 设计约束（审阅冻结）：
//!
//! * **极窄**：仓库首个 Rust→Dart `StreamSink`，只表达一件事 ——
//!   «cover durable truth for this source may have changed.»
//! * 事件**不得**携带 authoritative 状态：没有 `state` / `error_code` /
//!   retry 信息 / authoritative revision / generation 派生的 UI 语义。
//!   consumer 收到后必须**自己重读** durable state。
//! * **单 subscriber**：只服务 Dart coordinator 一个消费者。支持
//!   install / replace on resubscribe / clear stale sink on send failure。
//!   不做 broadcast bus、多 subscriber、topic routing、per-asset registry。
//! * **commit-after-emit**：`notify_cover_revision` 只能在事务**提交成功之后**调用；
//!   绝不在事务内部 send（Dart 卡顿不得延长 DB 事务，sink 失败不得 rollback
//!   durable state，worker 正确性不得依赖 UI delivery）。
//! * delivery 是 **best-effort**：send 失败只清理失效 sink，不影响 worker/DB。

use crate::api::remote_cover::CoverRevisionEvent;
use crate::frb_generated::StreamSink;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

static SINK: Mutex<Option<StreamSink<CoverRevisionEvent>>> = Mutex::new(None);

/// transport 诊断：production 路径**决定**发出 wake 的次数（与是否有 sink 无关）。
/// 仅用于契约测试观察"durable transition 是否决定唤醒"，不含任何业务语义。
static WAKE_DECIDED: AtomicU64 = AtomicU64::new(0);

/// 读取"决定唤醒"计数（诊断用）。
pub fn wake_decided_count() -> u64 {
    WAKE_DECIDED.load(Ordering::Relaxed)
}

/// 归零"决定唤醒"计数（测试用）。
pub fn reset_wake_decided_count() {
    WAKE_DECIDED.store(0, Ordering::Relaxed);
}

/// 安装/替换当前 sink（由 `api::remote_cover::subscribe_cover_revisions` 调用）。
///
/// **只支持单个 subscriber**：再次调用会**替换**当前 sink（resubscribe），
/// 不建立 multi-subscriber registry。widget 永远不直接触碰本层。
pub fn install_sink(sink: StreamSink<CoverRevisionEvent>) {
    let mut guard = match SINK.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    // 替换旧 sink：旧 sink 自然失效，不再接收后续事件。
    *guard = Some(sink);
}

/// 发出一次 source-level wake-up。**best-effort**：
///
/// * 必须在 durable mutation **commit 成功之后**调用；
/// * 没有 subscriber 时静默返回（durable state 不受影响）；
/// * send 失败时清理失效 sink 并返回，**不**影响 worker、**不**改 job state。
pub fn notify_cover_revision(source_id: &str, asset_id: Option<&str>) {
    if source_id.trim().is_empty() {
        return;
    }
    // 只要 production 路径判定"durable truth 变了"就记一次决定。
    // 注意：sink 缺失/send 失败**不**回滚任何 durable state。
    WAKE_DECIDED.fetch_add(1, Ordering::Relaxed);
    let mut guard = match SINK.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    let Some(sink) = guard.as_ref() else {
        return;
    };
    let event = CoverRevisionEvent {
        source_id: source_id.to_string(),
        asset_id: asset_id.map(|value| value.to_string()),
    };
    if sink.add(event).is_err() {
        // 失效 sink：清理，避免后续事件继续写向已断开的通道。
        // durable state / revision 已经提交，这里不回滚任何东西。
        *guard = None;
    }
}
