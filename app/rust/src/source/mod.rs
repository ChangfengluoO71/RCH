//! 书源字节访问抽象:流式阅读的基石。
//!
//! 任何来源(本地 / WebDAV / 未来的网盘)都统一抽象为
//! “支持 Range 随机访问的只读字节流”。格式解析器只面向 [`ByteSource`]
//! 编程,不关心底层是本地文件还是远程服务器。

use std::io::{self, Read, Seek, SeekFrom};

pub mod baidu;
pub mod cloud115;
pub mod gate;
pub mod local;
pub mod quark;
pub mod sftp;
pub mod singleflight;
pub mod webdav;

pub(crate) use gate::{GateLimit, RateGate};

/// 记录一次门控等待，按通道归属到 API / CDN 桶，CDN 再按优先级分桶。
///
/// 分桶只影响埋点，不影响任何调度。调用方必须在取得许可后、且仍在同一线程上
/// 调用它，这样 `current_priority()` 才是这次等待的真实优先级。
pub(crate) fn record_gate_wait(channel: &str, waited_us: u64) {
    use crate::perf::{observe_us, Counter};
    if channel.ends_with(".cdn") {
        observe_us(
            Counter::CdnGateWaitUsTotal,
            Counter::CdnGateWaitUsMax,
            waited_us,
        );
        if gate::current_priority() == gate::RequestPriority::Foreground {
            observe_us(
                Counter::CdnGateWaitUsTotalForeground,
                Counter::CdnGateWaitUsMaxForeground,
                waited_us,
            );
        } else {
            observe_us(
                Counter::CdnGateWaitUsTotalBackground,
                Counter::CdnGateWaitUsMaxBackground,
                waited_us,
            );
        }
    } else {
        observe_us(
            Counter::ApiGateWaitUsTotal,
            Counter::ApiGateWaitUsMax,
            waited_us,
        );
    }
}

/// 目录条目(书架 / 浏览用)。
#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    /// 修改时间（unix 秒）；来源无此信息时为 0（如 WebDAV）。
    pub mtime: i64,
}

/// 统一可随机访问的只读字节源。
pub trait ByteSource: Send + Sync {
    /// 总字节数。
    fn len(&self) -> u64;
    /// 是否为空。
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// 从 `offset` 处读取,尽量填满 `buf`,返回实际读取字节数(0 表示 EOF)。
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize>;

    /// 从 `offset` 处精确读满 `buf`(循环 `read_at` 直至填满或 EOF)。
    fn read_exact_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        let mut filled = 0usize;
        while filled < buf.len() {
            let n = self.read_at(offset + filled as u64, &mut buf[filled..])?;
            if n == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "read_exact_at 提前到达 EOF",
                ));
            }
            filled += n;
        }
        Ok(())
    }
}

impl<S: ByteSource + ?Sized> ByteSource for std::sync::Arc<S> {
    fn len(&self) -> u64 { (**self).len() }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        (**self).read_at(offset, buf)
    }
}

