//! 阅读会话:L1 内存缓存 + L2 磁盘缓存 + 后台并行预取。
//!
//! L1(内存 LRU)管"翻页零等待";L2(磁盘)管"重复阅读秒开"——
//! 读过的页字节写盘,下次打开同一本书(尤其 WebDAV)无需重新下载。

use crate::document::Document;
use anyhow::{bail, Result};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Instant;

/// L1 内存缓存容量(原始页字节)。
const CACHE_CAP: usize = 24;
/// 预取半径(以当前页为中心,前后各预取的页数)。
const PREFETCH_RADIUS: i64 = 3;
const REQUEST_GOVERNOR_CAPACITY: usize = 3;
const REQUEST_GOVERNOR_QUEUE_CAPACITY: usize = 64;

/// 最近一次**前台**网络取页的时间戳（毫秒；0 = 从未）。
///
/// 第 79 轮真机结论：手机上门控只有 4 请求/秒、2 并发，几百个后台封面任务会把带宽与
/// CDN 连接吃满，前台翻页只能排在它们中间 ⇒ 表现为"翻页一直转圈"。
/// 后台封面任务据此让路（见 `api::remote_scan::run_remote_cover_worker`）。
static LAST_FOREGROUND_READ_MS: AtomicI64 = AtomicI64::new(0);

fn note_foreground_read() {
    LAST_FOREGROUND_READ_MS.store(crate::db::now_ms(), Ordering::Relaxed);
}

/// 距上一次前台网络取页的毫秒数；从未取过返回 `None`。
pub fn foreground_read_idle_ms() -> Option<i64> {
    match LAST_FOREGROUND_READ_MS.load(Ordering::Relaxed) {
        0 => None,
        last => Some((crate::db::now_ms() - last).max(0)),
    }
}

/// Priority for blocking work that might make a remote document request.
///
/// 唯一定义在 `source::gate`，这里只做再导出：粒度较粗的 governor（跨整段任务）
/// 与粒度较细的 `RateGate`（每次网络请求）必须共用同一个优先级取值，否则外层
/// 优先级无法贯通到底层请求。
///
/// The governor only chooses which queued work may start. Once the permit is
/// held, the underlying synchronous I/O remains non-cancellable.
pub use crate::source::gate::RequestPriority;

/// Fair, bounded coordinator for blocking remote work.
///
/// Background work is limited to `capacity - 1`, reserving one opportunity
/// for a current-page request. FIFO is preserved within each priority class;
/// queued foreground work prevents lower priorities from starting first.
pub struct BlockingRequestGovernor {
    capacity: usize,
    queue_capacity: usize,
    state: Mutex<RequestGovernorState>,
    changed: Condvar,
}

struct RequestGovernorState {
    active_total: usize,
    active_background: usize,
    next_ticket: u64,
    queues: [VecDeque<u64>; 4],
}

/// Held only while the blocking operation has actually started.
pub struct BlockingRequestPermit<'a> {
    governor: &'a BlockingRequestGovernor,
    priority: RequestPriority,
}

impl BlockingRequestGovernor {
    pub fn new(capacity: usize, queue_capacity: usize) -> Self {
        assert!(capacity >= 2, "reserve one foreground opportunity");
        Self {
            capacity,
            queue_capacity,
            state: Mutex::new(RequestGovernorState {
                active_total: 0,
                active_background: 0,
                next_ticket: 0,
                queues: std::array::from_fn(|_| VecDeque::new()),
            }),
            changed: Condvar::new(),
        }
    }

    pub fn acquire(&self, priority: RequestPriority) -> Result<BlockingRequestPermit<'_>> {
        let mut state = self.state.lock().unwrap();
        if state.queues[priority.queue_index()].len() >= self.queue_capacity {
            bail!("blocking request priority queue is full");
        }
        let ticket = state.next_ticket;
        state.next_ticket = state.next_ticket.wrapping_add(1);
        state.queues[priority.queue_index()].push_back(ticket);

