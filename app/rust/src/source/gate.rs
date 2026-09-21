//! P0-B：把"限速"从"固定间隔 + 持锁 sleep"改成**优先级令牌桶 + 独立在途上限**。
//!
//! # 为什么要改
//!
//! P0-A 实测（`../../../../.trellis/tasks/09-14-remote-cover-cleanup/research/p0/2026-09-17-p0a-baseline.md`）：
//!
//! - 9 次 Range 读，服务端到达时刻被精确摊成
//!   `[0, 249, 500, 750, 1000, 1250, 1501, 1751, 2001] ms`；`wall_ms = 2259` 里
//!   门控等待占 2197 ms（97.3%），真实网络成本约 60 ms。
//! - 9 个调用线程同时进入，服务端观测到的最大并发**仍是 1** —— 旧的
//!   `RateGate::wait` 持锁 sleep，把并发彻底串行化。
//! - 前台阅读在有后台 Range 负载时 p50 从 762 ms 涨到 3895 ms（**5.11×**）。
//!
//! # 设计（对应审阅冻结的第 3、4、5 条）
//!
//! 1. **速率与并发分开治理**：令牌桶只决定"什么时候允许发起下一个请求"；
//!    另用一个**独立**的在途上限约束真实并发连接数。两者不互相替代。
//! 2. **优先级真正贯穿到单次网络请求**：`Foreground > Prefetch > Cover > Scan`。
//!    前台只能抢"下一个可用许可"，不打断已经在途的请求。
//! 3. **等待不持锁 sleep**：`Condvar::wait_timeout` 分片等待，等待期间释放互斥量；
//!    分片边界重新评估优先级、取消信号与令牌可用时间。
//! 4. **防饿死**：aging 提升（等待越久有效优先级越高，最多升到 Foreground）+
//!    同级按 ticket 序号 FIFO，两者合起来保证任何等待者都有界地前进。
//! 5. **等待可取消**：`CancelSignal` 基于**代际**而非布尔位 —— 取消是单向的，
//!    新请求不会被历史信号误伤。
//!
//! # 同步/异步模型（审阅第 5 条要求先确认，再决定原语）
//!
//! 这条链路**是同步阻塞的**，因此 `Condvar` 是与运行时匹配的原语：
//!
//! - `source/mod.rs` 的 `trait ByteSource { fn read_at(&self, ..) -> io::Result<usize> }`
//!   是同步签名，无法 `.await`；
//! - `reader.rs` 用 `std::thread::spawn` 起预取线程，`BlockingRequestGovernor`
//!   也是 `Mutex` + `Condvar`；
//! - provider 客户端是 `reqwest::blocking`；
//! - async 只出现在 FRB 边界（`api/source.rs` 用 `tokio::task::spawn_blocking` 桥接）。
//!
//! 所以在 `read_at` 到 CDN 这段同步代码里 `tokio::sync::Notify/Semaphore` 用不了
//! （非 async 上下文不能 `.await`）。选 `Condvar` 是模型匹配的结果，不是习惯复制。

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// 阻塞型远程请求的优先级。
///
/// 同时被 `reader::BlockingRequestGovernor`（粗粒度、跨整段任务）与本模块的
/// `RateGate`（细粒度、每次网络请求）使用，保证外层优先级能真正到达底层请求。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RequestPriority {
    Foreground,
    Prefetch,
    Cover,
    Scan,
}

impl RequestPriority {
    /// 队列下标：0 最高。
    pub(crate) const fn queue_index(self) -> usize {
        self as usize
    }

    pub(crate) const fn is_background(self) -> bool {
        !matches!(self, Self::Foreground)
    }
}

thread_local! {
    /// 本线程当前正在为哪个优先级工作。
    ///
    /// 用线程本地而不是给 `ByteSource::read_at` 加参数：`read_at` 是同步 trait
    /// 方法、被所有格式解析器共享，加参数会波及每个 Document 实现；而这些工作
    /// 单元天然是"一个线程一件事"（前台读页 / 预取线程 / 封面 worker / 扫描
    /// worker），线程本地能精确表达且不改接口。
    static CURRENT_PRIORITY: std::cell::Cell<Option<RequestPriority>> =
        const { std::cell::Cell::new(None) };
}

