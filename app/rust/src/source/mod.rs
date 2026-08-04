//! 书源字节访问抽象:流式阅读的基石。
//!
//! 任何来源(本地 / WebDAV / 未来的网盘)都统一抽象为
//! “支持 Range 随机访问的只读字节流”。格式解析器只面向 [`ByteSource`]
//! 编程,不关心底层是本地文件还是远程服务器。

use std::io::{self, Read, Seek, SeekFrom};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub mod local;
pub mod baidu;
pub mod cloud115;
pub mod quark;
pub mod sftp;
pub mod webdav;

/// 简单 API 节流器（网盘开放平台有频率限制，如 115 建议 1 r/s）。
/// 持锁期间 sleep，天然串行化同一客户端的 API 调用。
pub(crate) struct RateGate {
    last: Mutex<Instant>,
    interval: Duration,
}

impl RateGate {
    pub fn new(per_sec: f64) -> Self {
        let interval = if per_sec > 0.0 {
            Duration::from_secs_f64(1.0 / per_sec)
        } else {
            Duration::ZERO
        };
        RateGate {
            last: Mutex::new(Instant::now()),
            interval,
        }
    }

    /// 距上次调用不足间隔则 sleep 补齐；返回后可以发请求。
    pub fn wait(&self) {
        if self.interval.is_zero() {
            return;
        }
        let mut last = self.last.lock().unwrap();
        let now = Instant::now();
        let elapsed = now.saturating_duration_since(*last);
        if elapsed < self.interval {
            std::thread::sleep(self.interval - elapsed);
        }
        *last = Instant::now();
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

/// 读放大块大小:连续小块 read 时一次多读,减少底层(尤其远程)请求次数。
const READ_AHEAD: u64 = 256 * 1024;

/// 把 [`ByteSource`] 适配成 std 的 [`Read`] + [`Seek`],供 zip 等同步解析器使用。
/// 内置读放大缓存:zip 解析(如中心目录)的小块连续 read 会合并成少量大块请求。
pub struct SourceReader<S: ByteSource> {
    src: S,
    pos: u64,
    len: u64,
    buf_start: u64,
    buf: Vec<u8>,
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
        }
    }
    pub fn into_inner(self) -> S {
        self.src
    }
}

impl<S: ByteSource> Read for SourceReader<S> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if self.pos >= self.len {
            return Ok(0);
        }
        let end = self.pos + out.len() as u64;
        // 请求区间是否完整落在读缓存内。
        let hit = !self.buf.is_empty()
            && self.pos >= self.buf_start
            && end <= self.buf_start + self.buf.len() as u64;
        if !hit {
            let size = READ_AHEAD.max(out.len() as u64).min(self.len - self.pos);
            let mut data = vec![0u8; size as usize];
            let n = self.src.read_at(self.pos, &mut data)?;
            data.truncate(n);
            self.buf_start = self.pos;
            self.buf = data;
        }
        let s = (self.pos - self.buf_start) as usize;
        let avail = self.buf.len() - s;
        let n = avail.min(out.len());
        out[..n].copy_from_slice(&self.buf[s..s + n]);
        self.pos += n as u64;
        Ok(n)
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
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "seek 到负位置",
            ));
        }
        self.pos = new as u64;
        Ok(self.pos)
    }
}
