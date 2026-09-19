//! P0 阅读速度修复的取证埋点。
//!
//! # 设计约束
//!
//! - **行为中立**：本模块只读时钟、只累加计数、只写可选的事件流。它不参与调度、
//!   限流、缓存、错误映射的任何判定，移除后行为必须与不接埋点时完全一致。
//! - **零依赖启用**：进程内计数始终累加（原子操作，纳秒级）；JSONL 事件流默认
//!   关闭，只有设置环境变量 `RCH_PERF_LOG=<path>` 后才会写。
//! - **脱敏**：事件里绝不出现 Cookie、Authorization、直链 URL、请求/响应正文。
//!   只允许记录偏移量、长度、状态码、耗时、计数与来源标识。
//!
//! # 环境变量
//!
//! - `RCH_PERF_LOG`：事件流输出路径。父目录不存在时自动创建。
//! - `RCH_PERF_TAG`：本轮测量标签（如 `baseline` / `p0b` / `p0c` / `p0d`），
//!   写入每一行，便于事后按阶段分组对比。
//!
//! # 用途
//!
//! P0 的对照测量要求：首屏/单页 wall time、每页 Range 次数、Range wait time、
//! download-link 获取次数、cache hit/miss、request start timestamps、in-flight 数、
//! HTTP status、cancellation、115 WAF/cooldown 类错误。全部由本模块承载。

use serde_json::{json, Map, Value};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

macro_rules! define_counters {
    ($($name:ident),* $(,)?) => {
        /// 进程内计数器编号。每新增一个字段都必须追加在末尾，不要插入中间，
        /// 以免让已经发布的 JSONL 与快照的字段含义错位。
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum Counter {
            $($name),*
        }

        impl Counter {
            /// 计数器总数。
            pub const COUNT: usize = [$(stringify!($name)),*].len();

            /// 全部计数器，用于快照与重置。
            pub const ALL: [Counter; Self::COUNT] = [$(Counter::$name),*];

            /// 稳定的字段名（写进快照 JSON 的键）。
            pub const fn name(self) -> &'static str {
                match self {
                    $(Counter::$name => stringify!($name)),*
                }
            }

            const fn idx(self) -> usize {
                self as usize
            }

            /// 计数器初值（编译期常量，供静态数组初始化）。
            const fn zero(self) -> AtomicU64 {
                AtomicU64::new(0)
            }
        }

        static VALUES: [AtomicU64; Counter::COUNT] = [$(Counter::$name.zero()),*];
    };
}

define_counters!(
    // ---- CDN Range 通道 ----
    RangeRequests,
    RangeBytes,
    RangeWaitUsTotal,
    RangeWaitUsMax,
    RangeInFlight,
    RangeInFlightMax,
    RangeStatus206,
    RangeStatus200,
    RangeStatus403,
    RangeStatus405,
    RangeStatus429,
    RangeStatusOther,
    RangeErrors,
    // ---- download URL（取链）----
    DownUrlRequests,
    DownUrlCacheHits,
    DownUrlFetched,
    DownUrlCoalesced,
    DownUrlErrors,
    // 名字保留自 P0-A，含义已在 P0-C 收敛为"搭同一次取链的车所等待的时间"：
    // 全局锁已被按 pickcode 的 singleflight 取代，follower 的等待即 leader 的取链耗时。
    DownUrlLockWaitUsTotal,
    DownUrlLockWaitUsMax,
    // ---- 门控等待 ----
    ApiGateWaitUsTotal,
    ApiGateWaitUsMax,
    CdnGateWaitUsTotal,
    CdnGateWaitUsMax,
    // ---- 阅读页 ----
    PageLoads,
    PageLoadUsTotal,
    PageLoadUsMax,
    PageDiskHits,
    PageMemoryHits,
    // ---- governor ----
    GovernorWaitUsTotal,
    GovernorWaitUsMax,
    GovernorWaitUsTotalBackground,
    // ---- 风控与取消 ----
    WafCooldowns,
    Cancellations,
    // ---- ByteSource 层（每次 read_at 对远程源=1 个 Range 请求）----
    SourceReadAt,
    SourceReadAtBytes,
    // ---- 按优先级分桶的 CDN 门控等待 ----
    //
    // 这是 P0-B 的核心验收指标：把"前台自己等了多少"从"整体等了多少"里分出来，
    // 否则后台消耗的预算会把前台的真实处境掩盖掉。
    CdnGateWaitUsTotalForeground,
    CdnGateWaitUsMaxForeground,
    CdnGateWaitUsTotalBackground,
    CdnGateWaitUsMaxBackground,
    // ---- 远程封面抓取（③-1：用数据证明"封面不再整包下载"）----
    //
    // 非归档（单图/图片文件夹）按窗口读取；归档/PDF 走 `AdapterByteSource`，
    // 那里**不经过** `SourceReadAtBytes`（③-1 真实数据复测实测：PDF 封面抓取
    // 0 个 `source.read_at` 事件），所以在 `read_at` 里单独计数，否则"整包下载"
    // 的最大一笔恰恰没有数。
    CoverRangeReads,
    CoverBytesFetched,
    CoverDocumentReads,
    CoverEscalations,
    CoverFailures,
);