/// 顺序读的预读块大小：连续小块 read 时一次多读，减少底层（尤其远程）请求次数。
///
/// 只用于**顺序延续**的读取。随机/元数据读取走下面的小窗口，不再被它放大。
///
/// 第 68 轮**试过**把它提到 1 MiB，被 `tests/p0b2_zip_read_amplification.rs` 当场否掉：
/// 读 8 页内容 2.46 MB 却传输 7.09 MB（2.9×）、单页读也要 1 MiB ✗
/// ⇒ 放大代价大于省下的往返次数。真正的杠杆是**减少重复的元数据读**（见下一轮），
/// 不是加大窗口。
const READ_AHEAD: u64 = 256 * 1024;
/// 连续顺序读达到阈值后把窗口提到这个大小（自适应）。
///
/// 依据（第 68 轮实测）：真实 ZIP 页约 1.3 MB，256 KiB 窗口 ⇒ 每页 ~5 次网络往返
/// （115 CDN 243 ms/次 ⇒ 每页 ~1.24 s）。但**不能写死大窗口**：P0-B2 的 300 KiB
/// 页夹具会被放大成 2.9×（门禁当场否掉）。
/// 因此窗口只在"连续顺序读"时增长，且小页（1–2 次读）**不会**触发。
const READ_AHEAD_MAX: u64 = 1024 * 1024;
/// 连续顺序读多少次后放大窗口。
const READ_AHEAD_GROW_AFTER: u32 = 3;
/// 非顺序（随机/元数据）读的最小取数长度。
/// 64 B 足以覆盖 ZIP 的 `ZipLocalEntryBlock`（30 B）与单个中央目录条目（46 B）。
const META_MIN_FETCH: u64 = 64;
/// 非顺序读的取数上限。小 metadata 读**不得**被放大成 256 KiB Range。
const META_MAX_FETCH: u64 = 16 * 1024;
/// 小于该长度的请求才可能走元数据小窗口。
///
/// ZIP metadata 读的实测请求长度：local header 30 B、中央目录条目 46 B。
/// 4 KiB 是宽松上限；正文读（`read_to_end` 一次 8 KiB 起）一律走大窗口，
/// 因此**不会**把顺序页数据切成小块。
const META_ASK_LIMIT: u64 = 4 * 1024;
/// 与"上一次读取结束位置"的邻近判定（字节）。
///
/// 落在该范围内视为顺序/邻近访问，继续沿用大窗口；只有真正散落的读才进小窗口。
const META_NEAR_SLACK: u64 = 4 * 1024;
/// 非顺序读的窗口槽数。
///
/// 2 已足够：ZIP 打开期只有两类交错访问——文件尾的 central directory 流与分散的
/// local header 读。命中会刷新 LRU 触碰，因此被反复使用的目录窗口不会被分散读淘汰。
///
/// **刻意不是通用缓存系统**：槽数固定、无淘汰策略配置、不增长、不参与淘汰统计。
const META_WINDOWS: usize = 2;

/// 窗口内数据的区间。
struct Window {
    start: u64,
    data: Vec<u8>,
    /// LRU 触碰时间戳（单调递增）。
    touched: u64,
}

impl Window {
    fn end(&self) -> u64 {
        self.start + self.data.len() as u64
    }

    fn contains(&self, start: u64, end: u64) -> bool {
        start >= self.start && end <= self.end()
    }
}

/// 把 [`ByteSource`] 适配成 std 的 [`Read`] + [`Seek`],供 zip 等同步解析器使用。
///
/// 缓存分两层，各自服务一种访问形态：
///
/// - **顺序大窗口**（`buf`）：正文顺序读，一次 `READ_AHEAD`，保持既有的"一页几次请求"。
/// - **非顺序小窗口**（`meta`）：解析器的随机 metadata 读（ZIP 的 local header /
///   中央目录条目），按请求长度小区间取数，并保留少量槽位避免两个交错流互相淘汰。
///
/// P0-B2 之前的实现只有一层大窗口，于是"30 字节的 local header 读"会被放大成
/// 256 KiB，且"中央目录 ↔ 分散 local header"的来回 seek 会让窗口持续失效。
pub struct SourceReader<S: ByteSource> {
    src: S,
    pos: u64,
    len: u64,
    /// 顺序读的大窗口。
    buf_start: u64,
    buf: Vec<u8>,
    /// 非顺序读的小窗口（固定槽数 + LRU 触碰）。
    meta: [Option<Window>; META_WINDOWS],
    meta_clock: u64,
    /// 上一次读取结束的位置；`pos` 与之相等即视为顺序延续。
    last_end: Option<u64>,
    /// 已连续顺序读的次数（达到 [`READ_AHEAD_GROW_AFTER`] 后放大窗口）。
    sequential_runs: u32,
    /// 这个 reader 还没有读过任何数据（全新 clone 的页读取器）。
    ///
    /// 必须与"`last_end` 为空"区分开：`seek` 之后 `last_end` 也为空，但**不能**
    /// 因此把 seek 后的第一个随机读当成顺序读 —— ZIP 的 `find_data_start` 每次都会
    /// seek，误判会让每条 local header 都被放大成 256 KiB（P0-B2 实测到的回归）。
    fresh: bool,
}

impl<S: ByteSource + Clone> Clone for SourceReader<S> {
    fn clone(&self) -> Self {
        // Independent cursor/window; share only the random-access source.
        // Archive clones must not duplicate the read-ahead buffer per page.
        Self {
            src: self.src.clone(),
            pos: self.pos,
            len: self.len,
            buf_start: 0,
            buf: Vec::new(),
            meta: std::array::from_fn(|_| None),
            meta_clock: 0,
            last_end: None,
            sequential_runs: 0,
            fresh: true,
        }
    }
}