        loop {
            if self.can_start(&state, priority, ticket) {
                state.queues[priority.queue_index()].pop_front();
                state.active_total += 1;
                if priority.is_background() {
                    state.active_background += 1;
                }
                return Ok(BlockingRequestPermit {
                    governor: self,
                    priority,
                });
            }
            state = self.changed.wait(state).unwrap();
        }
    }

    fn can_start(
        &self,
        state: &RequestGovernorState,
        priority: RequestPriority,
        ticket: u64,
    ) -> bool {
        if state.queues[priority.queue_index()].front() != Some(&ticket) {
            return false;
        }
        if state.active_total >= self.capacity {
            return false;
        }
        match priority {
            RequestPriority::Foreground => true,
            RequestPriority::Prefetch => {
                state.queues[RequestPriority::Foreground.queue_index()].is_empty()
                    && state.active_background < self.capacity - 1
            }
            RequestPriority::Cover => {
                state.queues[RequestPriority::Foreground.queue_index()].is_empty()
                    && state.queues[RequestPriority::Prefetch.queue_index()].is_empty()
                    && state.active_background < self.capacity - 1
            }
            RequestPriority::Scan => {
                state.queues[RequestPriority::Foreground.queue_index()].is_empty()
                    && state.queues[RequestPriority::Prefetch.queue_index()].is_empty()
                    && state.queues[RequestPriority::Cover.queue_index()].is_empty()
                    && state.active_background < self.capacity - 1
            }
        }
    }

    fn release(&self, priority: RequestPriority) {
        let mut state = self.state.lock().unwrap();
        state.active_total -= 1;
        if priority.is_background() {
            state.active_background -= 1;
        }
        self.changed.notify_all();
    }
}

impl Drop for BlockingRequestPermit<'_> {
    fn drop(&mut self) {
        self.governor.release(self.priority);
    }
}

/// Process-wide governor shared by reader work and safe remote cover reads.
pub fn blocking_request_governor() -> Arc<BlockingRequestGovernor> {
    static GOVERNOR: OnceLock<Arc<BlockingRequestGovernor>> = OnceLock::new();
    GOVERNOR
        .get_or_init(|| {
            Arc::new(BlockingRequestGovernor::new(
                REQUEST_GOVERNOR_CAPACITY,
                REQUEST_GOVERNOR_QUEUE_CAPACITY,
            ))
        })
        .clone()
}

/// 轻量 LRU:容量有限的内存缓存。
struct Lru {
    map: HashMap<u32, Arc<Vec<u8>>>,
    order: VecDeque<u32>, // 队首 = 最近使用
    cap: usize,
}

impl Lru {
    fn new(cap: usize) -> Self {
        Lru {
            map: HashMap::new(),
            order: VecDeque::new(),
            cap,
        }
    }

    fn contains(&self, k: &u32) -> bool {
        self.map.contains_key(k)
    }

    fn get(&mut self, k: &u32) -> Option<Arc<Vec<u8>>> {
        if let Some(v) = self.map.get(k) {
            let v = v.clone();
            self.order.retain(|x| x != k);
            self.order.push_front(*k);
            Some(v)
        } else {
            None
        }
    }

    fn insert(&mut self, k: u32, v: Arc<Vec<u8>>) {
        if self.map.contains_key(&k) {
            self.order.retain(|x| x != &k);
        } else if self.map.len() >= self.cap {
            if let Some(old) = self.order.pop_back() {
                self.map.remove(&old);
            }
        }
        self.order.push_front(k);
        self.map.insert(k, v);
    }
}