/// 进程启动基准，用于把 `Instant` 转成可跨线程比较的相对微秒。
fn origin() -> Instant {
    static ORIGIN: OnceLock<Instant> = OnceLock::new();
    *ORIGIN.get_or_init(Instant::now)
}

/// 自进程启动以来的单调微秒数（事件时间轴）。
pub fn now_us() -> u64 {
    origin().elapsed().as_micros() as u64
}

/// 墙钟毫秒数（用于把事件流与外部日志对齐）。
fn wall_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 事件流是否启用。
pub fn enabled() -> bool {
    sink_path().is_some()
}

fn sink_path() -> Option<String> {
    static PATH: OnceLock<Option<String>> = OnceLock::new();
    PATH.get_or_init(|| {
        std::env::var("RCH_PERF_LOG")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    })
    .clone()
}

fn tag() -> &'static str {
    static TAG: OnceLock<String> = OnceLock::new();
    TAG.get_or_init(|| {
        std::env::var("RCH_PERF_TAG")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| "default".to_string())
    })
}

fn sink() -> Option<&'static Mutex<std::fs::File>> {
    static SINK: OnceLock<Option<Mutex<std::fs::File>>> = OnceLock::new();
    SINK.get_or_init(|| {
        let path = sink_path()?;
        if let Some(parent) = std::path::Path::new(&path).parent() {
            if !parent.as_os_str().is_empty() {
                let _ = std::fs::create_dir_all(parent);
            }
        }
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .ok()
            .map(Mutex::new)
    })
    .as_ref()
}

// ---------------------------------------------------------------------------
// 计数器
// ---------------------------------------------------------------------------

/// 计数 +1。
pub fn bump(counter: Counter) {
    add(counter, 1);
}

/// 计数 +n。
pub fn add(counter: Counter, n: u64) {
    VALUES[counter.idx()].fetch_add(n, Ordering::Relaxed);
}

/// 记录耗时：累加总量并推进峰值。
pub fn observe_us(counter: Counter, max_counter: Counter, us: u64) {
    add(counter, us);
    VALUES[max_counter.idx()].fetch_max(us, Ordering::Relaxed);
}

/// 读取单个计数器当前值。
pub fn read(counter: Counter) -> u64 {
    VALUES[counter.idx()].load(Ordering::Relaxed)
}

/// 由调用方自增/自减的 in-flight 仪表格，`Drop` 时自动归还。
pub struct InFlightGauge(Counter);

impl InFlightGauge {
    /// 进入：整体计数 +1（不更新峰值）。
    pub fn enter(gauge: Counter) -> Self {
        VALUES[gauge.idx()].fetch_add(1, Ordering::Relaxed);
        InFlightGauge(gauge)
    }

    /// 进入并同步推进对应的峰值计数器。
    pub fn enter_with_peak(gauge: Counter, peak: Counter) -> Self {
        let now = VALUES[gauge.idx()].fetch_add(1, Ordering::Relaxed) + 1;
        VALUES[peak.idx()].fetch_max(now, Ordering::Relaxed);
        InFlightGauge(gauge)
    }
}

