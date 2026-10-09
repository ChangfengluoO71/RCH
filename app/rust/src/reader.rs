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
type PageCacheKey = (u32, Option<u32>);

struct Lru {
    map: HashMap<PageCacheKey, Arc<Vec<u8>>>,
    order: VecDeque<PageCacheKey>,
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

    fn get(&mut self, key: &PageCacheKey) -> Option<Arc<Vec<u8>>> {
        if let Some(value) = self.map.get(key) {
            let value = Arc::clone(value);
            self.order.retain(|entry| entry != key);
            self.order.push_front(*key);
            Some(value)
        } else {
            None
        }
    }

    fn insert(&mut self, key: PageCacheKey, value: Arc<Vec<u8>>) {
        if self.map.contains_key(&key) {
            self.order.retain(|entry| entry != &key);
        } else if self.map.len() >= self.cap {
            if let Some(old) = self.order.pop_back() {
                self.map.remove(&old);
            }
        }
        self.order.push_front(key);
        self.map.insert(key, value);
    }
}

const MAX_PAGE_RENDER_WIDTH: u32 = 8192;

fn normalize_target_width(target_width: Option<u32>) -> Option<u32> {
    target_width
        .filter(|width| *width > 0)
        .map(|width| width.min(MAX_PAGE_RENDER_WIDTH))
}

/// A reading session with width-specific L1/L2 caches and lazy prefetch.
pub struct Reader {
    book: Box<dyn Document>,
    cache: Mutex<Lru>,
    inflight: Mutex<HashSet<PageCacheKey>>,
    prefetch_scheduled: Mutex<HashSet<PageCacheKey>>,
    inflight_done: Condvar,
    disk_dir: PathBuf,
    governor: Arc<BlockingRequestGovernor>,
}

impl Reader {
    /// `cache_ns` is the stable namespace for this book's disk cache.
    pub fn new(book: Box<dyn Document>, cache_ns: &str) -> Self {
        let disk_dir = crate::cache::CacheDir::Page
            .ensure()
            .ok()
            .unwrap_or_else(|| {
                let path = crate::cache::cache_root()
                    .join("cache")
                    .join("page")
                    .join(crate::cache::stable_hash(cache_ns));
                let _ = std::fs::create_dir_all(&path);
                path
            });

        let dir = disk_dir.join(crate::cache::stable_hash(cache_ns));
        let _ = std::fs::create_dir_all(&dir);
        Reader {
            book,
            cache: Mutex::new(Lru::new(CACHE_CAP)),
            inflight: Mutex::new(HashSet::new()),
            prefetch_scheduled: Mutex::new(HashSet::new()),
            inflight_done: Condvar::new(),
            disk_dir: dir,
            governor: blocking_request_governor(),
        }
    }

    pub fn page_count(&self) -> u32 {
        self.book.page_count()
    }

    pub fn page_dimensions(&self, index: u32) -> Result<Option<(u32, u32)>> {
        self.book.page_dimensions(index)
    }

    pub fn title(&self) -> String {
        self.book.metadata().title
    }

    /// Read a source page using the historical/default render width.
    pub fn get_page(self: &Arc<Self>, index: u32) -> Result<Arc<Vec<u8>>> {
        self.get_page_with_width(index, None)
    }

    /// Read a source page using a request-scoped render width.
    pub fn get_page_with_width(
        self: &Arc<Self>,
        index: u32,
        target_width: Option<u32>,
    ) -> Result<Arc<Vec<u8>>> {
        let target_width = normalize_target_width(target_width);
        let key = (index, target_width);
        let span = crate::perf::Span::new("reader.get_page")
            .field_u64("index", index as u64)
            .field_u64("target_width", u64::from(target_width.unwrap_or(0)));
        let cached = { self.cache.lock().unwrap().get(&key) };
        if let Some(bytes) = cached {
            crate::perf::bump(crate::perf::Counter::PageMemoryHits);
            span.field_str("source", "l1").end();
            self.spawn_prefetch(index, target_width);
            return Ok(bytes);
        }

        let bytes = self.load_or_wait(index, target_width, RequestPriority::Foreground)?;
        span.field_str("source", "load").end();
        self.spawn_prefetch(index, target_width);
        Ok(bytes)
    }

    /// Low-priority read using the historical/default render width.
    pub fn get_page_prefetch(&self, index: u32) -> Result<Arc<Vec<u8>>> {
        self.get_page_prefetch_with_width(index, None)
    }