/// 一本书的阅读会话。
pub struct Reader {
    book: Box<dyn Document>, // page_bytes 为 &self,可并发调用,无需锁
    cache: Mutex<Lru>,
    /// 所有正在生成的页。前台读取与后台预取共享，避免同一页重复解码/渲染。
    inflight: Mutex<HashSet<u32>>,
    /// 某页生成完成（成功或失败）后唤醒等待该页的前台读取。
    inflight_done: Condvar,
    /// 该书的磁盘缓存目录(原始页字节)。
    disk_dir: PathBuf,
    /// 当前显示宽度（0 = 未指定 ⇒ 沿用文档默认渲染宽度 1600）。
    ///
    /// D7（2026-09-21）：阅读页一页 1600 宽的渲染结果约 1.9 MB 要经 FRB 交给 Dart，
    /// 是"翻页重"的主要来源；把宽度交给用户选（省流 1080 / 标准 1600 / 跟随屏幕）。
    /// 宽度是**每个 Reader 的会话状态**：前台取页设置它，预取沿用同一个值，
    /// 这样同一本书不会因为前台/预取而写出两套尺寸的页。
    display_width: std::sync::atomic::AtomicU32,
    governor: Arc<BlockingRequestGovernor>,
}

impl Reader {
    /// `cache_ns`:该书在磁盘缓存中的命名空间(同一本书应稳定不变)。
    pub fn new(book: Box<dyn Document>, cache_ns: &str) -> Self {
        let disk_dir = crate::cache::CacheDir::Page
            .ensure()
            .ok()
            .unwrap_or_else(|| {
                // 兜底：直接构造路径
                let p = crate::cache::cache_root()
                    .join("cache")
                    .join("page")
                    .join(crate::cache::stable_hash(cache_ns));
                let _ = std::fs::create_dir_all(&p);
                p
            });

        let dir = disk_dir.join(crate::cache::stable_hash(cache_ns));
        let _ = std::fs::create_dir_all(&dir);
        Reader {
            book,
            cache: Mutex::new(Lru::new(CACHE_CAP)),
            inflight: Mutex::new(HashSet::new()),
            inflight_done: Condvar::new(),
            disk_dir: dir,
            display_width: std::sync::atomic::AtomicU32::new(0),
            governor: blocking_request_governor(),
        }
    }

    pub fn page_count(&self) -> u32 {
        self.book.page_count()
    }

    pub fn title(&self) -> String {
        self.book.metadata().title
    }

    /// 获取一页:先 L1 内存,未命中则等待/认领唯一一次实际生成;完成后触发周边预取。
    pub fn get_page(self: &Arc<Self>, index: u32) -> Result<Arc<Vec<u8>>> {
        // P0 埋点：单页端到端 wall time（含等 inflight、等许可、等门控、网络）。
        let span = crate::perf::Span::new("reader.get_page").field_u64("index", index as u64);
        let cached = { self.cache.lock().unwrap().get(&index) };
        if let Some(bytes) = cached {
            crate::perf::bump(crate::perf::Counter::PageMemoryHits);
            span.field_str("source", "l1").end();
            self.spawn_prefetch(index);
            return Ok(bytes);
        }

        let bytes = self.load_or_wait(index)?;
        span.field_str("source", "load").end();
        self.spawn_prefetch(index);
        Ok(bytes)
    }

    /// 打开书后立即预取开头若干页。
    ///
    /// 保留给显式 warm-up 场景；前台 get_page 与这些预取会共享 inflight，不会重复生成同一页。
    pub fn warm_up(self: &Arc<Self>) {
        self.spawn_prefetch(0);
    }

    /// 前台读取：若同页已有后台/前台生成任务则等待；否则成为唯一生成者。
    fn load_or_wait(&self, index: u32) -> Result<Arc<Vec<u8>>> {
        loop {
            // 所有涉及 inflight + cache 的嵌套加锁统一使用 inflight -> cache 顺序。
            let mut inflight = self.inflight.lock().unwrap();
            while inflight.contains(&index) {
                inflight = self.inflight_done.wait(inflight).unwrap();
            }

            // 生成者完成后缓存可能已经可用；在认领前必须二次检查，避免完成/认领竞态。
            if let Some(bytes) = self.cache.lock().unwrap().get(&index) {
                return Ok(bytes);
            }

            inflight.insert(index);
            drop(inflight);
            return self.load_claimed(index, RequestPriority::Foreground);
        }
    }