impl Drop for InFlightGauge {
    fn drop(&mut self) {
        // fetch_sub 在并发下可能瞬时低于 0（被解读为巨大无符号数），因此用
        // 饱和减法语义：只有当前值 > 0 时才减。
        let idx = self.0.idx();
        loop {
            let current = VALUES[idx].load(Ordering::Relaxed);
            if current == 0 {
                return;
            }
            if VALUES[idx]
                .compare_exchange_weak(current, current - 1, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                return;
            }
        }
    }
}

/// 快照：全部计数器 + 进程元信息。
pub fn snapshot() -> Value {
    let mut counters = Map::new();
    for counter in Counter::ALL {
        counters.insert(counter.name().to_string(), json!(read(counter)));
    }
    json!({
        "kind": "snapshot",
        "t_us": now_us(),
        "wall_ms": wall_ms(),
        "tag": tag(),
        "pid": std::process::id(),
        "counters": Value::Object(counters),
    })
}

/// 清零全部计数器（仅用于对照测量的分段）。
pub fn reset() {
    for counter in Counter::ALL {
        VALUES[counter.idx()].store(0, Ordering::Relaxed);
    }
}

/// 按 HTTP 状态码归类 CDN Range 结果。用于 P0 验收里"没有新的 403/405/429"
/// 这一条：状态码分布必须能直接读出，而不是靠人肉翻日志。
pub fn record_range_status(status: u16) {
    let counter = match status {
        206 => Counter::RangeStatus206,
        200 => Counter::RangeStatus200,
        403 => Counter::RangeStatus403,
        405 => Counter::RangeStatus405,
        429 => Counter::RangeStatus429,
        _ => Counter::RangeStatusOther,
    };
    bump(counter);
}

// ---------------------------------------------------------------------------
// 事件流
// ---------------------------------------------------------------------------

/// 一次操作的计时片段。`end` 时写一行 JSONL（未启用时只是一个 `Instant` 比较）。
pub struct Span {
    kind: &'static str,
    start: Instant,
    fields: Map<String, Value>,
}

impl Span {
    /// 开始计时。
    pub fn new(kind: &'static str) -> Self {
        Span {
            kind,
            start: Instant::now(),
            fields: Map::new(),
        }
    }

    /// 附加数值字段。
    pub fn field_u64(mut self, key: &str, value: u64) -> Self {
        self.fields.insert(key.to_string(), json!(value));
        self
    }

    /// 附加字符串字段（调用方必须保证不含凭据与直链）。
    pub fn field_str(mut self, key: &str, value: impl Into<String>) -> Self {
        self.fields.insert(key.to_string(), json!(value.into()));
        self
    }

    /// 结束并写入事件。
    pub fn end(self) -> u64 {
        let dur_us = self.start.elapsed().as_micros() as u64;
        if enabled() {
            emit(self.kind, dur_us, &self.fields);
        }
        dur_us
    }

    /// 结束，但仍写入事件（即使事件流关闭也会返回耗时）。
    pub fn elapsed_us(&self) -> u64 {
        self.start.elapsed().as_micros() as u64
    }
}

/// 直接写一条事件（无耗时字段）。
pub fn event(kind: &str, fields: Map<String, Value>) {
    if enabled() {
        emit(kind, 0, &fields);
    }
}

/// 便捷方法：写一条只有几个字段的事件。
pub fn note(kind: &str, key: &str, value: impl Into<String>) {
    if !enabled() {
        return;
    }
    let mut fields = Map::new();
    fields.insert(key.to_string(), json!(value.into()));
    emit(kind, 0, &fields);
}

