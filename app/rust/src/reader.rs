//! 阅读会话:L1 内存缓存 + L2 磁盘缓存 + 后台并行预取。
//!
//! L1(内存 LRU)管"翻页零等待";L2(磁盘)管"重复阅读秒开"——
//! 读过的页字节写盘,下次打开同一本书(尤其 WebDAV)无需重新下载。

use crate::document::Document;
use anyhow::Result;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// L1 内存缓存容量(原始页字节)。
const CACHE_CAP: usize = 24;
/// 预取半径(以当前页为中心,前后各预取的页数)。
const PREFETCH_RADIUS: i64 = 3;

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
    /// 正在后台预取的页(去重)。
    prefetching: Mutex<HashSet<u32>>,
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
            prefetching: Mutex::new(HashSet::new()),
            disk_dir: dir,
        }
    }

    pub fn page_count(&self) -> u32 {
        self.book.page_count()
    }

    pub fn title(&self) -> String {
        self.book.metadata().title
    }

    /// 获取一页:先 L1 内存,未命中走 read_page(L2 磁盘 / 下载);随后触发周边预取。
    pub fn get_page(self: &Arc<Self>, index: u32) -> Result<Arc<Vec<u8>>> {
        // 查 L1:锁的 guard 限于此块内,避免后续 spawn_prefetch 再加锁造成死锁。
        let cached = { self.cache.lock().unwrap().get(&index) };
        if let Some(bytes) = cached {
            self.spawn_prefetch(index);
            return Ok(bytes);
        }
        let bytes = self.read_page(index)?;
        {
            self.cache.lock().unwrap().insert(index, bytes.clone());
        }
        self.spawn_prefetch(index);
        Ok(bytes)
    }

    /// 打开书后立即预取开头若干页。
    pub fn warm_up(self: &Arc<Self>) {
        self.spawn_prefetch(0);
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

    fn read_and_cache(&self, index: u32) -> Result<()> {
        if self.cache.lock().unwrap().contains(&index) {
            return Ok(());
        }
        let bytes = self.read_page(index)?;
        self.cache.lock().unwrap().insert(index, bytes);
        Ok(())
    }

    fn disk_get(&self, index: u32) -> Option<Vec<u8>> {
        std::fs::read(self.disk_dir.join(format!("{index}.bin"))).ok()
    }

    fn disk_put(&self, index: u32, data: &[u8]) {
        let _ = std::fs::write(self.disk_dir.join(format!("{index}.bin")), data);
    }

    /// 后台并行预取 index 前后各 PREFETCH_RADIUS 页(去重、跳过已缓存)。
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
            if self.cache.lock().unwrap().contains(&t) {
                continue;
            }
            // 已在预取中则跳过(insert 返回 false 表示已存在)。
            if !self.prefetching.lock().unwrap().insert(t) {
                continue;
            }
            let me = Arc::clone(self);
            std::thread::spawn(move || {
                let _ = me.read_and_cache(t);
                me.prefetching.lock().unwrap().remove(&t);
            });
        }
    }
}