/// 本线程当前的有效优先级。
///
/// 未显式标注时按 `Foreground`：未标注的调用都是用户直接触发的临时读写
/// （封面编辑器、详情页等），不该被后台任务挤后。真正的工作单元都会显式标注。
pub fn current_priority() -> RequestPriority {
    CURRENT_PRIORITY
        .with(std::cell::Cell::get)
        .unwrap_or(RequestPriority::Foreground)
}

/// 在指定优先级作用域内执行 `f`；退出（含 panic）时恢复原值。
pub fn with_priority<R>(priority: RequestPriority, f: impl FnOnce() -> R) -> R {
    struct Restore(Option<RequestPriority>);
    impl Drop for Restore {
        fn drop(&mut self) {
            CURRENT_PRIORITY.with(|slot| slot.set(self.0));
        }
    }
    let previous = CURRENT_PRIORITY.with(|slot| slot.replace(Some(priority)));
    let _restore = Restore(previous);
    f()
}

/// 单向取消信号：基于代际。
///
/// `cancel()` 使所有"登记代际早于当前代际"的等待者退出。之所以不用布尔位：
/// 布尔位无法区分"本次取消"与"上一次已过期的取消"，新请求会被历史信号误伤。
#[derive(Clone, Default)]
pub struct CancelSignal(Arc<AtomicU64>);

impl CancelSignal {
    pub fn new() -> Self {
        Self::default()
    }

    /// 触发取消。
    pub fn cancel(&self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }

    /// 当前代际；调用方在开始等待前登记它。
    pub fn epoch(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

/// 令牌桶参数。
#[derive(Clone, Copy, Debug)]
pub struct GateLimit {
    /// 持续速率（请求/秒）。`<= 0` 表示不做速率限制。
    pub per_sec: f64,
    /// 突发额度：允许一次性连发多少个请求而不必等补桶。
    ///
    /// `burst = 1` 即退化为旧的固定间隔行为（每 `1/per_sec` 秒才准发一个）。
    pub burst: f64,
    /// **独立于速率**的在途并发上限（真实同时打开多少个请求）。
    pub max_in_flight: usize,
    /// 等待超过该毫秒数即把有效优先级提升一级（防饿死）；0 表示不提升。
    pub aging_ms: u64,
}

impl GateLimit {
    /// 把旧的"每秒 N 次固定间隔"翻译成令牌桶：突发 1、在途 1 —— 行为等价。
    pub const fn fixed_interval(per_sec: f64) -> Self {
        Self {
            per_sec,
            burst: 1.0,
            max_in_flight: 1,
            aging_ms: 1_000,
        }
    }

    /// 不做任何限制（本地缓存路径不该经过门控）。
    pub const fn unlimited() -> Self {
        Self {
            per_sec: 0.0,
            burst: 0.0,
            max_in_flight: usize::MAX,
            aging_ms: 1_000,
        }
    }
}

/// 等待期间被取消。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GateCancelled;

impl std::fmt::Display for GateCancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "请求在等待限速许可时被取消")
    }
}

impl std::error::Error for GateCancelled {}

struct Ticket {
    id: u64,
    enqueued: Instant,
}

/// 门控状态快照（只读，用于埋点与测试）。
#[derive(Debug, Clone, Copy, Default)]
pub struct GateSnapshot {
    /// 各优先级当前排队数（下标 = `RequestPriority::queue_index`）。
    pub queued: [usize; 4],
    pub in_flight: usize,
    /// 距下一个令牌可用还需多少微秒（0 = 现在就有）。
    pub next_token_in_us: u64,
}

struct GateState {
    tokens: f64,
    last_refill: Instant,
    next_ticket: u64,
    queues: [VecDeque<Ticket>; 4],
    in_flight: usize,
}

/// 带优先级与独立在途上限的门控。
///
/// `enter()` 返回的 [`GateGuard`] 在 drop 时归还"在途"名额，因此调用方必须把它
/// 绑定到局部变量并持有到实际请求结束。
pub(crate) struct RateGate {
    channel: &'static str,
    limit: GateLimit,
    state: Mutex<GateState>,
    changed: Condvar,
}

/// 持有它表示"占有一个令牌与一个在途名额"。
#[must_use = "在途名额在 Guard drop 时归还；不绑定会在请求发出前就释放"]
pub(crate) struct GateGuard<'a> {
    gate: &'a RateGate,
    waited_us: u64,
}

