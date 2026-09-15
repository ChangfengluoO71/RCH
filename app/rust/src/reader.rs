//! 阅读会话:L1 内存缓存 + L2 磁盘缓存 + 后台并行预取。
//!
//! L1(内存 LRU)管"翻页零等待";L2(磁盘)管"重复阅读秒开"——
//! 读过的页字节写盘,下次打开同一本书(尤其 WebDAV)无需重新下载。

use crate::document::Document;
use anyhow::{bail, Result};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, OnceLock};

/// L1 内存缓存容量(原始页字节)。
const CACHE_CAP: usize = 24;
/// 预取半径(以当前页为中心,前后各预取的页数)。
const PREFETCH_RADIUS: i64 = 3;
const REQUEST_GOVERNOR_CAPACITY: usize = 3;
const REQUEST_GOVERNOR_QUEUE_CAPACITY: usize = 64;

/// Priority for blocking work that might make a remote document request.
///
/// The governor only chooses which queued work may start. Once the permit is
/// held, the underlying synchronous I/O remains non-cancellable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestPriority {
    Foreground,
    Prefetch,
    Cover,
    Scan,
}

impl RequestPriority {
    const fn queue_index(self) -> usize {
        match self {
            Self::Foreground => 0,
            Self::Prefetch => 1,
            Self::Cover => 2,
            Self::Scan => 3,
        }
    }

    const fn is_background(self) -> bool {
        !matches!(self, Self::Foreground)
    }
}

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
pub fn blocking_request_governor() -> &'static BlockingRequestGovernor {
    static GOVERNOR: OnceLock<BlockingRequestGovernor> = OnceLock::new();
    GOVERNOR.get_or_init(|| {
        BlockingRequestGovernor::new(REQUEST_GOVERNOR_CAPACITY, REQUEST_GOVERNOR_QUEUE_CAPACITY)
    })
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
        let cached = { self.cache.lock().unwrap().get(&index) };
        if let Some(bytes) = cached {
            self.spawn_prefetch(index);
            return Ok(bytes);
        }

        let bytes = self.load_or_wait(index)?;
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
            return self.load_claimed(index);
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
    fn load_claimed(&self, index: u32) -> Result<Arc<Vec<u8>>> {
        let outcome =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.read_page(index)));

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

    /// 读一页:L2 磁盘命中则直接用,否则从书源下载并写盘。
    fn read_page(&self, index: u32) -> Result<Arc<Vec<u8>>> {
        if let Some(bytes) = self.disk_get(index) {
            return Ok(Arc::new(bytes));
        }
        let bytes = self.book.page_bytes(index)?;
        self.disk_put(index, &bytes);
        Ok(Arc::new(bytes))
    }

    fn disk_get(&self, index: u32) -> Option<Vec<u8>> {
        std::fs::read(self.disk_dir.join(format!("{index}.bin"))).ok()
    }

    fn disk_put(&self, index: u32, data: &[u8]) {
        let _ = std::fs::write(self.disk_dir.join(format!("{index}.bin")), data);
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
                let _ = me.load_claimed(t);
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
        assert!(RequestPriority::Foreground.queue_index() < RequestPriority::Prefetch.queue_index());
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
        while governor.state.lock().unwrap().queues[RequestPriority::Scan.queue_index()].is_empty() {
            assert!(std::time::Instant::now() < deadline, "scan waiter did not queue");
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
}