    /// Low-priority read using a request-scoped render width.
    pub fn get_page_prefetch_with_width(
        &self,
        index: u32,
        target_width: Option<u32>,
    ) -> Result<Arc<Vec<u8>>> {
        let target_width = normalize_target_width(target_width);
        let key = (index, target_width);
        let span = crate::perf::Span::new("reader.get_page_prefetch")
            .field_u64("index", index as u64)
            .field_u64("target_width", u64::from(target_width.unwrap_or(0)));
        if let Some(bytes) = self.cache.lock().unwrap().get(&key) {
            crate::perf::bump(crate::perf::Counter::PageMemoryHits);
            span.field_str("source", "l1").end();
            return Ok(bytes);
        }
        let bytes = self.load_or_wait(index, target_width, RequestPriority::Prefetch)?;
        span.field_str("source", "load").end();
        Ok(bytes)
    }

    /// Start lazy prefetch around the first page using the legacy/default size.
    pub fn warm_up(self: &Arc<Self>) {
        self.spawn_prefetch(0, None);
    }

    fn load_or_wait(
        &self,
        index: u32,
        target_width: Option<u32>,
        priority: RequestPriority,
    ) -> Result<Arc<Vec<u8>>> {
        let target_width = normalize_target_width(target_width);
        let key = (index, target_width);
        loop {
            let mut inflight = self.inflight.lock().unwrap();
            while inflight.contains(&key) {
                inflight = self.inflight_done.wait(inflight).unwrap();
            }
            if let Some(bytes) = self.cache.lock().unwrap().get(&key) {
                return Ok(bytes);
            }
            drop(inflight);

            let disk_enter = Instant::now();
            if let Some(bytes) = self.disk_get(index, target_width) {
                let mut inflight = self.inflight.lock().unwrap();
                while inflight.contains(&key) {
                    inflight = self.inflight_done.wait(inflight).unwrap();
                }
                if let Some(bytes) = self.cache.lock().unwrap().get(&key) {
                    return Ok(bytes);
                }
                let bytes = Arc::new(bytes);
                self.cache.lock().unwrap().insert(key, Arc::clone(&bytes));
                crate::perf::bump(crate::perf::Counter::PageLoads);
                crate::perf::bump(crate::perf::Counter::PageDiskHits);
                crate::perf::observe_us(
                    crate::perf::Counter::PageLoadUsTotal,
                    crate::perf::Counter::PageLoadUsMax,
                    disk_enter.elapsed().as_micros() as u64,
                );
                return Ok(bytes);
            }

            let governor_enter = Instant::now();
            let permit = self.governor.acquire(priority)?;
            let mut inflight = self.inflight.lock().unwrap();
            if inflight.contains(&key) {
                drop(inflight);
                drop(permit);
                let mut inflight = self.inflight.lock().unwrap();
                while inflight.contains(&key) {
                    inflight = self.inflight_done.wait(inflight).unwrap();
                }
                continue;
            }
            if let Some(bytes) = self.cache.lock().unwrap().get(&key) {
                return Ok(bytes);
            }

            inflight.insert(key);
            drop(inflight);
            return self.load_claimed(index, target_width, key, priority, permit, governor_enter);
        }
    }