impl GateGuard<'_> {
    /// 本次取得许可实际等待的微秒数。
    pub fn waited_us(&self) -> u64 {
        self.waited_us
    }
}

impl Drop for GateGuard<'_> {
    fn drop(&mut self) {
        let mut state = self.gate.state.lock().unwrap();
        state.in_flight = state.in_flight.saturating_sub(1);
        drop(state);
        self.gate.changed.notify_all();
    }
}

impl RateGate {
    /// 带通道名与限速参数的构造。`channel` 只用于埋点归属。
    pub fn new(channel: &'static str, limit: GateLimit) -> Self {
        let burst = if limit.burst > 0.0 { limit.burst } else { 1.0 };
        RateGate {
            channel,
            limit,
            state: Mutex::new(GateState {
                // 起始满桶：第一个请求永远不必等。
                tokens: burst,
                last_refill: Instant::now(),
                next_ticket: 0,
                queues: std::array::from_fn(|_| VecDeque::new()),
                in_flight: 0,
            }),
            changed: Condvar::new(),
        }
    }

    /// 旧式"每秒 N 次固定间隔"的兼容构造（行为与改造前等价）。
    pub fn fixed_interval(channel: &'static str, per_sec: f64) -> Self {
        Self::new(channel, GateLimit::fixed_interval(per_sec))
    }