    /// 后台预取尝试认领一页；已缓存或已在生成时不再额外启动线程。
    fn try_claim_prefetch(&self, index: u32) -> bool {
        let mut inflight = self.inflight.lock().unwrap();
        if inflight.contains(&index) || self.cache.lock().unwrap().contains(&index) {
            return false;
        }
        inflight.insert(index);
        true
    }

    /// 已认领页的唯一实际读取路径。无论成功、失败还是 panic 都释放 inflight 并唤醒等待者。
    fn load_claimed(&self, index: u32, priority: RequestPriority) -> Result<Arc<Vec<u8>>> {
        use crate::perf::{add, bump, observe_us, Counter};
        let span = crate::perf::Span::new("reader.load_claimed")
            .field_u64("index", index as u64)
            .field_str("priority", format!("{priority:?}"));
        let governor_enter = Instant::now();
        let mut disk_hit = false;
        let mut governor_wait_us = 0_u64;
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // Local cache hits must not queue behind remote requests.
            if let Some(bytes) = self.disk_get(index) {
                disk_hit = true;
                return Ok(Arc::new(bytes));
            }
            // 真正要上网取页了：前台请求据此让后台封面让路（磁盘命中不算 —— 那种情况下
            // 不占网络，后台封面可以继续跑）。
            if priority == RequestPriority::Foreground {
                note_foreground_read();
            }
            let _permit = self.governor.acquire(priority)?;
            governor_wait_us = governor_enter.elapsed().as_micros() as u64;
            // 把优先级标注到本线程，使底层每一次网络请求（CDN Range / 取链）
            // 都能按同一个优先级排队 —— 这是"外层优先级贯通到底层"的落点。
            crate::source::gate::with_priority(priority, || self.read_page(index))
        }));
        bump(Counter::PageLoads);
        if disk_hit {
            bump(Counter::PageDiskHits);
        }
        observe_us(Counter::PageLoadUsTotal, Counter::PageLoadUsMax, span.elapsed_us());
        observe_us(
            Counter::GovernorWaitUsTotal,
            Counter::GovernorWaitUsMax,
            governor_wait_us,
        );
        if priority.is_background() {
            add(Counter::GovernorWaitUsTotalBackground, governor_wait_us);
        }
        span.field_u64("disk_hit", u64::from(disk_hit))
            .field_u64("governor_wait_us", governor_wait_us)
            .end();

        match outcome {
            Ok(result) => {
                let mut inflight = self.inflight.lock().unwrap();
                if let Ok(bytes) = &result {
                    self.cache.lock().unwrap().insert(index, Arc::clone(bytes));
                }
                inflight.remove(&index);
                self.inflight_done.notify_all();
                drop(inflight);
                result
            }
            Err(payload) => {
                let mut inflight = self.inflight.lock().unwrap_or_else(|p| p.into_inner());
                inflight.remove(&index);
                self.inflight_done.notify_all();
                drop(inflight);
                std::panic::resume_unwind(payload);
            }
        }
    }

    /// 当前显示宽度（0 = 未指定）。
    pub fn display_width(&self) -> u32 {
        self.display_width.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// 设置显示宽度（0 = 回到默认）。**变化时清空 L1**，避免新旧尺寸混用同一页。
    ///
    /// 只清内存缓存：L2 页缓存按宽度分目录（见 [`Reader::page_disk_path`]），
    /// 因此换宽度既不误用旧尺寸的页，也不作废标准档已有的缓存。
    pub fn set_display_width(&self, width: u32) {
        let previous = self
            .display_width
            .swap(width, std::sync::atomic::Ordering::Relaxed);
        if previous != width {
            // `Lru` 没有 clear()：直接换一个新的（容量常量复用同一处定义）。
            *self.cache.lock().unwrap() = Lru::new(CACHE_CAP);
        }
    }

    /// 一页在磁盘缓存里的路径。
    ///
    /// 宽度为 0（标准档）时**沿用历史布局** `page/<ns>/<index>.bin`，
    /// 否则放进 `page/<ns>/w<width>/<index>.bin`（不会与标准档互相污染）。
    fn page_disk_path(&self, index: u32) -> PathBuf {
        let width = self.display_width();
        let dir = if width == 0 {
            self.disk_dir.clone()
        } else {
            self.disk_dir.join(format!("w{width}"))
        };
        dir.join(format!("{index}.bin"))
    }

    /// 读一页:L2 磁盘命中则直接用,否则从书源下载并写盘。
    fn read_page(&self, index: u32) -> Result<Arc<Vec<u8>>> {
        if let Some(bytes) = self.disk_get(index) {
            return Ok(Arc::new(bytes));
        }
        let width = self.display_width();
        let bytes = if width == 0 {
            self.book.page_bytes(index)?
        } else {
            // 只有显式指定显示宽度时才走"按显示尺寸渲染"（目前仅 PDF 覆写）。
            self.book.page_bytes_for_display(index, width)?
        };
        self.disk_put(index, &bytes);
        Ok(Arc::new(bytes))
    }

    fn disk_get(&self, index: u32) -> Option<Vec<u8>> {
        std::fs::read(self.page_disk_path(index)).ok()
    }

    fn disk_put(&self, index: u32, data: &[u8]) {
        let path = self.page_disk_path(index);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(path, data);
    }

    /// 后台并行预取 index 前后各 PREFETCH_RADIUS 页。
    /// 同页前台/后台共用 inflight claim，因此每页最多存在一个实际生成者。
    fn spawn_prefetch(self: &Arc<Self>, index: u32) {
        let count = self.page_count() as i64;
        for off in -PREFETCH_RADIUS..=PREFETCH_RADIUS {
            if off == 0 {
                continue;
            }
            let t = index as i64 + off;
            if t < 0 || t >= count {
                continue;
            }
            let t = t as u32;
            if !self.try_claim_prefetch(t) {
                continue;
            }
            let me = Arc::clone(self);
            std::thread::spawn(move || {
                let _ = me.load_claimed(t, RequestPriority::Prefetch);
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::DocumentMeta;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc, Condvar,
    };
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    #[test]
    fn request_priority_contract_keeps_reader_before_prefetch_before_cover() {
        assert!(
            RequestPriority::Foreground.queue_index() < RequestPriority::Prefetch.queue_index()
        );
        assert!(RequestPriority::Prefetch.queue_index() < RequestPriority::Cover.queue_index());
        assert!(RequestPriority::Cover.queue_index() < RequestPriority::Scan.queue_index());
    }

    #[test]
    fn scan_queue_is_bounded_and_keeps_a_foreground_slot_available() {
        let governor = Arc::new(BlockingRequestGovernor::new(2, 1));
        let active_scan = governor.acquire(RequestPriority::Scan).unwrap();
        let (queued_tx, queued_rx) = mpsc::channel();
        let waiting_governor = Arc::clone(&governor);
        let waiting = std::thread::spawn(move || {
            queued_tx.send(()).unwrap();
            let _permit = waiting_governor.acquire(RequestPriority::Scan).unwrap();
        });
        queued_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        while governor.state.lock().unwrap().queues[RequestPriority::Scan.queue_index()].is_empty()
        {
            assert!(
                std::time::Instant::now() < deadline,
                "scan waiter did not queue"
            );
            std::thread::yield_now();
        }

        assert!(governor.acquire(RequestPriority::Scan).is_err());
        let foreground = governor.acquire(RequestPriority::Foreground).unwrap();
        drop(foreground);
        drop(active_scan);
        waiting.join().unwrap();
    }

    #[test]
    fn queued_foreground_work_wins_over_prefetch_and_cover_work() {
        let governor = Arc::new(BlockingRequestGovernor::new(2, 8));
        let first_foreground = governor
            .acquire(RequestPriority::Foreground)
            .expect("initial foreground permit");
        let first_cover = governor
            .acquire(RequestPriority::Cover)
            .expect("initial cover permit");
        let (started_tx, started_rx) = mpsc::channel();

        for (name, priority) in [
            ("prefetch", RequestPriority::Prefetch),
            ("cover", RequestPriority::Cover),
            ("foreground", RequestPriority::Foreground),
        ] {
            let governor = Arc::clone(&governor);
            let started_tx = started_tx.clone();
            std::thread::spawn(move || {
                let _permit = governor.acquire(priority).expect("queued permit");
                started_tx.send(name).expect("receiver stays alive");
            });
        }
        drop(started_tx);

        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while governor
            .state
            .lock()
            .unwrap()
            .queues
            .iter()
            .map(VecDeque::len)
            .sum::<usize>()
            != 3
        {
            assert!(
                std::time::Instant::now() < deadline,
                "all priority waiters should queue before permits are released"
            );
            std::thread::yield_now();
        }

        drop(first_foreground);
        assert_eq!(
            started_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            "foreground",
            "a queued current-page request must start before queued prefetch/cover work"
        );
        drop(first_cover);
    }

    struct BlockingDoc {
        page1_calls: Arc<AtomicUsize>,
        page1_started: mpsc::Sender<()>,
        release_page1: Arc<(Mutex<bool>, Condvar)>,
    }

    /// D7（2026-09-21）：显示宽度必须 (a) 真的传给文档的"按显示尺寸渲染"入口，
    /// (b) 让不同宽度的页落在**不同的磁盘目录**（互不污染、也不作废标准档已有缓存）。
    struct WidthDoc {
        widths: Arc<std::sync::Mutex<Vec<Option<u32>>>>,
    }

    impl crate::document::Document for WidthDoc {
        fn page_count(&self) -> u32 {
            2
        }
        fn page_bytes(&self, index: u32) -> anyhow::Result<Vec<u8>> {
            self.widths.lock().unwrap().push(None);
            Ok(vec![index as u8; 4])
        }
        fn page_bytes_for_display(&self, index: u32, target_width: u32) -> anyhow::Result<Vec<u8>> {
            self.widths.lock().unwrap().push(Some(target_width));
            Ok(vec![index as u8; 8])
        }
    }

    #[test]
    fn display_width_is_forwarded_and_partitions_the_page_cache() {
        let widths = Arc::new(std::sync::Mutex::new(Vec::new()));
        let disk_dir = std::env::temp_dir().join(format!(
            "rch_reader_width_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&disk_dir).unwrap();
        let reader = Arc::new(Reader {
            book: Box::new(WidthDoc {
                widths: Arc::clone(&widths),
            }),
            cache: Mutex::new(Lru::new(CACHE_CAP)),
            inflight: Mutex::new(HashSet::new()),
            inflight_done: Condvar::new(),
            disk_dir: disk_dir.clone(),
            display_width: std::sync::atomic::AtomicU32::new(0),
            governor: Arc::new(BlockingRequestGovernor::new(4, 16)),
        });

        // 标准档（宽度 0）：走 `page_bytes`，落在历史目录。
        reader.get_page(0).unwrap();
        assert_eq!(widths.lock().unwrap().as_slice(), &[None]);
        assert!(disk_dir.join("0.bin").exists(), "标准档沿用历史布局");

        // 省流档：走 `page_bytes_for_display(1080)`，落进 w1080 子目录。
        reader.set_display_width(1080);
        reader.get_page(1).unwrap();
        assert_eq!(widths.lock().unwrap().as_slice(), &[None, Some(1080)]);
        assert!(disk_dir.join("w1080").join("1.bin").exists());
        assert!(!disk_dir.join("1.bin").exists(), "不得与标准档混用同一目录");

        // 换宽度必须清空 L1：否则会拿旧尺寸的页当新尺寸用。
        reader.set_display_width(1600);
        assert!(!reader.cache.lock().unwrap().contains(&1));
        reader.get_page(1).unwrap();
        assert!(disk_dir.join("w1600").join("1.bin").exists());
        assert!(disk_dir.join("w1080").join("1.bin").exists(), "旧宽度缓存保留，互不干扰");

        let _ = std::fs::remove_dir_all(disk_dir);
    }

    #[test]
    fn disk_hit_does_not_wait_for_network_permits() {
        let (started_tx, _) = mpsc::channel();
        let disk_dir = std::env::temp_dir().join(format!(
            "rch_reader_disk_priority_{}", SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&disk_dir).unwrap();
        std::fs::write(disk_dir.join("0.bin"), [42]).unwrap();
        let governor = Arc::new(BlockingRequestGovernor::new(2, 8));
        let first = governor.acquire(RequestPriority::Foreground).unwrap();
        let second = governor.acquire(RequestPriority::Foreground).unwrap();
        let reader = Arc::new(Reader {
            book: Box::new(BlockingDoc {
                page1_calls: Arc::new(AtomicUsize::new(0)),
                page1_started: started_tx,
                release_page1: Arc::new((Mutex::new(true), Condvar::new())),
            }),
            cache: Mutex::new(Lru::new(CACHE_CAP)),
            inflight: Mutex::new(HashSet::new()),
            inflight_done: Condvar::new(),
            disk_dir: disk_dir.clone(),
            display_width: std::sync::atomic::AtomicU32::new(0),
            governor: Arc::clone(&governor),
        });
        let (tx, rx) = mpsc::channel();
        let handle = std::thread::spawn(move || tx.send(reader.load_or_wait(0)).unwrap());
        let cached = rx.recv_timeout(Duration::from_secs(2));
        // Always release/join before asserting so a failure cannot leak a blocked worker.
        drop(first);
        drop(second);
        handle.join().unwrap();
        std::fs::remove_dir_all(disk_dir).unwrap();
        assert_eq!(&**cached.expect("disk hit queued behind network I/O").unwrap(), &[42]);
    }

    impl Document for BlockingDoc {
        fn page_count(&self) -> u32 {
            4
        }

        fn metadata(&self) -> DocumentMeta {
            DocumentMeta {
                title: "blocking-test".to_string(),
                ..Default::default()
            }
        }

        fn page_bytes(&self, index: u32) -> Result<Vec<u8>> {
            if index == 1 {
                self.page1_calls.fetch_add(1, Ordering::SeqCst);
                let _ = self.page1_started.send(());
                let (lock, cv) = &*self.release_page1;
                let mut released = lock.lock().unwrap();
                while !*released {
                    released = cv.wait(released).unwrap();
                }
            }
            Ok(vec![index as u8])
        }
    }

    #[test]
    fn foreground_does_not_duplicate_an_inflight_prefetch() {
        let page1_calls = Arc::new(AtomicUsize::new(0));
        let (started_tx, started_rx) = mpsc::channel();
        let release_page1 = Arc::new((Mutex::new(false), Condvar::new()));
        let doc = BlockingDoc {
            page1_calls: Arc::clone(&page1_calls),
            page1_started: started_tx,
            release_page1: Arc::clone(&release_page1),
        };
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let disk_dir = std::env::temp_dir().join(format!("rch_reader_inflight_{nonce}"));
        std::fs::create_dir_all(&disk_dir).unwrap();
        let reader = Arc::new(Reader {
            book: Box::new(doc),
            cache: Mutex::new(Lru::new(CACHE_CAP)),
            inflight: Mutex::new(HashSet::new()),
            inflight_done: Condvar::new(),
            disk_dir: disk_dir.clone(),
            display_width: std::sync::atomic::AtomicU32::new(0),
            governor: Arc::new(BlockingRequestGovernor::new(3, 8)),
        });

        reader.warm_up();
        started_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("warm-up should start page 1 prefetch");

        let foreground = {
            let reader = Arc::clone(&reader);
            std::thread::spawn(move || reader.get_page(1))
        };

        let duplicate_started = started_rx.recv_timeout(Duration::from_millis(150)).is_ok();
        {
            let (lock, cv) = &*release_page1;
            *lock.lock().unwrap() = true;
            cv.notify_all();
        }

        let bytes = foreground
            .join()
            .expect("foreground worker should not panic")
            .expect("foreground page load should succeed");
        assert_eq!(&*bytes, &[1]);
        assert!(
            !duplicate_started,
            "foreground load duplicated page 1 while warm-up prefetch was already in flight"
        );
        assert_eq!(page1_calls.load(Ordering::SeqCst), 1);

        let mut inflight = reader.inflight.lock().unwrap();
        while !inflight.is_empty() {
            let (guard, timeout) = reader
                .inflight_done
                .wait_timeout(inflight, Duration::from_secs(2))
                .unwrap();
            inflight = guard;
            assert!(!timeout.timed_out(), "background prefetch should finish");
        }
        drop(inflight);
        let _ = std::fs::remove_dir_all(disk_dir);
    }

    #[test]
    fn real_foreground_read_waits_for_a_governor_permit() {
        let governor = Arc::new(BlockingRequestGovernor::new(3, 8));
        let held = [
            governor.acquire(RequestPriority::Foreground).unwrap(),
            governor.acquire(RequestPriority::Foreground).unwrap(),
            governor.acquire(RequestPriority::Foreground).unwrap(),
        ];
        let (started_tx, started_rx) = mpsc::channel();
        let disk_dir = std::env::temp_dir().join(format!(
            "rch_reader_foreground_governor_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        ));
        std::fs::create_dir_all(&disk_dir).unwrap();
        let reader = Arc::new(Reader {
            book: Box::new(NotifyDoc(started_tx)),
            cache: Mutex::new(Lru::new(CACHE_CAP)),
            inflight: Mutex::new(HashSet::new()),
            inflight_done: Condvar::new(),
            disk_dir: disk_dir.clone(),
            display_width: std::sync::atomic::AtomicU32::new(0),
            governor: Arc::clone(&governor),
        });
        let worker = std::thread::spawn(move || reader.get_page(0));
        assert!(started_rx.recv_timeout(Duration::from_millis(100)).is_err());
        drop(held);
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        worker.join().unwrap().unwrap();
        let _ = std::fs::remove_dir_all(disk_dir);
    }

    #[test]
    fn real_prefetch_respects_background_reservation() {
        let governor = Arc::new(BlockingRequestGovernor::new(3, 8));
        let scan_a = governor.acquire(RequestPriority::Scan).unwrap();
        let scan_b = governor.acquire(RequestPriority::Scan).unwrap();
        let (started_tx, started_rx) = mpsc::channel();
        let disk_dir = std::env::temp_dir().join(format!(
            "rch_reader_governor_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        ));
        std::fs::create_dir_all(&disk_dir).unwrap();
        let reader = Arc::new(Reader {
            book: Box::new(NotifyDoc(started_tx)),
            cache: Mutex::new(Lru::new(CACHE_CAP)),
            inflight: Mutex::new(HashSet::new()),
            inflight_done: Condvar::new(),
            disk_dir: disk_dir.clone(),
            display_width: std::sync::atomic::AtomicU32::new(0),
            governor: Arc::clone(&governor),
        });
        reader.warm_up();
        assert!(started_rx.recv_timeout(Duration::from_millis(100)).is_err());
        drop(scan_a);
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        drop(scan_b);
        let _ = std::fs::remove_dir_all(disk_dir);
    }

    struct NotifyDoc(mpsc::Sender<()>);

    impl Document for NotifyDoc {
        fn page_count(&self) -> u32 {
            2
        }
        fn metadata(&self) -> DocumentMeta {
            DocumentMeta::default()
        }
        fn page_bytes(&self, _index: u32) -> Result<Vec<u8>> {
            let _ = self.0.send(());
            Ok(vec![1])
        }
    }
}