    fn load_claimed(
        &self,
        index: u32,
        target_width: Option<u32>,
        key: PageCacheKey,
        priority: RequestPriority,
        _permit: BlockingRequestPermit<'_>,
        governor_enter: Instant,
    ) -> Result<Arc<Vec<u8>>> {
        use crate::perf::{add, bump, observe_us, Counter};
        let span = crate::perf::Span::new("reader.load_claimed")
            .field_u64("index", index as u64)
            .field_u64("target_width", u64::from(target_width.unwrap_or(0)))
            .field_str("priority", format!("{priority:?}"));
        let mut disk_hit = false;
        let mut governor_wait_us = 0_u64;
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if let Some(bytes) = self.disk_get(index, target_width) {
                disk_hit = true;
                return Ok(Arc::new(bytes));
            }
            if priority == RequestPriority::Foreground {
                note_foreground_read();
            }
            governor_wait_us = governor_enter.elapsed().as_micros() as u64;
            crate::source::gate::with_priority(priority, || self.read_page(index, target_width))
        }));
        bump(Counter::PageLoads);
        if disk_hit {
            bump(Counter::PageDiskHits);
        }
        observe_us(
            Counter::PageLoadUsTotal,
            Counter::PageLoadUsMax,
            span.elapsed_us(),
        );
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
                    self.cache.lock().unwrap().insert(key, Arc::clone(bytes));
                }
                inflight.remove(&key);
                self.inflight_done.notify_all();
                drop(inflight);
                result
            }
            Err(payload) => {
                let mut inflight = self.inflight.lock().unwrap_or_else(|p| p.into_inner());
                inflight.remove(&key);
                self.inflight_done.notify_all();
                drop(inflight);
                std::panic::resume_unwind(payload);
            }
        }
    }

    fn page_disk_path(&self, index: u32, target_width: Option<u32>) -> PathBuf {
        let dir = match normalize_target_width(target_width) {
            None => self.disk_dir.clone(),
            Some(width) => self.disk_dir.join(format!("w{width}")),
        };
        dir.join(format!("{index}.bin"))
    }

    fn read_page(&self, index: u32, target_width: Option<u32>) -> Result<Arc<Vec<u8>>> {
        if let Some(bytes) = self.disk_get(index, target_width) {
            return Ok(Arc::new(bytes));
        }
        let bytes = match normalize_target_width(target_width) {
            None => self.book.page_bytes(index)?,
            Some(width) => self.book.page_bytes_for_display(index, width)?,
        };
        self.disk_put(index, target_width, &bytes);
        Ok(Arc::new(bytes))
    }

    fn disk_get(&self, index: u32, target_width: Option<u32>) -> Option<Vec<u8>> {
        std::fs::read(self.page_disk_path(index, target_width)).ok()
    }

    fn disk_put(&self, index: u32, target_width: Option<u32>, data: &[u8]) {
        let path = self.page_disk_path(index, target_width);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(path, data);
    }

    fn spawn_prefetch(self: &Arc<Self>, index: u32, target_width: Option<u32>) {
        let target_width = normalize_target_width(target_width);
        let count = self.page_count() as i64;
        for off in -PREFETCH_RADIUS..=PREFETCH_RADIUS {
            if off == 0 {
                continue;
            }
            let target = index as i64 + off;
            if target < 0 || target >= count {
                continue;
            }
            let target = target as u32;
            let key = (target, target_width);
            if !self.prefetch_scheduled.lock().unwrap().insert(key) {
                continue;
            }
            let reader = Arc::clone(self);
            std::thread::spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let _ = reader.load_or_wait(target, target_width, RequestPriority::Prefetch);
                }));
                reader.prefetch_scheduled.lock().unwrap().remove(&key);
                if let Err(payload) = result {
                    std::panic::resume_unwind(payload);
                }
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

    impl Document for WidthDoc {
        fn page_count(&self) -> u32 {
            2
        }
        fn page_bytes(&self, index: u32) -> anyhow::Result<Vec<u8>> {
            self.widths.lock().unwrap().push(None);
            Ok(vec![index as u8; 4])
        }
        fn page_bytes_for_display(
            &self,
            _index: u32,
            target_width: u32,
        ) -> anyhow::Result<Vec<u8>> {
            self.widths.lock().unwrap().push(Some(target_width));
            Ok(vec![target_width as u8; 8])
        }
    }

    #[test]
    fn request_width_partitions_memory_and_disk_caches_and_keeps_legacy_path() {
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
            prefetch_scheduled: Mutex::new(HashSet::new()),
            inflight_done: Condvar::new(),
            disk_dir: disk_dir.clone(),
            governor: Arc::new(BlockingRequestGovernor::new(4, 16)),
        });

        let standard = reader.get_page_prefetch(0).unwrap();
        assert_eq!(&**standard, &[0; 4]);
        assert_eq!(widths.lock().unwrap().as_slice(), &[None]);
        assert!(
            disk_dir.join("0.bin").exists(),
            "None keeps the legacy cache path"
        );

        let narrow = reader.get_page_prefetch_with_width(0, Some(1080)).unwrap();
        assert_eq!(&**narrow, &[56; 8]);
        assert!(disk_dir.join("w1080").join("0.bin").exists());

        let wide = reader.get_page_prefetch_with_width(0, Some(1600)).unwrap();
        assert_eq!(&**wide, &[64; 8]);
        assert!(disk_dir.join("w1600").join("0.bin").exists());
        assert_eq!(
            widths.lock().unwrap().as_slice(),
            &[None, Some(1080), Some(1600)],
            "the same source page must be rendered independently at each width"
        );

        *reader.cache.lock().unwrap() = Lru::new(CACHE_CAP);
        assert_eq!(
            &**reader.get_page_prefetch_with_width(0, Some(1080)).unwrap(),
            &[56; 8]
        );
        assert_eq!(
            &**reader.get_page_prefetch_with_width(0, Some(1600)).unwrap(),
            &[64; 8]
        );
        assert_eq!(
            widths.lock().unwrap().as_slice(),
            &[None, Some(1080), Some(1600)]
        );

        std::fs::write(disk_dir.join("1.bin"), [99]).unwrap();
        assert_eq!(&**reader.get_page_prefetch(1).unwrap(), &[99]);
        assert_eq!(
            widths.lock().unwrap().as_slice(),
            &[None, Some(1080), Some(1600)]
        );

        let _ = std::fs::remove_dir_all(disk_dir);
    }

    #[test]
    fn foreground_prefetch_keeps_the_requested_render_width() {
        let widths = Arc::new(Mutex::new(Vec::new()));
        let disk_dir = std::env::temp_dir().join(format!(
            "rch_reader_width_prefetch_{}",
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
            prefetch_scheduled: Mutex::new(HashSet::new()),
            inflight_done: Condvar::new(),
            disk_dir: disk_dir.clone(),
            governor: Arc::new(BlockingRequestGovernor::new(3, 8)),
        });

        assert_eq!(
            &**reader.get_page_with_width(0, Some(1080)).unwrap(),
            &[56; 8]
        );
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            let requested_widths = widths.lock().unwrap();
            if requested_widths
                .iter()
                .filter(|width| **width == Some(1080))
                .count()
                == 2
            {
                break;
            }
            drop(requested_widths);
            assert!(
                std::time::Instant::now() < deadline,
                "neighbor prefetch should use the foreground request width"
            );
            std::thread::sleep(Duration::from_millis(5));
        }

        assert_eq!(
            &**reader.get_page_prefetch_with_width(1, Some(1080)).unwrap(),
            &[56; 8]
        );
        assert_eq!(
            &**reader.get_page_prefetch_with_width(1, Some(1600)).unwrap(),
            &[64; 8]
        );
        assert_eq!(
            widths.lock().unwrap().as_slice(),
            &[Some(1080), Some(1080), Some(1600)]
        );
        let _ = std::fs::remove_dir_all(disk_dir);
    }

    struct BlockingWidthDoc {
        first_width_started: mpsc::Sender<()>,
        release_first_width: Arc<(Mutex<bool>, Condvar)>,
    }

    impl Document for BlockingWidthDoc {
        fn page_count(&self) -> u32 {
            1
        }

        fn metadata(&self) -> DocumentMeta {
            DocumentMeta::default()
        }

        fn page_bytes(&self, _index: u32) -> Result<Vec<u8>> {
            Ok(vec![0])
        }

        fn page_bytes_for_display(&self, _index: u32, width: u32) -> Result<Vec<u8>> {
            if width == 1080 {
                let _ = self.first_width_started.send(());
                let (lock, cv) = &*self.release_first_width;
                let mut released = lock.lock().unwrap();
                while !*released {
                    released = cv.wait(released).unwrap();
                }
            }
            Ok(vec![width as u8])
        }
    }

    #[test]
    fn concurrent_prefetches_for_same_page_at_different_widths_do_not_coalesce() {
        let (started_tx, started_rx) = mpsc::channel();
        let release_first_width = Arc::new((Mutex::new(false), Condvar::new()));
        let disk_dir = std::env::temp_dir().join(format!(
            "rch_reader_width_inflight_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&disk_dir).unwrap();
        let reader = Arc::new(Reader {
            book: Box::new(BlockingWidthDoc {
                first_width_started: started_tx,
                release_first_width: Arc::clone(&release_first_width),
            }),
            cache: Mutex::new(Lru::new(CACHE_CAP)),
            inflight: Mutex::new(HashSet::new()),
            prefetch_scheduled: Mutex::new(HashSet::new()),
            inflight_done: Condvar::new(),
            disk_dir: disk_dir.clone(),
            governor: Arc::new(BlockingRequestGovernor::new(3, 8)),
        });

        let first = {
            let reader = Arc::clone(&reader);
            std::thread::spawn(move || reader.get_page_prefetch_with_width(0, Some(1080)))
        };
        started_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("1080px render should start and block");

        let (second_tx, second_rx) = mpsc::channel();
        let second = {
            let reader = Arc::clone(&reader);
            std::thread::spawn(move || {
                let _ = second_tx.send(reader.get_page_prefetch_with_width(0, Some(1600)));
            })
        };
        let second_finished_before_release = second_rx.recv_timeout(Duration::from_millis(300));

        {
            let (lock, cv) = &*release_first_width;
            *lock.lock().unwrap() = true;
            cv.notify_all();
        }
        let first_bytes = first.join().unwrap().unwrap();
        second.join().unwrap();
        let second_bytes = second_finished_before_release
            .expect("1600px request must not wait for the same page at 1080px")
            .unwrap();
        assert_eq!(&*first_bytes, &[56]);
        assert_eq!(&*second_bytes, &[64]);
        let _ = std::fs::remove_dir_all(disk_dir);
    }

    #[test]
    fn document_without_dimensions_reports_none() {
        assert_eq!(
            BlockingWidthDoc {
                first_width_started: mpsc::channel().0,
                release_first_width: Arc::new((Mutex::new(true), Condvar::new())),
            }
            .page_dimensions(0)
            .unwrap(),
            None
        );
    }

    #[test]
    fn reader_exposes_document_page_dimensions() {
        struct DimensionsDoc;

        impl Document for DimensionsDoc {
            fn page_count(&self) -> u32 {
                1
            }
            fn metadata(&self) -> DocumentMeta {
                DocumentMeta::default()
            }
            fn page_bytes(&self, _index: u32) -> Result<Vec<u8>> {
                Ok(vec![])
            }
            fn page_dimensions(&self, _index: u32) -> Result<Option<(u32, u32)>> {
                Ok(Some((2400, 1600)))
            }
        }

        let disk_dir = std::env::temp_dir().join(format!(
            "rch_reader_dimensions_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&disk_dir).unwrap();
        let reader = Reader {
            book: Box::new(DimensionsDoc),
            cache: Mutex::new(Lru::new(CACHE_CAP)),
            inflight: Mutex::new(HashSet::new()),
            prefetch_scheduled: Mutex::new(HashSet::new()),
            inflight_done: Condvar::new(),
            disk_dir: disk_dir.clone(),
            governor: Arc::new(BlockingRequestGovernor::new(3, 8)),
        };

        assert_eq!(reader.page_dimensions(0).unwrap(), Some((2400, 1600)));
        let _ = std::fs::remove_dir_all(disk_dir);
    }

    #[test]
    fn disk_hit_does_not_wait_for_network_permits() {
        let (started_tx, _) = mpsc::channel();
        let disk_dir = std::env::temp_dir().join(format!(
            "rch_reader_disk_priority_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
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
            prefetch_scheduled: Mutex::new(HashSet::new()),
            inflight_done: Condvar::new(),
            disk_dir: disk_dir.clone(),
            governor: Arc::clone(&governor),
        });
        let (tx, rx) = mpsc::channel();
        let handle = std::thread::spawn(move || {
            tx.send(reader.load_or_wait(0, None, RequestPriority::Foreground))
                .unwrap()
        });
        let cached = rx.recv_timeout(Duration::from_secs(2));
        // Always release/join before asserting so a failure cannot leak a blocked worker.
        drop(first);
        drop(second);
        handle.join().unwrap();
        std::fs::remove_dir_all(disk_dir).unwrap();
        assert_eq!(
            &**cached.expect("disk hit queued behind network I/O").unwrap(),
            &[42]
        );
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
            prefetch_scheduled: Mutex::new(HashSet::new()),
            inflight_done: Condvar::new(),
            disk_dir: disk_dir.clone(),
            governor: Arc::new(BlockingRequestGovernor::new(3, 8)),
        });

        reader.warm_up();
        started_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("warm-up should start page 1 prefetch");

        let foreground = {
            let reader = Arc::clone(&reader);
            std::thread::spawn(move || reader.get_page_with_width(1, None))
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
            prefetch_scheduled: Mutex::new(HashSet::new()),
            inflight_done: Condvar::new(),
            disk_dir: disk_dir.clone(),
            governor: Arc::clone(&governor),
        });
        let worker = std::thread::spawn(move || reader.get_page_with_width(0, None));
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
            prefetch_scheduled: Mutex::new(HashSet::new()),
            inflight_done: Condvar::new(),
            disk_dir: disk_dir.clone(),
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