    pub fn channel(&self) -> &'static str {
        self.channel
    }

    pub fn limit(&self) -> GateLimit {
        self.limit
    }

    /// 纯查询：排队数、在途数、距下一个令牌的时间。不改变任何状态。
    ///
    /// 它必须"随时可查" —— 这正是旧实现做不到的（旧实现持锁 sleep，查询会被
    /// 睡着的线程挡住）。
    pub fn snapshot(&self) -> GateSnapshot {
        let mut state = self.state.lock().unwrap();
        self.refill(&mut state);
        let mut queued = [0_usize; 4];
        for (index, queue) in state.queues.iter().enumerate() {
            queued[index] = queue.len();
        }
        GateSnapshot {
            queued,
            in_flight: state.in_flight,
            next_token_in_us: self.time_until_token(&state).as_micros() as u64,
        }
    }

    fn refill(&self, state: &mut GateState) {
        if self.limit.per_sec <= 0.0 {
            return;
        }
        let now = Instant::now();
        let elapsed = now.saturating_duration_since(state.last_refill);
        state.last_refill = now;
        let burst = if self.limit.burst > 0.0 {
            self.limit.burst
        } else {
            1.0
        };
        state.tokens = (state.tokens + elapsed.as_secs_f64() * self.limit.per_sec).min(burst);
    }

    fn time_until_token(&self, state: &GateState) -> Duration {
        if self.limit.per_sec <= 0.0 || state.tokens >= 1.0 {
            return Duration::ZERO;
        }
        let missing = 1.0 - state.tokens;
        Duration::from_secs_f64((missing / self.limit.per_sec).max(0.0))
    }

    /// 等待并获得一个许可。优先级取自 [`current_priority`]。
    ///
    /// 只抢"下一个可用许可"，不打断已经在途的请求。
    pub fn enter(&self) -> GateGuard<'_> {
        match self.enter_inner(None) {
            Ok(guard) => guard,
            Err(()) => unreachable!("uncancellable wait cannot be cancelled"),
        }
    }

    /// 同上，但等待期间若 `cancel` 的代际发生变化则立即放弃等待。
    pub fn enter_cancellable(&self, cancel: &CancelSignal) -> Result<GateGuard<'_>, GateCancelled> {
        let epoch = cancel.epoch();
        self.enter_inner(Some((cancel, epoch)))
            .map_err(|()| GateCancelled)
    }

    fn enter_inner(&self, cancel: Option<(&CancelSignal, u64)>) -> Result<GateGuard<'_>, ()> {
        let priority = current_priority();
        let entered = Instant::now();
        debug_assert!(priority.queue_index() < 4);
        let mut state = self.state.lock().unwrap();

        // 快速路径：不限速且还有在途名额 → 立刻放行，不排队、不注册 ticket。
        if self.limit.per_sec <= 0.0 && state.in_flight < self.limit.max_in_flight {
            state.in_flight += 1;
            drop(state);
            return Ok(GateGuard {
                gate: self,
                waited_us: entered.elapsed().as_micros() as u64,
            });
        }

        let id = state.next_ticket;
        state.next_ticket = state.next_ticket.wrapping_add(1);
        state.queues[priority.queue_index()].push_back(Ticket {
            id,
            enqueued: Instant::now(),
        });

        let mut admitted = false;
        loop {
            self.refill(&mut state);

            if self.can_admit(&state, id) {
                state.tokens -= 1.0;
                state.in_flight += 1;
                admitted = true;
                break;
            }

            if let Some((signal, epoch)) = cancel {
                if signal.epoch() != epoch {
                    break;
                }
            }

            // 分片等待：取"下一个令牌可用时间"与 20 ms 的较小值。
            // 分片而不是一次睡到底，是为了让优先级抢占、aging 提升与取消信号
            // 都能被及时观察到；且分片期间互斥量是**释放**的。
            let slice = self
                .time_until_token(&state)
                .min(Duration::from_millis(20))
                .max(Duration::from_millis(1));
            let (next, _) = self.changed.wait_timeout(state, slice).unwrap();
            state = next;
        }

        // 无论放行还是取消，都把自己从队列里摘掉。
        if let Some(queue) = state
            .queues
            .iter_mut()
            .find(|queue| queue.iter().any(|t| t.id == id))
        {
            queue.retain(|t| t.id != id);
        }
        drop(state);
        if !admitted {
            return Err(());
        }
        Ok(GateGuard {
            gate: self,
            waited_us: entered.elapsed().as_micros() as u64,
        })
    }

    /// 某个 ticket 的**有效**优先级下标：基础下标按等待时长提升，最多升到 0。
    fn effective_index(&self, index: usize, ticket: &Ticket, now: Instant) -> usize {
        if self.limit.aging_ms == 0 {
            return index;
        }
        let waited_ms = now.saturating_duration_since(ticket.enqueued).as_millis() as u64;
        let promotions = (waited_ms / self.limit.aging_ms).min(3) as usize;
        index.saturating_sub(promotions)
    }

    /// 放行条件：令牌够、在途未满、且自己在"（有效优先级, ticket 序号）"字典序上最小。
    ///
    /// 有效优先级负责优先级与 aging；ticket 序号单调递增，因此被提升到同一级的
    /// 老等待者天然排在前面 —— 这保证不会永久饿死。
    fn can_admit(&self, state: &GateState, id: u64) -> bool {
        if state.tokens < 1.0 || state.in_flight >= self.limit.max_in_flight {
            return false;
        }
        let now = Instant::now();
        let mut mine: Option<(usize, u64)> = None;
        let mut best: Option<(usize, u64)> = None;
        for (index, queue) in state.queues.iter().enumerate() {
            for ticket in queue {
                let candidate = (self.effective_index(index, ticket, now), ticket.id);
                if ticket.id == id {
                    mine = Some(candidate);
                }
                if best.is_none_or(|current| candidate < current) {
                    best = Some(candidate);
                }
            }
        }
        match (mine, best) {
            (Some(mine), Some(best)) => mine == best,
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn gate(per_sec: f64, burst: f64, max_in_flight: usize) -> Arc<RateGate> {
        Arc::new(RateGate::new(
            "test",
            GateLimit {
                per_sec,
                burst,
                max_in_flight,
                aging_ms: 10_000, // 默认关掉 aging，让优先级测试不受提升干扰
            },
        ))
    }

    fn ms(value: u64) -> Duration {
        Duration::from_millis(value)
    }

    #[test]
    fn priority_scope_is_thread_local_and_restored() {
        assert_eq!(current_priority(), RequestPriority::Foreground);
        with_priority(RequestPriority::Cover, || {
            assert_eq!(current_priority(), RequestPriority::Cover);
            with_priority(RequestPriority::Scan, || {
                assert_eq!(current_priority(), RequestPriority::Scan);
            });
            assert_eq!(current_priority(), RequestPriority::Cover);
        });
        assert_eq!(current_priority(), RequestPriority::Foreground);
    }

    #[test]
    fn foreground_waiter_is_admitted_before_an_older_background_waiter() {
        let gate = gate(20.0, 1.0, 1);
        let order = Arc::new(Mutex::new(Vec::<&'static str>::new()));

        // 占住唯一的令牌与在途名额。
        let held = with_priority(RequestPriority::Foreground, || gate.enter());

        let cover = {
            let gate = Arc::clone(&gate);
            let order = Arc::clone(&order);
            std::thread::spawn(move || {
                let _guard = with_priority(RequestPriority::Cover, || gate.enter());
                order.lock().unwrap().push("cover");
            })
        };
        // 让后台先排队。
        std::thread::sleep(ms(20));

        let foreground = {
            let gate = Arc::clone(&gate);
            let order = Arc::clone(&order);
            std::thread::spawn(move || {
                let _guard = with_priority(RequestPriority::Foreground, || gate.enter());
                order.lock().unwrap().push("foreground");
            })
        };
        std::thread::sleep(ms(20));

        drop(held);
        cover.join().unwrap();
        foreground.join().unwrap();

        // 这正是旧实现做不到的：旧实现按到达顺序放行，先排队的 Cover 会先拿到。
        assert_eq!(
            order.lock().unwrap().first().copied(),
            Some("foreground"),
            "foreground must take the next available permit ahead of a queued cover wait"
        );
    }

    #[test]
    fn background_is_not_permanently_starved_by_a_foreground_flood() {
        let gate = Arc::new(RateGate::new(
            "test",
            GateLimit {
                per_sec: 100.0,
                burst: 1.0,
                max_in_flight: 1,
                aging_ms: 30, // 30 ms 提升一级 → 90 ms 内升到 Foreground
            },
        ));

        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flood = {
            let gate = Arc::clone(&gate);
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                while !stop.load(Ordering::SeqCst) {
                    let _guard = with_priority(RequestPriority::Foreground, || gate.enter());
                    std::thread::sleep(Duration::from_millis(1));
                }
            })
        };

        // 后台在洪泛中间挂号，必须在有界时间内拿到许可。
        std::thread::sleep(ms(50));
        let started = Instant::now();
        let cover_guard = with_priority(RequestPriority::Cover, || gate.enter());
        let waited = started.elapsed();
        // 必须先归还唯一的在途名额再 join：在途上限=1 时，被卡在 enter() 里的
        // 洪泛线程不会自行退出（这正是"并发上限独立于速率"生效的证据）。
        drop(cover_guard);

        stop.store(true, Ordering::SeqCst);
        flood.join().unwrap();

        assert!(
            waited < ms(1_000),
            "aging must bound background starvation; waited {waited:?}"
        );
    }

    #[test]
    fn waiting_request_can_be_cancelled_without_waiting_for_the_token() {
        let gate = gate(1.0, 1.0, 1); // 1 token/s → 一次等待接近 1 秒
        let held = with_priority(RequestPriority::Foreground, || gate.enter());
        let signal = CancelSignal::new();

        let worker = {
            let gate = Arc::clone(&gate);
            let signal = signal.clone();
            std::thread::spawn(move || {
                let started = Instant::now();
                let outcome =
                    with_priority(RequestPriority::Cover, || gate.enter_cancellable(&signal));
                (outcome.is_err(), started.elapsed())
            })
        };

        std::thread::sleep(ms(60));
        signal.cancel();
        let (cancelled, waited) = worker.join().unwrap();
        drop(held);

        assert!(
            cancelled,
            "the waiting request must observe the cancel signal"
        );
        assert!(
            waited < ms(600),
            "cancellation must not wait for the token; waited {waited:?}"
        );
        assert_eq!(
            gate.snapshot().in_flight,
            0,
            "cancelled wait must not leak an in-flight slot"
        );
    }

    #[test]
    fn snapshot_stays_responsive_while_another_thread_is_waiting() {
        let gate = gate(1.0, 1.0, 1);
        let held = with_priority(RequestPriority::Foreground, || gate.enter());

        let waiter = {
            let gate = Arc::clone(&gate);
            std::thread::spawn(move || {
                let _guard = with_priority(RequestPriority::Cover, || gate.enter());
            })
        };
        std::thread::sleep(ms(30));

        // 旧实现持锁 sleep，任何并发查询/登记都会被睡着的线程挡住整个间隔；
        // 新实现分片等待并在等待时释放互斥量，因此这里必须是微秒级。
        let started = Instant::now();
        let snapshot = gate.snapshot();
        let probe = started.elapsed();

        assert!(
            probe < ms(60),
            "state must stay observable while a waiter sleeps; took {probe:?}"
        );
        assert_eq!(snapshot.queued[RequestPriority::Cover.queue_index()], 1);
        assert_eq!(snapshot.in_flight, 1);

        drop(held);
        waiter.join().unwrap();
    }

    #[test]
    fn in_flight_cap_is_independent_of_the_rate() {
        // 完全不限速，只限制并发 —— 这两个维度必须能分开设置。
        let gate = Arc::new(RateGate::new(
            "test",
            GateLimit {
                per_sec: 0.0,
                burst: 0.0,
                max_in_flight: 2,
                aging_ms: 10_000,
            },
        ));

        let first = with_priority(RequestPriority::Foreground, || gate.enter());
        let second = with_priority(RequestPriority::Foreground, || gate.enter());

        let (tx, rx) = mpsc::channel();
        let third = {
            let gate = Arc::clone(&gate);
            std::thread::spawn(move || {
                let _guard = with_priority(RequestPriority::Foreground, || gate.enter());
                tx.send(()).unwrap();
            })
        };
        assert!(
            rx.recv_timeout(ms(80)).is_err(),
            "the third request must be blocked by the in-flight cap"
        );

        drop(first);
        assert!(
            rx.recv_timeout(ms(500)).is_ok(),
            "releasing an in-flight slot must admit the waiting request"
        );
        drop(second);
        third.join().unwrap();
    }

    #[test]
    fn burst_lets_one_page_of_windows_through_without_per_request_spacing() {
        // 4 req/s 但允许突发 6 —— 一页 1.5 MiB / 256 KiB = 6 个窗口正好放行，
        // 这是"读速不再由固定 4 QPS 门控形成理论下限"的核心机制。
        let gate = gate(4.0, 6.0, 8);
        let started = Instant::now();
        let mut guards = Vec::new();
        for _ in 0..6 {
            guards.push(with_priority(RequestPriority::Foreground, || gate.enter()));
        }
        let elapsed = started.elapsed();

        assert!(
            elapsed < ms(300),
            "a burst of 6 must not be spaced by 250 ms each; took {elapsed:?}"
        );
        assert_eq!(gate.snapshot().in_flight, 6);

        // 第 7 个必须回到速率约束：4 req/s → 约 250 ms。
        let seventh = {
            let gate = Arc::clone(&gate);
            std::thread::spawn(move || {
                let started = Instant::now();
                let _guard = with_priority(RequestPriority::Foreground, || gate.enter());
                started.elapsed()
            })
        };
        let waited = seventh.join().unwrap();
        assert!(
            waited >= ms(200),
            "beyond the burst the sustained rate must still apply; waited {waited:?}"
        );
        drop(guards);
    }

    #[test]
    fn gates_are_independent_so_api_waits_never_block_cdn_reads() {
        let api = Arc::new(RateGate::fixed_interval("test.api", 1.0));
        let cdn = Arc::new(RateGate::fixed_interval("test.cdn", 1.0));

        // 把 API 门的令牌吃掉，让它进入长时间等待。
        let _api_held = with_priority(RequestPriority::Foreground, || api.enter());
        let api_waiter = {
            let api = Arc::clone(&api);
            std::thread::spawn(move || {
                let _guard = with_priority(RequestPriority::Cover, || api.enter());
            })
        };
        std::thread::sleep(ms(30));

        // CDN 门必须完全不受影响：CDN 的第一次请求是满桶，立刻放行。
        let started = Instant::now();
        let _cdn_guard = with_priority(RequestPriority::Foreground, || cdn.enter());
        let waited = started.elapsed();

        assert!(
            waited < ms(60),
            "CDN must not queue behind the API gate; waited {waited:?}"
        );
        drop(_api_held);
        api_waiter.join().unwrap();
    }

    #[test]
    fn fixed_interval_limit_reproduces_the_old_spacing_exactly() {
        // burst=1 时必须与旧的固定间隔语义一致，保证 API 通道行为零变化。
        let gate = Arc::new(RateGate::fixed_interval("test.api", 10.0)); // 100 ms
                                                                         // `fixed_interval` 的在途上限是 1，所以必须先把第一个许可还回去，
                                                                         // 否则第二个请求会被并发上限（而不是速率）挡住。
        let first = with_priority(RequestPriority::Foreground, || gate.enter());
        drop(first);
        let started = Instant::now();
        let _second = with_priority(RequestPriority::Foreground, || gate.enter());
        let waited = started.elapsed();
        assert!(
            waited >= ms(70),
            "burst=1 must space requests by 1/rate; waited {waited:?}"
        );
    }
}