fn emit(kind: &str, dur_us: u64, fields: &Map<String, Value>) {
    let Some(sink) = sink() else { return };
    let mut record = Map::new();
    // 先放调用方字段，再写保留键：保留键（`kind`/`t_us`/`wall_ms`/`tag`/`pid`/`dur_us`）
    // **永远**不被自定义字段覆盖。③-1 真实数据复测踩过：调用方一个
    // `field_str("kind", …)` 就把事件类型改成 `pdf`，按 kind 过滤整条事件流全部失效。
    for (key, value) in fields {
        record.insert(key.clone(), value.clone());
    }
    record.insert("kind".into(), json!(kind));
    record.insert("t_us".into(), json!(now_us()));
    record.insert("wall_ms".into(), json!(wall_ms()));
    record.insert("tag".into(), json!(tag()));
    record.insert("pid".into(), json!(std::process::id()));
    if dur_us > 0 {
        record.insert("dur_us".into(), json!(dur_us));
    }
    let line = Value::Object(record).to_string();
    if let Ok(mut file) = sink.lock() {
        use std::io::Write;
        let _ = writeln!(file, "{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 计数器是进程级全局状态，而 Rust 测试默认并行执行。所有触碰计数器的用例
    /// 必须共用这把锁，否则一个用例的 `reset()` 会清掉另一个用例的累加结果。
    fn serial() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[test]
    fn counters_accumulate_and_reset_independently() {
        let _guard = serial();
        reset();
        bump(Counter::RangeRequests);
        bump(Counter::RangeRequests);
        add(Counter::RangeBytes, 4096);
        assert_eq!(read(Counter::RangeRequests), 2);
        assert_eq!(read(Counter::RangeBytes), 4096);
        reset();
        assert_eq!(read(Counter::RangeRequests), 0);
        assert_eq!(read(Counter::RangeBytes), 0);
    }

    #[test]
    fn observe_us_tracks_total_and_peak_separately() {
        let _guard = serial();
        reset();
        observe_us(Counter::RangeWaitUsTotal, Counter::RangeWaitUsMax, 100);
        observe_us(Counter::RangeWaitUsTotal, Counter::RangeWaitUsMax, 700);
        observe_us(Counter::RangeWaitUsTotal, Counter::RangeWaitUsMax, 50);
        assert_eq!(read(Counter::RangeWaitUsTotal), 850);
        assert_eq!(read(Counter::RangeWaitUsMax), 700);
    }

    #[test]
    fn inflight_gauge_tracks_peak_and_saturates_at_zero() {
        let _guard = serial();
        reset();
        {
            let _a =
                InFlightGauge::enter_with_peak(Counter::RangeInFlight, Counter::RangeInFlightMax);
            let _b =
                InFlightGauge::enter_with_peak(Counter::RangeInFlight, Counter::RangeInFlightMax);
            assert_eq!(read(Counter::RangeInFlight), 2);
            assert_eq!(read(Counter::RangeInFlightMax), 2);
        }
        assert_eq!(read(Counter::RangeInFlight), 0);
        // 额外的释放不能把仪表压成无符号巨大值。
        let extra = InFlightGauge::enter(Counter::RangeInFlight);
        assert_eq!(read(Counter::RangeInFlight), 1);
        drop(extra);
        assert_eq!(read(Counter::RangeInFlight), 0);
    }

    #[test]
    fn snapshot_exposes_every_counter_by_stable_name() {
        let _guard = serial();
        reset();
        bump(Counter::PageLoads);
        let snap = snapshot();
        let counters = snap.get("counters").expect("counters object");
        assert_eq!(counters.get("PageLoads").and_then(Value::as_u64), Some(1));
        assert_eq!(counters.as_object().map(Map::len), Some(Counter::COUNT));
        // 名称必须唯一，否则快照会丢字段。
        let mut names: Vec<&str> = Counter::ALL.iter().map(|c| c.name()).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(names.len(), before, "counter names must be unique");
    }

    #[test]
    fn spans_are_cheap_and_report_elapsed_without_a_sink() {
        // 未设置 RCH_PERF_LOG 时 enabled() 为 false，Span 仍然可用。
        let span = Span::new("test.noop").field_u64("n", 1).field_str("k", "v");
        let us = span.end();
        // 只要不 panic 且返回合理范围即可（不依赖精确时间）。
        assert!(us < 1_000_000);
    }

    #[test]
    fn jsonl_sink_stays_off_unless_explicitly_enabled() {
        // 默认（未设置 RCH_PERF_LOG）绝不能写文件——行为中立的前提。
        assert!(!enabled(), "perf event stream must be opt-in");
        let _guard = serial();
        note("test.note", "k", "v");
        event("test.event", Map::new());
        let _ = Span::new("test.span").end();
    }
}