impl<S: ByteSource> SourceReader<S> {
    pub fn new(src: S) -> Self {
        let len = src.len();
        SourceReader {
            src,
            pos: 0,
            len,
            buf_start: 0,
            buf: Vec::new(),
            meta: std::array::from_fn(|_| None),
            meta_clock: 0,
            last_end: None,
            sequential_runs: 0,
            fresh: true,
        }
    }

    pub fn into_inner(self) -> S {
        self.src
    }

    /// 在非顺序窗口里找命中区间并刷新 LRU 触碰。
    fn meta_hit(&mut self, start: u64, end: u64) -> Option<usize> {
        let index = self
            .meta
            .iter()
            .position(|slot| slot.as_ref().is_some_and(|w| w.contains(start, end)))?;
        self.meta_clock += 1;
        if let Some(window) = self.meta[index].as_mut() {
            window.touched = self.meta_clock;
        }
        Some(index)
    }

    /// 若这次读取紧邻某个已有元数据窗口，返回该窗口长度。
    ///
    /// 用途：ZIP 的中央目录条目是**顺序**的，但会被分散的 local header 读打断，
    /// 因此不能靠"紧接上一次读取结束位置"识别。改为按**空间邻近**识别：命中同一
    /// 区域时放大取数，让整个中央目录一次覆盖，从而消除反复重下。
    fn meta_region_len(&self, pos: u64, ask: u64) -> Option<usize> {
        let read_end = pos + ask;
        self.meta
            .iter()
            .filter_map(|slot| slot.as_ref())
            .find(|window| {
                let overlaps = pos < window.end() && read_end > window.start;
                let near_edge = pos.abs_diff(window.start) <= META_NEAR_SLACK
                    || pos.abs_diff(window.end()) <= META_NEAR_SLACK;
                overlaps || near_edge
            })
            .map(|window| window.data.len())
    }

    /// 写入非顺序窗口，淘汰最近最少使用的槽。
    fn meta_put(&mut self, start: u64, data: Vec<u8>) -> usize {
        self.meta_clock += 1;
        let empty = self.meta.iter().position(|slot| slot.is_none());
        let index = match empty {
            Some(index) => index,
            None => {
                let mut victim = 0;
                let mut oldest = u64::MAX;
                for (index, slot) in self.meta.iter().enumerate() {
                    if let Some(window) = slot {
                        if window.touched < oldest {
                            oldest = window.touched;
                            victim = index;
                        }
                    }
                }
                victim
            }
        };
        self.meta[index] = Some(Window {
            start,
            data,
            touched: self.meta_clock,
        });
        index
    }

    /// 从指定窗口拷出数据并推进游标。
    fn serve(
        &mut self,
        window_start: u64,
        window_len: usize,
        out: &mut [u8],
        from_meta: Option<usize>,
    ) -> usize {
        let offset = (self.pos - window_start) as usize;
        let avail = window_len - offset;
        let n = avail.min(out.len());
        match from_meta {
            Some(index) => {
                let data = &self.meta[index].as_ref().expect("meta window").data;
                out[..n].copy_from_slice(&data[offset..offset + n]);
            }
            None => {
                out[..n].copy_from_slice(&self.buf[offset..offset + n]);
            }
        }
        self.pos += n as u64;
        self.last_end = Some(window_start + window_len as u64);
        self.fresh = false;
        n
    }

    /// 一次底层取数（含 P0 埋点）。
    fn fetch(&self, size: u64, requested: usize) -> std::io::Result<Vec<u8>> {
        let mut data = vec![0u8; size as usize];
        // P0 埋点：每次窗口未命中 = 对远程源 1 个 Range 请求。这是
        // "每页 Range 次数" 的权威来源。
        let span = crate::perf::Span::new("source.read_at")
            .field_u64("offset", self.pos)
            .field_u64("len", size)
            .field_u64("requested", requested as u64);
        let n = self.src.read_at(self.pos, &mut data)?;
        span.field_u64("filled", n as u64).end();
        crate::perf::bump(crate::perf::Counter::SourceReadAt);
        crate::perf::add(crate::perf::Counter::SourceReadAtBytes, size);
        data.truncate(n);
        Ok(data)
    }
}

impl<S: ByteSource> Read for SourceReader<S> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if self.pos >= self.len || out.is_empty() {
            return Ok(0);
        }
        let end = self.pos + out.len() as u64;

        // 1) 顺序大窗口命中。
        if !self.buf.is_empty()
            && self.pos >= self.buf_start
            && end <= self.buf_start + self.buf.len() as u64
        {
            let start = self.buf_start;
            let len = self.buf.len();
            return Ok(self.serve(start, len, out, None));
        }

        // 2) 非顺序小窗口命中。
        if let Some(index) = self.meta_hit(self.pos, end) {
            let window = self.meta[index].as_ref().expect("meta window");
            let (start, len) = (window.start, window.data.len());
            return Ok(self.serve(start, len, out, Some(index)));
        }

        // 3) 未命中：按访问形态选择取数长度。
        //
        // 只有"紧接上一次读取结束位置"才算顺序延续；否则按随机/元数据读处理，
        // 不再把几十字节的解析器读放大成 256 KiB。
        // 只有"小且散"的读取才进元数据小窗口；其余一律沿用大窗口。
        //
        // P0-B2 实测踩到的两个反例（都写进注释防止回退）：
        // - 不能要求"大窗口非空"才算顺序：会让首次读走不到大窗口，正文读被切成小块
        //   （8 页从 16 次请求涨到 160 次）；
        // - 不能用 `last_end.is_none()` 代替 `fresh`：`seek` 之后 `last_end` 也是空，
        //   而 ZIP 的 `find_data_start` 每次都 seek，于是每条 local header 都被
        //   放大成 256 KiB（打开 40 页仍是 10 MiB）。
        let near_last = self
            .last_end
            .is_some_and(|end| self.pos.abs_diff(end) <= META_NEAR_SLACK);
        let meta_read = !self.fresh && !near_last && (out.len() as u64) < META_ASK_LIMIT;

        let want = if meta_read {
            match self.meta_region_len(self.pos, out.len() as u64) {
                // 同一访问流的延续：放大取数，让中央目录一次覆盖完。
                Some(previous_len) => (previous_len as u64 * 8)
                    .clamp(META_MIN_FETCH, META_MAX_FETCH)
                    .max(out.len() as u64),
                None => (out.len() as u64).clamp(META_MIN_FETCH, META_MAX_FETCH),
            }
        } else {
            // 保险（第 68 轮复测发现）：上一窗口**没被吃掉一半**就又要新窗口
            // ⇒ 说明顺序性不足（典型：大页之后紧跟一个小条目），立即回落到 256 KiB，
            // 不让 1 MiB 的窗口继续用在小读上。命中窗口的读不计入（它们不走这里）。
            if !self.buf.is_empty() {
                let consumed = self.pos.saturating_sub(self.buf_start);
                if consumed * 2 < self.buf.len() as u64 {
                    self.sequential_runs = 0;
                }
            }
            // 连续顺序读 ⇒ 逐步放大窗口；随机/首次读保持 256 KiB（小页不被放大）。
            if near_last || self.fresh {
                self.sequential_runs = self.sequential_runs.saturating_add(1);
            } else {
                self.sequential_runs = 1;
            }
            let window = if self.sequential_runs >= READ_AHEAD_GROW_AFTER {
                READ_AHEAD_MAX
            } else {
                READ_AHEAD
            };
            window.max(out.len() as u64)
        };
        let size = want.min(self.len - self.pos).max(1);
        let data = self.fetch(size, out.len())?;

        if !meta_read {
            self.buf_start = self.pos;
            self.buf = data;
            let start = self.buf_start;
            let len = self.buf.len();
            Ok(self.serve(start, len, out, None))
        } else {
            let index = self.meta_put(self.pos, data);
            let window = self.meta[index].as_ref().expect("meta window");
            let (start, len) = (window.start, window.data.len());
            Ok(self.serve(start, len, out, Some(index)))
        }
    }
}

impl<S: ByteSource> Seek for SourceReader<S> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let new = match pos {
            SeekFrom::Start(p) => p as i128,
            SeekFrom::End(p) => self.len as i128 + p as i128,
            SeekFrom::Current(p) => self.pos as i128 + p as i128,
        };
        if new < 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "seek 到负位置"));
        }
        self.pos = new as u64;
        // seek 之后不再存在"顺序延续"关系，但也**不是**全新 reader。
        self.last_end = None;
        self.fresh = false;
        Ok(self.pos)
    }
}
