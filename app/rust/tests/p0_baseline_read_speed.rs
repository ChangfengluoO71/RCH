//! P0-A：阅读速度的**机制级基线**测量装置。
//!
//! # 为什么需要这个装置
//!
//! P0 的验收要求"阅读延迟不再由旧的固定 4 QPS CDN 门控形成理论下限"。要证明
//! 这一点，必须测量**生产代码路径本身**，而不是另写一个近似模型。本装置因此：
//!
//! - 起一个本地 HTTP 服务，只提供 115 CDN 直链的**协议形状**（`Range: bytes=a-b`
//!   → 206 + `Content-Range`），不碰任何真实账号；
//! - 用 `Cloud115WebClient::read_range_url`（未改动的生产函数，含生产 `range_gate`）
//!   逐字节读取；
//! - 用生产 `Reader` → `ZipBook` → `SourceReader` → `ByteSource` 的完整链路取页，
//!   因此 L1/L2 缓存、inflight 去重、governor 优先级、预读块大小全部是真实行为。
//!
//! 也就是说：**只有端点换成了 localhost，其余全是生产代码**。
//!
//! # 边界（必须诚实标注）
//!
//! - 本装置不能替代真机 + 真实 115/夸克账号的端到端测量：它不覆盖 CDN 侧真实
//!   延迟分布、TLS、WAF、真实压缩包结构与真实页面大小分布。
//! - `downurl`（取链）端点需要 m115 加密响应，本装置无法伪造，因此
//!   "取链全局锁"的量化在 P0-C 通过可注入 fetch 的单元测试给出。
//!
//! # 运行
//!
//! ```text
//! cargo test --test p0_baseline_read_speed -- --nocapture --test-threads=1
//! ```
//!
//! 每个用例都会打印一行 `P0-BASELINE {...}` JSON，供 baseline → P0-B → P0-C → P0-D
//! 的同口径对比。

use rust_lib_app::document::open_document;
use rust_lib_app::perf::{self, Counter};
use rust_lib_app::reader::Reader;
use rust_lib_app::source::cloud115::Cloud115WebClient;
use rust_lib_app::source::gate::{with_priority, RequestPriority};
use rust_lib_app::source::ByteSource;
use std::io::{Read as _, Write as _};
use std::net::{SocketAddr, TcpListener};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use rust_lib_app::cache;
use rust_lib_app::api::source::{open_webdav_book, webdav_connect, webdav_disconnect};
use rust_lib_app::source::webdav::WebDavClient;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// 本地 mock CDN
// ---------------------------------------------------------------------------

#[derive(Default)]
struct MockState {
    /// 人为服务延迟（毫秒），用于模拟 CDN 往返。
    delay_ms: AtomicU64,
    /// 期望状态码覆盖：0 表示正常 206。
    force_status: AtomicU64,
    /// 收到的请求：[到达时刻相对进程启动的微秒, range 起点, range 长度]。
    arrivals: Mutex<Vec<(u64, u64, u64)>>,
    /// 服务端**真实**在途请求数与其峰值。这是 "in-flight 数" 的权威口径：
    /// 客户端侧的门控会先把并发压成串行，所以客户端仪表看不出真实并发。
    active: AtomicU64,
    max_overlap: AtomicU64,
    // ---- RG-A 扩展（默认值 ⇒ 既有 P0-A 行为完全不变）----
    /// 合成体总长（>0 ⇒ **不存储实体**，按偏移计算字节）；0 = 关闭。
    synthetic_len: AtomicU64,
    /// 慢滴块大小（0 = 不慢滴：实体一次性写出；合成体按 64KiB 分块零等待写出）。
    drip_bytes: AtomicU64,
    /// 慢滴块间间隔（毫秒）。
    drip_ms: AtomicU64,
    /// 忠实响应码：>0 且请求**无** Range 头时回 200 OK（而非 206）。
    faithful_status: AtomicU64,
    /// RG-A：中途断流 —— >0 时只发前 N 字节随后主动关闭连接（0 = 关闭该模式）。
    abort_after_bytes: AtomicU64,
    /// RG-A：**不支持 Range 的服务器** —— >0 时忽略 Range 头，一律 200 + 全量体。
    /// 这是 ADR-005 fallback 的触发条件（probe 收到 200 ⇒ unsupported）。
    no_range_support: AtomicU64,
    /// RG-A：按**请求类型**记录，用于区分 probe / full GET / 其它（含 method、是否带
    /// Range 头、请求路径），避免仅凭总请求数猜类型。
    requests: Mutex<Vec<(String, bool, String)>>,
    /// RG-A：**只**对"无 Range 头的 full GET"断流（A2-4 专用；Range probe 不受影响）。
    abort_after_bytes_on_full_get: AtomicU64,
    /// RG-A：206 但 `Content-Range` 非法（A2-6 fail-closed 专用）。
    malformed_content_range: AtomicU64,
}

struct MockCdn {
    addr: SocketAddr,
    state: Arc<MockState>,
    body: Arc<Vec<u8>>,
}

impl MockCdn {
    fn start(body: Vec<u8>, delay_ms: u64) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock cdn");
        let addr = listener.local_addr().expect("mock cdn addr");
        let state = Arc::new(MockState::default());
        state.delay_ms.store(delay_ms, Ordering::SeqCst);
        let body = Arc::new(body);

        let thread_state = Arc::clone(&state);
        let thread_body = Arc::clone(&body);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let state = Arc::clone(&thread_state);
                let body = Arc::clone(&thread_body);
                std::thread::spawn(move || {
                    handle_conn(&mut stream, &state, &body);
                });
            }
        });

        MockCdn { addr, state, body }
    }

    fn url(&self) -> String {
        format!("http://{}/file.cbz", self.addr)
    }

    fn len(&self) -> u64 {
        self.body.len() as u64
    }

    /// RG-A：**合成体** mock —— 零内存地提供一个任意长度的 206/200 端点。
    ///
    /// 用于 20 / 100 / 300 MiB 尺寸矩阵：本装置不分配实体字节，
    /// 而是按绝对偏移合成确定性字节。
    fn start_synthetic(total_len: u64, delay_ms: u64) -> Self {
        let cdn = MockCdn::start(Vec::new(), delay_ms);
        cdn.state.synthetic_len.store(total_len, Ordering::SeqCst);
        cdn.state.faithful_status.store(1, Ordering::SeqCst);
        cdn
    }

    /// RG-A：慢滴模式（块大小 + 块间间隔），用于证明客户端是**流式落盘**。
    fn with_drip(self, chunk_bytes: u64, gap_ms: u64) -> Self {
        self.state.drip_bytes.store(chunk_bytes, Ordering::SeqCst);
        self.state.drip_ms.store(gap_ms, Ordering::SeqCst);
        self
    }

    /// RG-A：中途断流模式 —— 响应头声明完整 `Content-Length`，但只发前 `bytes` 字节
    /// 就主动关闭连接（客户端应报"响应体不完整"，而不是 HTTP 状态分类失败）。
    fn with_abort_after(self, bytes: u64) -> Self {
        self.state.abort_after_bytes.store(bytes, Ordering::SeqCst);
        self
    }

    /// RG-A：运行期开关（0 = 关闭）。
    fn set_abort_after(&self, bytes: u64) {
        self.state.abort_after_bytes.store(bytes, Ordering::SeqCst);
    }

    /// RG-A：模拟**不支持 Range** 的 WebDAV 服务器（200 + 全量体，忽略 Range 头）。
    fn with_no_range_support(self) -> Self {
        self.state.no_range_support.store(1, Ordering::SeqCst);
        self
    }

    /// RG-A：只对 **full GET（无 Range 头）** 生效的断流（A2-4）。不影响 Range probe。
    fn with_abort_after_full_get(self, bytes: u64) -> Self {
        self.state
            .abort_after_bytes_on_full_get
            .store(bytes, Ordering::SeqCst);
        self
    }

    fn set_abort_after_full_get(&self, bytes: u64) {
        self.state
            .abort_after_bytes_on_full_get
            .store(bytes, Ordering::SeqCst);
    }

    /// RG-A：206 携带**非法** `Content-Range`（A2-6）。
    fn with_malformed_content_range(self) -> Self {
        self.state.malformed_content_range.store(1, Ordering::SeqCst);
        self
    }

    /// 请求类型记录（method、是否带 Range、路径）。
    fn requests(&self) -> Vec<(String, bool, String)> {
        self.state.requests.lock().unwrap().clone()
    }

    /// 计数：`method` 精确匹配；`ranged` = Some(true) 只数带 Range、Some(false) 只数不带、None 不限。
    fn request_count_of(&self, method: &str, ranged: Option<bool>) -> usize {
        self.requests()
            .into_iter()
            .filter(|(m, has_range, _)| {
                m.eq_ignore_ascii_case(method) && ranged.is_none_or(|want| want == *has_range)
            })
            .count()
    }

    fn clear_requests(&self) {
        self.state.requests.lock().unwrap().clear();
    }

    /// 有效总长（合成体或实体）。
    fn total_len(&self) -> u64 {
        let synthetic = self.state.synthetic_len.load(Ordering::SeqCst);
        if synthetic > 0 {
            synthetic
        } else {
            self.body.len() as u64
        }
    }

    fn arrivals(&self) -> Vec<(u64, u64, u64)> {
        self.state.arrivals.lock().unwrap().clone()
    }

    fn request_count(&self) -> usize {
        self.state.arrivals.lock().unwrap().len()
    }

    /// 服务端观察到的最大并发在途请求数。
    fn max_overlap(&self) -> u64 {
        self.state.max_overlap.load(Ordering::SeqCst)
    }

    /// 有界等待：直到本用例的 CDN 在一段时间内不再收到新请求，或超时。
    ///
    /// 目的是让 `Reader` 的后台预取线程收工后再结束用例，避免它们的计数泄漏到
    /// 后续用例（`perf` 计数器是进程级全局的，而 cargo test 共用一个进程）。
    fn wait_quiet(&self, quiet_ms: u64, budget_ms: u64) -> bool {
        let deadline = Instant::now() + Duration::from_millis(budget_ms);
        let mut last_seen = self.request_count();
        let mut quiet_since = Instant::now();
        while Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(25));
            let now = self.request_count();
            if now != last_seen {
                last_seen = now;
                quiet_since = Instant::now();
                continue;
            }
            if quiet_since.elapsed() >= Duration::from_millis(quiet_ms) {
                return true;
            }
        }
        false
    }

    fn set_force_status(&self, status: u16) {
        self.state
            .force_status
            .store(u64::from(status), Ordering::SeqCst);
    }

    fn reset_arrivals(&self) {
        self.state.arrivals.lock().unwrap().clear();
    }
}

/// 请求结束时归还"在途"仪表（含提前 return 的所有路径）。
struct ActiveGuard<'a>(&'a MockState);

impl Drop for ActiveGuard<'_> {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::SeqCst);
    }
}

fn handle_conn(stream: &mut std::net::TcpStream, state: &MockState, body: &[u8]) {
    let overlap = state.active.fetch_add(1, Ordering::SeqCst) + 1;
    state.max_overlap.fetch_max(overlap, Ordering::SeqCst);
    let _guard = ActiveGuard(state);
    let _ = stream.set_nodelay(true);
    let mut raw = Vec::new();
    let mut buf = [0_u8; 1024];
    // 只读请求头（本装置不需要请求体）。
    loop {
        match stream.read(&mut buf) {
            Ok(0) => return,
            Ok(n) => {
                raw.extend_from_slice(&buf[..n]);
                if raw.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
                if raw.len() > 64 * 1024 {
                    return;
                }
            }
            Err(_) => return,
        }
    }
    let head = String::from_utf8_lossy(&raw).to_string();

    // RG-A：请求类型记录（method / 是否带 Range / 路径）。
    let method = head
        .split_whitespace()
        .next()
        .unwrap_or("GET")
        .to_ascii_uppercase();
    let request_path = head.split_whitespace().nth(1).unwrap_or("/").to_string();
    let head_has_range = head
        .lines()
        .any(|l| l.to_ascii_lowercase().starts_with("range:"));
    state
        .requests
        .lock()
        .unwrap()
        .push((method.clone(), head_has_range, request_path));

    let synthetic = state.synthetic_len.load(Ordering::SeqCst);
    let total_len = if synthetic > 0 {
        synthetic
    } else {
        body.len() as u64
    };
    // RG-A：不支持 Range 的服务器 ⇒ 忽略 Range 头，按整段 200 返回。
    let range_ignored = state.no_range_support.load(Ordering::SeqCst) != 0;
    let (start, len, had_range) = if range_ignored {
        (0_u64, total_len, false)
    } else {
        parse_range_ex(&head, total_len)
    };
    // 仅数据 GET 计入 arrivals（PROPFIND/HEAD 已在上面 return）⇒ 既有语义不变。
    state
        .arrivals
        .lock()
        .unwrap()
        .push((perf::now_us(), start, len));

    let delay = state.delay_ms.load(Ordering::SeqCst);
    if delay > 0 {
        std::thread::sleep(Duration::from_millis(delay));
    }

    // RG-A：最小 WebDAV 能力 —— `webdav_connect` 需要 PROPFIND 得到 2xx（返回值被丢弃，
    // 因此无需真实 XML 解析），以及 HEAD（RTT 探测；失败也不致命）。
    if method == "PROPFIND" {
        // 生产  = PROPFIND + parse_multistatus 取 getcontentlength
        // ⇒ 必须给出真实长度，否则 Stream 分支无法构造 WebDavFile。
        let total_for_propfind = if synthetic > 0 {
            synthetic
        } else {
            body.len() as u64
        };
        let body = format!(
            concat!(
                "<?xml version=\"1.0\" encoding=\"utf-8\"?>",
                "<D:multistatus xmlns:D=\"DAV:\"><D:response>",
                "<D:href>/file.cbz</D:href><D:propstat><D:prop>",
                "<D:displayname>file.cbz</D:displayname>",
                "<D:getcontentlength>{}</D:getcontentlength>",
                "<D:resourcetype/></D:prop>",
                "<D:status>HTTP/1.1 200 OK</D:status></D:propstat>",
                "</D:response></D:multistatus>"
            ),
            total_for_propfind
        );
        let body = body.as_str();
        let response = format!(
            "HTTP/1.1 207 Multi-Status\r\nContent-Type: application/xml; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        let _ = stream.write_all(response.as_bytes());
        let _ = stream.flush();
        return;
    }
    if method == "HEAD" {
        let total = if synthetic > 0 {
            synthetic
        } else {
            body.len() as u64
        };
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {total}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n"
        );
        let _ = stream.write_all(response.as_bytes());
        let _ = stream.flush();
        return;
    }

    let forced = state.force_status.load(Ordering::SeqCst) as u16;
    if forced != 0 {
        let head = format!("HTTP/1.1 {forced} X\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        let _ = stream.write_all(head.as_bytes());
        let _ = stream.flush();
        return;
    }

    let total = total_len;
    let end = start.saturating_add(len).min(total);
    // RG-A：忠实响应码 —— 无 Range 头且启用时回 200（WebDAV 整包下载的真实形态）。
    // 忽略 Range 的服务器必须回 200 + 全量体（回 206 但覆盖整段会被分类器判为
    // MalformedResponse —— 那正是"声称支持 Range 却响应损坏"，与"不支持 Range"不同）。
    let faithful_200 =
        range_ignored || (!had_range && state.faithful_status.load(Ordering::SeqCst) != 0);
    let response_head = if faithful_200 {
        format!(
            "HTTP/1.1 200 OK\r\n\
             Content-Length: {}\r\n\
             Accept-Ranges: bytes\r\n\
             Connection: close\r\n\r\n",
            end.saturating_sub(start)
        )
    } else {
        // RG-A：A2-6 —— 故意给出非法 Content-Range（状态码与 body 其余部分正常）。
        let content_range = if state.malformed_content_range.load(Ordering::SeqCst) != 0 {
            "bytes garbage".to_string()
        } else {
            format!("bytes {}-{}/{}", start, end.saturating_sub(1), total)
        };
        format!(
            "HTTP/1.1 206 Partial Content\r\n\
             Content-Length: {}\r\n\
             Content-Range: {}\r\n\
             Accept-Ranges: bytes\r\n\
             Connection: close\r\n\r\n",
            end.saturating_sub(start),
            content_range
        )
    };
    let _ = stream.write_all(response_head.as_bytes());

    // RG-A：中途断流 —— 状态成功、长度已声明完整，但只发前 N 字节后关闭连接。
    // `abort_after_bytes` 对所有 GET 生效（Step 2 用）；`..._on_full_get` **只**对无
    // Range 头的 full GET 生效，确保 A2-4 的攻击点在 fallback，而不是 Range probe。
    let abort_any = state.abort_after_bytes.load(Ordering::SeqCst);
    let abort_full = state.abort_after_bytes_on_full_get.load(Ordering::SeqCst);
    let abort_after = if abort_any > 0 {
        abort_any
    } else if abort_full > 0 && !head_has_range {
        abort_full
    } else {
        0
    };
    if abort_after > 0 {
        let cut = end.min(start.saturating_add(abort_after));
        if synthetic > 0 {
            let mut cursor = start;
            while cursor < cut {
                let chunk_end = (cursor + 64 * 1024).min(cut);
                let buf: Vec<u8> = (cursor..chunk_end).map(synth_byte).collect();
                let _ = stream.write_all(&buf);
                cursor = chunk_end;
            }
        } else {
            let _ = stream.write_all(&body[start as usize..cut as usize]);
        }
        let _ = stream.flush();
        return; // 主动关闭连接（drop ⇒ FIN/RST）
    }

    let drip_bytes = state.drip_bytes.load(Ordering::SeqCst);
    let drip_ms = state.drip_ms.load(Ordering::SeqCst);
    if synthetic > 0 {
        // 合成体：始终分块写出（零内存）——慢滴时按 drip_bytes/间隔，否则 64KiB 零等待。
        let chunk = if drip_bytes > 0 { drip_bytes } else { 64 * 1024 };
        let mut cursor = start;
        while cursor < end {
            let chunk_end = (cursor + chunk).min(end);
            let buf: Vec<u8> = (cursor..chunk_end).map(synth_byte).collect();
            let _ = stream.write_all(&buf);
            let _ = stream.flush();
            cursor = chunk_end;
            if drip_ms > 0 {
                std::thread::sleep(Duration::from_millis(drip_ms));
            }
        }
    } else if drip_bytes == 0 {
        // 既有路径：实体一次性写出（P0-A 行为不变）。
        let _ = stream.write_all(&body[start as usize..end as usize]);
        let _ = stream.flush();
    } else {
        let mut cursor = start;
        while cursor < end {
            let chunk_end = (cursor + drip_bytes).min(end);
            let _ = stream.write_all(&body[cursor as usize..chunk_end as usize]);
            let _ = stream.flush();
            cursor = chunk_end;
            if drip_ms > 0 {
                std::thread::sleep(Duration::from_millis(drip_ms));
            }
        }
    }
}

/// RG-A：合成体的确定性字节（按**绝对偏移**计算，零内存且可校验）。
fn synth_byte(offset: u64) -> u8 {
    (offset.wrapping_mul(2_654_435_761) >> 13) as u8
}

/// 解析 `Range: bytes=a-b`，并返回**是否带了 Range 头**。
fn parse_range_ex(head: &str, total: u64) -> (u64, u64, bool) {
    for line in head.lines() {
        let lower = line.to_ascii_lowercase();
        if let Some(rest) = lower.strip_prefix("range:") {
            let value = rest.trim();
            if let Some(spec) = value.strip_prefix("bytes=") {
                let spec = spec.split(',').next().unwrap_or("").trim();
                let mut parts = spec.splitn(2, '-');
                let start = parts
                    .next()
                    .and_then(|v| v.trim().parse::<u64>().ok())
                    .unwrap_or(0);
                let end = parts
                    .next()
                    .and_then(|v| v.trim().parse::<u64>().ok())
                    .unwrap_or(total.saturating_sub(1));
                if end >= start {
                    return (start, end - start + 1, true);
                }
            }
        }
    }
    (0, total, false)
}

// ---------------------------------------------------------------------------
// 真实阅读链路：ByteSource -> Cloud115WebClient::read_range_url
// ---------------------------------------------------------------------------

/// 与生产 `Cloud115WebFile` 同构：每个 range 读都走 `read_range_url`
/// （因此共享同一个生产 `range_gate`）。
struct RangeBackedSource {
    client: Arc<Cloud115WebClient>,
    url: String,
    length: u64,
}

impl ByteSource for RangeBackedSource {
    fn len(&self) -> u64 {
        self.length
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> std::io::Result<usize> {
        self.client.read_range_url(&self.url, offset, buf)
    }
}

fn new_client() -> Arc<Cloud115WebClient> {
    Arc::new(Cloud115WebClient::new("UID=1_p0baseline", "0").expect("build 115 web client"))
}

// ---------------------------------------------------------------------------
// CBZ 构造
// ---------------------------------------------------------------------------

/// 构造一个每页 `page_bytes` 字节、共 `pages` 页的 CBZ（Stored，不压缩，
/// 因此字节偏移与文件大小可预测，测量不受 deflate 波动影响）。
fn build_cbz(pages: usize, page_bytes: usize) -> Vec<u8> {
    use zip::write::SimpleFileOptions;
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    // 用确定性伪随机填充，避免被 Stored 之外的路径压缩掉。
    let mut seed = 0x1234_5678_9abc_def0_u64;
    for index in 0..pages {
        writer
            .start_file(format!("{:04}.jpg", index + 1), options)
            .expect("start zip entry");
        let mut chunk = Vec::with_capacity(page_bytes);
        for _ in 0..page_bytes {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            chunk.push((seed >> 33) as u8);
        }
        writer.write_all(&chunk).expect("write zip entry");
    }
    writer.finish().expect("finish zip").into_inner()
}

/// P0-A 基线（改造前）在同一装置上测到的"后台负载对前台的放大倍数"。
/// 记录在这里是为了让 before/after 用同一口径可比，而不是事后各说各话。
const BASELINE_BACKGROUND_INFLATION_X: f64 = 5.11;

/// P0-A 基线（改造前）同装置测到的"前台单次门控等待峰值"（微秒）。
/// 1 248 ms 意味着前台排在整个后台队列之后。
const BASELINE_FOREGROUND_GATE_WAIT_MAX_US: u64 = 1_248_445;

// ---------------------------------------------------------------------------
// 报告
// ---------------------------------------------------------------------------

fn report(label: &str, payload: serde_json::Value) {
    println!(
        "P0-BASELINE {} {}",
        label,
        serde_json::to_string(&payload).unwrap_or_else(|_| "{}".into())
    );
}

fn delta(before: u64, after: u64) -> u64 {
    after.saturating_sub(before)
}

fn percentile(sorted: &[u64], p: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let idx = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[idx]
}

// ---------------------------------------------------------------------------
// 用例 1：CDN Range 门控是否构成人工下限
// ---------------------------------------------------------------------------

/// 顺序发起 `n` 次 Range 读取，测量总 wall time，并与固定间隔门控的理论下限
/// `(n-1) * interval` 对比。
///
/// 期望（改造前）：实测 ≈ 理论下限 —— 证明**读速被门控而不是被网络决定**。
/// 期望（P0-B 之后）：并发场景下实测显著低于 `(n-1) * interval`。
#[test]
fn p0a_cdn_gate_forms_the_latency_floor() {
    const REQUEST_INTERVAL_MS: u64 = 250; // 4 req/s
    let n = 9_usize;
    let cdn = MockCdn::start(build_cbz(4, 4096), 5);
    let client = new_client();
    let url = cdn.url();

    perf::reset();
    let mut wait_us = Vec::with_capacity(n);
    let wall = Instant::now();
    let mut buf = vec![0_u8; 1024];
    for i in 0..n {
        let before = perf::read(Counter::CdnGateWaitUsTotal);
        let read = client
            .read_range_url(&url, (i as u64) * 1024, &mut buf)
            .expect("range read");
        assert_eq!(read, 1024);
        wait_us.push(delta(before, perf::read(Counter::CdnGateWaitUsTotal)));
    }
    let wall_ms = wall.elapsed().as_millis() as u64;

    let theoretical_floor_ms = (n as u64 - 1) * REQUEST_INTERVAL_MS;
    // 服务端记录的到达时刻（相对进程启动，微秒）——P0 要求的 "request start
    // timestamps"。相邻差值直接显示门控把请求摊开成了 250 ms 一档。
    let arrivals = cdn.arrivals();
    let first_arrival = arrivals.first().map(|v| v.0).unwrap_or(0);
    let arrival_offsets_us: Vec<u64> = arrivals.iter().map(|v| v.0 - first_arrival).collect();
    report(
        "cdn_gate_sequential",
        serde_json::json!({
            "requests": n,
            "wall_ms": wall_ms,
            "theoretical_floor_ms": theoretical_floor_ms,
            "gate_wait_us_total": perf::read(Counter::CdnGateWaitUsTotal),
            "gate_wait_us_max": perf::read(Counter::CdnGateWaitUsMax),
            "cdn_requests": perf::read(Counter::RangeRequests),
            "cdn_status_206": perf::read(Counter::RangeStatus206),
            "inflight_max": perf::read(Counter::RangeInFlightMax),
            "server_request_count": cdn.request_count(),
            "arrival_offsets_us": arrival_offsets_us,
        }),
    );

    // P0-B 之后的新契约（改造前这里断言的是"必须被摊平成 2000 ms"）：
    //   1. 突发额度内的请求**不再**被逐个摊开；
    //   2. 超出突发后仍回到持续速率（4 req/s），不是无限放开；
    //   3. 旧的人工下限已经消失（wall 明显低于 floor）。
    assert!(
        wall_ms < theoretical_floor_ms,
        "the old fixed-interval floor must be gone: wall={wall_ms}ms floor={theoretical_floor_ms}ms"
    );
    let burst_head = arrival_offsets_us
        .iter()
        .take(6)
        .copied()
        .collect::<Vec<_>>();
    assert!(
        burst_head.last().copied().unwrap_or(u64::MAX) < 150_000,
        "the first 6 requests must go out as a burst, not 250 ms apart: {burst_head:?}"
    );
    let tail = &arrival_offsets_us[6..];
    for pair in tail.windows(2) {
        assert!(
            pair[1] - pair[0] >= 200_000,
            "beyond the burst the sustained rate must still apply: {tail:?}"
        );
    }
    // 同时确认门控等待确实被归因到 CDN 桶，而不是 API 桶。
    assert!(perf::read(Counter::CdnGateWaitUsTotal) > 0);
    assert_eq!(perf::read(Counter::RangeStatus206), n as u64);
}

/// 并发发起 `n` 次 Range 读取（模拟预取 fan-out + 后台封面同时打同一个客户端）。
///
/// 期望（改造前）：即便并发，串行化仍然成立 —— 实测 ≈ `(n-1) * interval`，
/// 且 `inflight_max == 1`（因为门锁把并发压成了串行）。
#[test]
fn p0a_concurrent_reads_are_serialised_by_the_same_gate() {
    const REQUEST_INTERVAL_MS: u64 = 250;
    let n = 9_usize;
    let cdn = MockCdn::start(build_cbz(4, 4096), 5);
    let client = new_client();
    let url = cdn.url();

    perf::reset();
    let wall = Instant::now();
    let mut handles = Vec::new();
    for i in 0..n {
        let client = Arc::clone(&client);
        let url = url.clone();
        handles.push(std::thread::spawn(move || {
            let mut buf = vec![0_u8; 1024];
            client
                .read_range_url(&url, (i as u64) * 1024, &mut buf)
                .expect("range read")
        }));
    }
    for handle in handles {
        assert_eq!(handle.join().expect("reader thread"), 1024);
    }
    let wall_ms = wall.elapsed().as_millis() as u64;
    let theoretical_floor_ms = (n as u64 - 1) * REQUEST_INTERVAL_MS;

    report(
        "cdn_gate_concurrent",
        serde_json::json!({
            "requests": n,
            "wall_ms": wall_ms,
            "theoretical_floor_ms": theoretical_floor_ms,
            "server_max_overlap": cdn.max_overlap(),
            "caller_entered_max": perf::read(Counter::RangeInFlightMax),
            "gate_wait_us_total": perf::read(Counter::CdnGateWaitUsTotal),
            "cdn_status_206": perf::read(Counter::RangeStatus206),
            "server_request_count": cdn.request_count(),
        }),
    );

    // P0-B 之后的新契约（改造前这里是 `wall >= floor` 且 `max_overlap == 1`）：
    // 独立于速率的在途上限允许真实并发，同时仍然有界（不是无限放开）。
    assert!(
        wall_ms < theoretical_floor_ms,
        "the old fixed-interval floor must be gone: wall={wall_ms}ms floor={theoretical_floor_ms}ms"
    );
    assert!(
        cdn.max_overlap() >= 2,
        "the independent in-flight cap must allow real concurrency, got {}",
        cdn.max_overlap()
    );
    assert!(
        cdn.max_overlap() <= 8,
        "concurrency must stay bounded, got {}",
        cdn.max_overlap()
    );
    assert_eq!(cdn.request_count(), n);
}

// ---------------------------------------------------------------------------
// 用例 2：真实阅读链路的单页延迟与每页 Range 次数
// ---------------------------------------------------------------------------

struct PageProbe {
    open_ms: u64,
    page_ms: Vec<u64>,
    source_reads_per_page: Vec<u64>,
    range_requests_per_page: Vec<u64>,
}

fn probe_reader(pages: usize, page_bytes: usize, cdn_delay_ms: u64) -> (PageProbe, MockCdn) {
    let cdn = MockCdn::start(build_cbz(pages, page_bytes), cdn_delay_ms);
    let client = new_client();
    let source = RangeBackedSource {
        client,
        url: cdn.url(),
        length: cdn.len(),
    };

    perf::reset();
    let open_start = Instant::now();
    let document = open_document(source, "p0-baseline.cbz").expect("open document");
    let open_ms = open_start.elapsed().as_millis() as u64;
    assert_eq!(document.page_count() as usize, pages);

    // 每次运行使用唯一的缓存命名空间，确保 L2 磁盘缓存从空开始。
    let cache_ns = format!("p0-baseline-{}", perf::now_us());
    let reader = Arc::new(Reader::new(document, &cache_ns));

    let mut page_ms = Vec::new();
    let mut source_reads_per_page = Vec::new();
    let mut range_requests_per_page = Vec::new();
    for index in 0..3_u32 {
        let reads_before = perf::read(Counter::SourceReadAt);
        let ranges_before = perf::read(Counter::RangeRequests);
        let start = Instant::now();
        let bytes = reader.get_page(index).expect("get page");
        page_ms.push(start.elapsed().as_millis() as u64);
        source_reads_per_page.push(delta(reads_before, perf::read(Counter::SourceReadAt)));
        range_requests_per_page.push(delta(ranges_before, perf::read(Counter::RangeRequests)));
        assert_eq!(bytes.len(), page_bytes);
    }

    let _ = cdn.wait_quiet(250, 3_000);
    cleanup_page_cache(&cache_ns);

    (
        PageProbe {
            open_ms,
            page_ms,
            source_reads_per_page,
            range_requests_per_page,
        },
        cdn,
    )
}

/// 删除本装置写入的 L2 分页缓存目录，避免污染真实缓存根。
fn cleanup_page_cache(cache_ns: &str) {
    if let Ok(dir) = rust_lib_app::cache::CacheDir::Page.ensure() {
        let target = dir.join(rust_lib_app::cache::stable_hash(cache_ns));
        let _ = std::fs::remove_dir_all(&target);
    }
}

/// 测量：单页 1.5 MiB、无后台竞争时，一页要付多少次 Range、多少 wall time。
///
/// 期望（改造前）：每页 `read_at` ≈ ceil(1.5 MiB / 256 KiB) = 6，
/// 门控下限 ≈ 5 × 250 ms = 1.25 s，因此单页 ≈ 1.3 s+。
#[test]
fn p0a_single_page_latency_without_background_load() {
    const PAGE_BYTES: usize = 1536 * 1024; // 1.5 MiB
    let (probe, _cdn) = probe_reader(4, PAGE_BYTES, 2);

    let reads = probe.source_reads_per_page.clone();
    report(
        "page_latency_no_background",
        serde_json::json!({
            "page_bytes": PAGE_BYTES,
            "read_ahead_bytes": 256 * 1024,
            "theoretical_reads_per_page": (PAGE_BYTES as u64).div_ceil(256 * 1024),
            "open_ms": probe.open_ms,
            "page_ms": probe.page_ms,
            "source_reads_per_page": reads,
            "source_reads_note": "并发下该逐页计数包含同窗口内其它线程的读取，仅作参考",
            "range_requests_per_page": probe.range_requests_per_page,
            "gate_wait_us_total": perf::read(Counter::CdnGateWaitUsTotal),
            "inflight_max": perf::read(Counter::RangeInFlightMax),
        }),
    );

    // 3 页合计的预读窗口未命中数应大于 0：被后台预取提前读进来的页会记 0，
    // 这是预取生效的正确行为，不是失败。
    let total_reads: u64 = probe.source_reads_per_page.iter().sum();
    let theoretical_total =
        probe.source_reads_per_page.len() as u64 * (PAGE_BYTES as u64).div_ceil(256 * 1024);
    report(
        "page_latency_no_background_totals",
        serde_json::json!({
            "total_source_reads": total_reads,
            "theoretical_total_if_all_cold": theoretical_total,
            "prefetched_pages": probe.source_reads_per_page.iter().filter(|&&v| v == 0).count(),
        }),
    );
    assert!(
        total_reads > 0,
        "reading pages over the network must issue range reads"
    );
    assert!(
        probe.page_ms.iter().any(|&v| v >= 500),
        "at least one cold page must pay the gate floor: {:?}",
        probe.page_ms
    );
}

// ---------------------------------------------------------------------------
// 用例 3：前台阅读 + 后台覆盖/扫描竞争
// ---------------------------------------------------------------------------

/// 同一次运行内做两相测量，直接得到"后台负载把前台延迟抬高了多少"：
///
/// - A 相：只有前台阅读（独占 4/s 门控）；
/// - B 相：3 个后台线程在**同一个客户端**上连续做 Range 读（等价于后台封面/目录
///   扫描拿到同一个生产 `range_gate`），前台同时在读同一本书。
///
/// 两相各自使用独立的客户端与独立的 L2 命名空间，避免相位之间互相预热。
///
/// 期望（改造前）：B 相前台 p50 显著高于 A 相 —— 这就是"外层优先级没有贯通到底层
/// 请求"的直接度量。P0-B 之后两相应明显收敛。
/// 返回：(每页毫秒, open 毫秒, 后台请求数, 前台门控等待总量µs, 前台门控单次峰值µs, CDN)
fn phase_foreground_latency(
    label: &str,
    background_threads: usize,
) -> (Vec<u64>, u64, u64, u64, u64, MockCdn) {
    const PAGE_BYTES: usize = 1536 * 1024;
    // 16 页：前台读 8 页 + 预取半径 3，保证不会越界。
    let cdn = MockCdn::start(build_cbz(16, PAGE_BYTES), 2);
    let client = new_client();
    let url = cdn.url();
    let source = RangeBackedSource {
        client: Arc::clone(&client),
        url: url.clone(),
        length: cdn.len(),
    };

    let open_start = Instant::now();
    let document = open_document(source, "p0-baseline.cbz").expect("open document");
    let open_ms = open_start.elapsed().as_millis() as u64;
    let cache_ns = format!("p0-{label}-{}", perf::now_us());
    let reader = Arc::new(Reader::new(document, &cache_ns));

    // 让 open 阶段的前台读数结算完毕，并给预取线程一点时间安静下来。
    std::thread::sleep(Duration::from_millis(300));
    perf::reset();
    let perf_foreground_wait_before = perf::read(Counter::CdnGateWaitUsTotalForeground);

    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut background = Vec::new();
    for worker in 0..background_threads as u64 {
        let client = Arc::clone(&client);
        let url = url.clone();
        let stop = Arc::clone(&stop);
        background.push(std::thread::spawn(move || {
            let mut count = 0_u64;
            let mut buf = vec![0_u8; 4096];
            while !stop.load(Ordering::SeqCst) {
                let offset = 1_000_000 + worker * 100_000 + (count % 16) * 4096;
                // 生产里后台封面/扫描在上层就标注了优先级（封面 worker 标注
                // Cover，目录发现标注 Scan）。装置必须同样标注，否则它模拟的
                // 是"另一个前台"，优先级机制当然不起作用。
                let outcome = with_priority(RequestPriority::Cover, || {
                    client.read_range_url(&url, offset, &mut buf)
                });
                if outcome.is_err() {
                    break;
                }
                count += 1;
                if count > 400 {
                    break;
                }
            }
            count
        }));
    }
    if background_threads > 0 {
        // 让后台先占住门控，模拟"后台已经在跑，用户此刻翻页"。
        std::thread::sleep(Duration::from_millis(120));
    }

    // 8 页而不是 3 页：单页延迟受"是否恰好被预取命中"影响，样本太少时
    // p50 会被单次命中/未命中主导。总量更稳。
    const FOREGROUND_PAGES: u32 = 8;
    let mut page_ms = Vec::new();
    for index in 0..FOREGROUND_PAGES {
        let start = Instant::now();
        let bytes = reader.get_page(index).expect("foreground page");
        page_ms.push(start.elapsed().as_millis() as u64);
        assert_eq!(bytes.len(), PAGE_BYTES);
    }

    // 前台自己的门控等待：在关闭后台线程**之前**读取，避免后台尾巴混进来。
    let foreground_wait_total = delta(
        perf_foreground_wait_before,
        perf::read(Counter::CdnGateWaitUsTotalForeground),
    );
    let foreground_wait_max = perf::read(Counter::CdnGateWaitUsMaxForeground);

    stop.store(true, Ordering::SeqCst);
    let background_requests: u64 = background.into_iter().map(|h| h.join().unwrap_or(0)).sum();

    cleanup_page_cache(&cache_ns);
    (
        page_ms,
        open_ms,
        background_requests,
        foreground_wait_total,
        foreground_wait_max,
        cdn,
    )
}

#[test]
fn p0a_foreground_page_latency_compared_with_and_without_background_load() {
    let (quiet_ms, quiet_open_ms, _, quiet_wait_total, quiet_wait_max, _) =
        phase_foreground_latency("quiet", 0);
    let (
        loaded_ms,
        loaded_open_ms,
        background_requests,
        loaded_wait_total,
        loaded_wait_max,
        loaded_cdn,
    ) = phase_foreground_latency("loaded", 3);

    let mut quiet_sorted = quiet_ms.clone();
    quiet_sorted.sort_unstable();
    let mut loaded_sorted = loaded_ms.clone();
    loaded_sorted.sort_unstable();
    let quiet_sum: u64 = quiet_ms.iter().sum();
    let loaded_sum: u64 = loaded_ms.iter().sum();
    let ratio = if quiet_sum > 0 {
        (loaded_sum as f64) / (quiet_sum as f64)
    } else {
        0.0
    };

    report(
        "foreground_vs_background",
        serde_json::json!({
            "quiet_page_ms": quiet_ms,
            "quiet_sum_ms": quiet_sum,
            "quiet_p50_ms": percentile(&quiet_sorted, 0.5),
            "quiet_max_ms": quiet_sorted.last().copied().unwrap_or(0),
            "quiet_open_ms": quiet_open_ms,
            "loaded_page_ms": loaded_ms,
            "loaded_sum_ms": loaded_sum,
            "loaded_p50_ms": percentile(&loaded_sorted, 0.5),
            "loaded_max_ms": loaded_sorted.last().copied().unwrap_or(0),
            "loaded_open_ms": loaded_open_ms,
            "loaded_sum_ratio_x": ratio,
            "baseline_before_p0b_ratio_x": BASELINE_BACKGROUND_INFLATION_X,
            "quiet_foreground_gate_wait_us": quiet_wait_total,
            "quiet_foreground_gate_wait_max_us": quiet_wait_max,
            "loaded_foreground_gate_wait_us": loaded_wait_total,
            "loaded_foreground_gate_wait_max_us": loaded_wait_max,
            "baseline_before_p0b_foreground_gate_wait_max_us": BASELINE_FOREGROUND_GATE_WAIT_MAX_US,
            "background_requests": background_requests,
            "background_max_overlap": loaded_cdn.max_overlap(),
        }),
    );

    assert!(
        background_requests > 0,
        "background load must issue real requests"
    );
    assert!(
        loaded_ms.iter().any(|&v| v > 0),
        "foreground latency must be measurable: {loaded_ms:?}"
    );
    // 1) 系统级：仍明显好于改造前的基线（同装置基线 5.11×）。
    assert!(
        ratio < BASELINE_BACKGROUND_INFLATION_X,
        "background must not inflate the foreground as badly as the baseline: quiet_sum={quiet_sum}ms loaded_sum={loaded_sum}ms ratio={ratio:.2} baseline={BASELINE_BACKGROUND_INFLATION_X}"
    );
    // 2) 机制级（关键）：前台自己**单次**门控等待不得超过一个令牌间隔量级。
    //    改造前同装置基线是 1 248 ms —— 前台排在整个后台队列之后。
    assert!(
        loaded_wait_max < BASELINE_FOREGROUND_GATE_WAIT_MAX_US,
        "foreground must not queue behind the whole background backlog: max={loaded_wait_max}us baseline={BASELINE_FOREGROUND_GATE_WAIT_MAX_US}us"
    );
    assert!(
        loaded_wait_max <= 400_000,
        "foreground single wait must stay within one token interval: {loaded_wait_max}us"
    );
}

// ---------------------------------------------------------------------------
// 用例 4：缓存命中不经过网络门控
// ---------------------------------------------------------------------------

/// 证明 L2 磁盘命中完全不经过 CDN 门控（这是 P0 必须保持的既有正确行为）。
///
/// 期望：第二次打开同一本书、同一页时，`cdn_range` 请求数为 0，页面立刻返回。
#[test]
fn p0a_disk_cache_hit_bypasses_the_network_gate() {
    const PAGE_BYTES: usize = 512 * 1024;
    let cdn = MockCdn::start(build_cbz(2, PAGE_BYTES), 2);
    let source = RangeBackedSource {
        client: new_client(),
        url: cdn.url(),
        length: cdn.len(),
    };
    let cache_ns = format!("p0-diskcache-{}", perf::now_us());

    // 第一次：冷启动，产生真实 Range 请求。
    perf::reset();
    let document = open_document(source, "p0-disk.cbz").expect("open document");
    let reader = Arc::new(Reader::new(document, &cache_ns));
    let first = reader.get_page(0).expect("cold page");
    assert_eq!(first.len(), PAGE_BYTES);
    let cold_ranges = perf::read(Counter::RangeRequests);
    assert!(cold_ranges > 0, "cold read must hit the network");

    // 第二次：同一 cache_ns。注意 —— 流式打开**每次都要重读压缩包中心目录**，
    // 因此 open 本身仍然要走网络；测量窗口必须收窄到 get_page 才能判定
    // "磁盘命中是否绕开门控"。
    let source2 = RangeBackedSource {
        client: new_client(),
        url: cdn.url(),
        length: cdn.len(),
    };
    cdn.reset_arrivals();
    let reopen_start = Instant::now();
    let document2 = open_document(source2, "p0-disk.cbz").expect("open document again");
    let reopen_ms = reopen_start.elapsed().as_millis() as u64;
    let reopen_requests = cdn.request_count();
    let reader2 = Arc::new(Reader::new(document2, &cache_ns));

    // 从这里开始才计量"页读取"。
    perf::reset();
    cdn.reset_arrivals();
    let start = Instant::now();
    let warm = reader2.get_page(0).expect("warm page");
    let warm_ms = start.elapsed().as_millis() as u64;
    let warm_requests = cdn.request_count();
    let warm_gate_wait = perf::read(Counter::CdnGateWaitUsTotal);
    let warm_disk_hits = perf::read(Counter::PageDiskHits);
    cleanup_page_cache(&cache_ns);

    report(
        "disk_cache_hit",
        serde_json::json!({
            "cold_ranges": cold_ranges,
            "reopen_ms": reopen_ms,
            "reopen_requests": reopen_requests,
            "warm_ms": warm_ms,
            "warm_cdn_requests": warm_requests,
            "warm_gate_wait_us": warm_gate_wait,
            "warm_disk_hits": warm_disk_hits,
            "note": "reopen_* 是流式打开重读压缩包中心目录的成本（P0 待优化项）",
        }),
    );

    assert_eq!(warm.len(), PAGE_BYTES);
    assert_eq!(
        warm_requests, 0,
        "a disk-cache hit must issue no provider request"
    );
    assert_eq!(
        warm_gate_wait, 0,
        "a disk-cache hit must not wait on the CDN gate"
    );
    assert!(
        warm_disk_hits >= 1,
        "the page must be served from the L2 disk cache"
    );
}

/// 证明 API 桶与 CDN 桶在埋点口径上是分开的，且 CDN 门控不会被 API 门控
/// 的等待污染（P0-B 将把这一"归属正确"升级为"调度真正解耦"）。
#[test]
fn p0a_gate_attribution_separates_api_and_cdn() {
    let cdn = MockCdn::start(build_cbz(2, 4096), 2);
    let client = new_client();
    let mut buf = vec![0_u8; 512];

    // 用增量测量：其它用例的预取线程可能在后台继续发请求，全局计数的绝对值
    // 不可靠，只有围绕本次调用的差值可信。
    let ranges_before = perf::read(Counter::RangeRequests);
    let cdn_before = perf::read(Counter::CdnGateWaitUsTotal);
    let api_before = perf::read(Counter::ApiGateWaitUsTotal);
    let arrivals_before = cdn.request_count();

    client
        .read_range_url(&cdn.url(), 0, &mut buf)
        .expect("range read");

    let cdn_wait = delta(cdn_before, perf::read(Counter::CdnGateWaitUsTotal));
    let api_wait = delta(api_before, perf::read(Counter::ApiGateWaitUsTotal));
    let cdn_requests = delta(ranges_before, perf::read(Counter::RangeRequests));
    let server_requests = cdn.request_count() - arrivals_before;

    report(
        "gate_attribution",
        serde_json::json!({
            "cdn_gate_wait_us_delta": cdn_wait,
            "api_gate_wait_us_delta": api_wait,
            "cdn_requests_delta": cdn_requests,
            "server_requests_delta": server_requests,
        }),
    );

    // 线级请求数以服务端观测为准（每个用例自带一个服务端，天然隔离）；
    // 进程级计数器只用于"归属哪个桶"，绝对值可能被其它用例的预取线程抬高。
    assert_eq!(
        server_requests, 1,
        "exactly one wire request for one range read"
    );
    assert!(
        cdn_requests >= 1,
        "the range read must bump the CDN counter"
    );
    assert!(
        cdn_wait > 0,
        "the CDN gate wait must be attributed to the CDN bucket"
    );
    // CDN 请求不应把等待记到 API 桶。
    assert_eq!(
        api_wait, 0,
        "CDN reads must not be attributed to the API gate"
    );
}

// ---------------------------------------------------------------------------
// 用例 5：错误码必须被如实计数（P0 验收"没有新的 403/405/429"的度量基础）
// ---------------------------------------------------------------------------

/// 强制 CDN 返回 403 / 405，验证状态码分布被如实归类，且 405 会触发 115 的
/// WAF 冷却计数。这三项是 P0 提高 CDN 档位时的**停止条件**，必须先能测到。
#[test]
fn p0a_range_statuses_are_accounted_for_forbidden_and_waf() {
    let cdn = MockCdn::start(build_cbz(2, 4096), 1);
    let url = cdn.url();
    let mut buf = vec![0_u8; 256];

    // 403：每个客户端独立，避免 405 的会话冷却污染统计。
    perf::reset();
    let forbidden = new_client();
    cdn.set_force_status(403);
    let err = forbidden
        .read_range_url(&url, 0, &mut buf)
        .expect_err("403 must surface as an error");
    assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
    assert_eq!(perf::read(Counter::RangeStatus403), 1);

    // 405：必须计入 405 与 WAF 冷却，并让后续请求快速失败。
    perf::reset();
    let waf = new_client();
    cdn.set_force_status(405);
    assert!(waf.read_range_url(&url, 0, &mut buf).is_err());
    assert_eq!(perf::read(Counter::RangeStatus405), 1);
    assert_eq!(perf::read(Counter::WafCooldowns), 1);
    let cooldown_err = waf
        .read_range_url(&url, 0, &mut buf)
        .expect_err("cooldown must short-circuit the next request");
    assert!(
        cooldown_err.to_string().contains("风控"),
        "cooldown error must stay user-facing Chinese: {cooldown_err}"
    );

    report(
        "range_status_accounting",
        serde_json::json!({
            "status_403": 1,
            "status_405": 1,
            "waf_cooldowns": 1,
            "cooldown_message_is_chinese": true,
        }),
    );
}


// ---------------------------------------------------------------------------
// RG-A A-1 / A-5：WebDAV `download_to_raw_cache` 作为 ADR-005 production 参考路径
// ---------------------------------------------------------------------------
//
// 契约（本轮钉死）：
// * 缓存权威判据仍是 **final path exists && len > 0**（不引入 checksum / 远端长度持久化 /
//   manifest / sidecar）；由于唯一生产发布入口已改为 atomic commit，A-1/A-5 只需证明
//   **不完整字节无法创建最终权威路径**。
// * 内存模型证据来自**结构性/时间性**观察（server 仍在发送时 `.part-*` 已在增长），
//   而不是 RSS 阈值 ⇒ "streaming behavior verified structurally/temporally;
//   no fixed RSS ceiling asserted."
// * 这些用例会切换**全局**自定义缓存根 ⇒ 必须与其他使用磁盘缓存的用例串行运行
//   （本仓门的跑法为 `-- --test-threads=1`）。

fn rga_tmp_root(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rch_rga_webdav_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    cache::set_custom_cache_root(dir.to_str().unwrap());
    dir
}

/// 递归查找同名文件（通用地规避依赖 cache 内部布局）。
fn rga_find_named(root: &Path, name: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.file_name().map(|n| n == name).unwrap_or(false) {
                out.push(p);
            }
        }
    }
    out
}

/// 递归查找 `.part-*` 临时文件。
fn rga_part_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
            } else if p
                .file_name()
                .map(|n| n.to_string_lossy().contains(".part-"))
                .unwrap_or(false)
            {
                out.push(p);
            }
        }
    }
    out
}

fn rga_base(cdn: &MockCdn) -> String {
    cdn.url()
        .trim_end_matches("/file.cbz")
        .to_string()
}

/// 走**生产** `WebDavClient::download_to_raw_cache`（每次调用新建 client 实例）。
fn rga_download(base: &str, path: &str) -> anyhow::Result<PathBuf> {
    let (client, _browse) = WebDavClient::new(base, "user", "pass")?;
    client.download_to_raw_cache(path, None)
}

/// 抽样读取（绝不把整个文件读回内存）。
fn rga_sample_offsets(path: &Path, offsets: &[u64]) -> Vec<u8> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path).unwrap();
    let mut out = Vec::new();
    for &o in offsets {
        f.seek(SeekFrom::Start(o)).unwrap();
        let mut b = [0u8; 1];
        f.read_exact(&mut b).unwrap();
        out.push(b[0]);
    }
    out
}

const RGA_MIB: u64 = 1024 * 1024;

/// A-1：20 / 100 / 300 MiB 全量下载矩阵（合成体 ⇒ 零内存夹具）。
///
/// 断言：成功、target 存在、`len == total`、offset 0/中点/末字节抽样与 `synth_byte` 一致、
/// `.part-*` = 0、每次首次下载恰好 1 次 HTTP 请求。
#[test]
fn rg_a_a1_webdav_full_download_matrix_20_100_300_mib() {
    for mib in [20_u64, 100, 300] {
        let total = mib * RGA_MIB;
        let root = rga_tmp_root(&format!("matrix_{mib}"));
        let cdn = MockCdn::start_synthetic(total, 0);
        let base = rga_base(&cdn);

        let target = rga_download(&base, "file.cbz").expect("full download must succeed");

        assert_eq!(cdn.total_len(), total);
        assert!(target.exists(), "{mib} MiB: target must exist");
        assert_eq!(
            std::fs::metadata(&target).unwrap().len(),
            total,
            "{mib} MiB: published length must equal the declared total"
        );

        let mid = total / 2;
        let samples = rga_sample_offsets(&target, &[0, mid, total - 1]);
        assert_eq!(samples[0], synth_byte(0), "{mib} MiB: offset 0");
        assert_eq!(samples[1], synth_byte(mid), "{mib} MiB: mid offset");
        assert_eq!(
            samples[2],
            synth_byte(total - 1),
            "{mib} MiB: last byte"
        );

        assert!(
            rga_part_files(&root).is_empty(),
            "{mib} MiB: commit must leave no .part-* behind"
        );
        assert_eq!(
            cdn.request_count(),
            1,
            "{mib} MiB: a fresh download must issue exactly one HTTP request"
        );

        // 释放磁盘：矩阵逐项清理，避免 300 MiB 叠加。
        let _ = std::fs::remove_file(&target);
    }
}

/// A-5（关键）：**流式落盘**而非整包缓冲。
///
/// server 仍在慢滴发送 body 时，`.part-*` 必须已经存在且 `0 < len < total`，同时最终
/// 权威路径**尚不存在**。若 production 内部用 `resp.bytes()`（先收完整包再写），
/// `.part` 只会在 body 收完后出现 ⇒ 本用例 RED。
///
/// 结论口径：streaming behavior verified structurally/temporally;
/// no fixed RSS ceiling asserted.
#[test]
fn rg_a_a5_webdav_streams_body_to_disk_before_completion() {
    let total = 8 * RGA_MIB;
    let root = rga_tmp_root("streaming");
    let cdn = MockCdn::start_synthetic(total, 0).with_drip(64 * 1024, 5);
    let base = rga_base(&cdn);

    let handle = std::thread::spawn(move || rga_download(&base, "file.cbz"));

    // 有界观察窗口：绝不无限轮询。
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut observed = None;
    while Instant::now() < deadline {
        let parts = rga_part_files(&root);
        if let Some(part) = parts.first() {
            let len = std::fs::metadata(part).map(|m| m.len()).unwrap_or(0);
            if len > 0 && len < total {
                assert!(
                    rga_find_named(&root, "file.cbz").is_empty(),
                    "the authoritative path must NOT exist while the body is still streaming"
                );
                observed = Some(len);
                break;
            }
        }
        if handle.is_finished() {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }

    let published = handle.join().unwrap().expect("download must finish");
    assert!(
        observed.is_some(),
        "must observe a growing .part-* while the server is still sending (streaming, not buffering)"
    );
    assert_eq!(std::fs::metadata(&published).unwrap().len(), total);
    assert!(rga_part_files(&root).is_empty());
}

/// A-5：**中途断流**（HTTP 200 + 声明完整长度，但只发前 2 MiB）⇒ 不完整字节绝不成为权威缓存。
///
/// 这条直接验证 `AtomicCacheFile` 的 Drop 清理路径：失败来自**响应体不完整**，
/// 不是 HTTP 状态分类。随后关闭断流再次调用 ⇒ **重新发请求**并成功。
#[test]
fn rg_a_a5_webdav_mid_body_abort_never_publishes_cache() {
    let total = 20 * RGA_MIB;
    let abort_after = 2 * RGA_MIB;
    let root = rga_tmp_root("abort");
    let cdn = MockCdn::start_synthetic(total, 0).with_abort_after(abort_after);
    let base = rga_base(&cdn);

    let failed = rga_download(&base, "file.cbz");
    assert!(
        failed.is_err(),
        "a truncated body must surface as an error, not a success"
    );
    assert!(
        rga_find_named(&root, "file.cbz").is_empty(),
        "partial material must NOT create the authoritative cache path"
    );
    assert!(
        rga_part_files(&root).is_empty(),
        "the aborted writer must clean up its .part-* file"
    );
    assert_eq!(cdn.request_count(), 1, "the aborted attempt issued one request");

    // 关闭断流 ⇒ 必须重新请求并成功完整下载。
    cdn.set_abort_after(0);
    let target = rga_download(&base, "file.cbz").expect("retry must succeed");
    assert_eq!(std::fs::metadata(&target).unwrap().len(), total);
    assert_eq!(
        cdn.request_count(),
        2,
        "after a failed attempt the cache must NOT be reused; a new request is required"
    );
    assert!(rga_part_files(&root).is_empty());
}

/// A-5：**body 之前的 HTTP 失败**（与"中途断流"是不同的失败类别，命名必须准确）。
#[test]
fn rg_a_a5_webdav_http_failure_before_body_does_not_publish_cache() {
    let total = 4 * RGA_MIB;
    let root = rga_tmp_root("http500");
    let cdn = MockCdn::start_synthetic(total, 0);
    cdn.set_force_status(500);
    let base = rga_base(&cdn);

    assert!(rga_download(&base, "file.cbz").is_err(), "500 must fail");
    assert!(rga_find_named(&root, "file.cbz").is_empty());
    assert!(rga_part_files(&root).is_empty());
    assert_eq!(cdn.request_count(), 1);

    cdn.set_force_status(0);
    let target = rga_download(&base, "file.cbz").expect("retry must succeed");
    assert_eq!(std::fs::metadata(&target).unwrap().len(), total);
    assert_eq!(cdn.request_count(), 2, "retry must re-request");
}

/// A-5：raw cache 跨**新 client 实例**复用（模拟新 session / 新进程侧消费者）。
///
/// 新实例请求同一 raw cache ⇒ 不再产生 HTTP 请求，且返回**同一条**缓存路径。
#[test]
fn rg_a_a5_webdav_raw_cache_is_reused_across_new_client_instance() {
    let total = 4 * RGA_MIB;
    let root = rga_tmp_root("reuse");
    let cdn = MockCdn::start_synthetic(total, 0);
    let base = rga_base(&cdn);

    let first = rga_download(&base, "file.cbz").expect("first download");
    assert_eq!(cdn.request_count(), 1);
    assert_eq!(std::fs::metadata(&first).unwrap().len(), total);

    // 全新 client 实例（新的生产对象），同一 URL + 同一 path。
    let second = rga_download(&base, "file.cbz").expect("second (cached) access");
    assert_eq!(second, first, "cross-session reuse must return the same cache path");
    assert_eq!(
        cdn.request_count(),
        1,
        "reuse must not issue any additional HTTP request"
    );
    assert!(rga_part_files(&root).is_empty());
}

/// A-5：production 侧确实使用 atomic helper（结构性断言：raw-cache 写入不再直接 create 最终路径）。
#[test]
fn rg_a_a5_webdav_production_uses_atomic_helper() {
    let src = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/source/webdav.rs"
    ))
    .expect("read webdav.rs");
    assert!(
        src.contains("AtomicCacheFile::create"),
        "WebDAV raw-cache writers must go through AtomicCacheFile"
    );
    assert!(
        !src.contains("std::fs::File::create(&file_path)"),
        "no raw-cache writer may create the final path directly"
    );
    assert!(
        !src.contains("std::io::copy(&mut resp, &mut disk)"),
        "download_full must stream through the atomic writer, not io::copy to a final path"
    );
}


// ---------------------------------------------------------------------------
// RG-A A-2：ADR-005 orchestration（真实 seam：webdav_connect → open_webdav_book）
// ---------------------------------------------------------------------------
//
// 证据口径：**只有 WebDAV** 声称自动化端到端 ADR-005 fallback 覆盖。其余 5 provider /
// 7 个 full-download 实现只共享 atomic publication helper（wiring 覆盖），**不是** provider E2E。
//
// 请求计数一律用**阶段增量**（snapshot before / after），避免把 `webdav_connect` 自身的
// PROPFIND / HEAD / Range probe 混进 `open_webdav_book` 的证据。

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct RgaCounts {
    /// GET 且**带** Range 头（book-level probe 或随机读）。
    ranged: usize,
    /// GET 且**不带** Range 头（整包下载）。
    full: usize,
    /// 全部请求（含 PROPFIND / HEAD 握手）。
    total: usize,
}

fn rga_counts(log: &[(String, bool, String)]) -> RgaCounts {
    let gets = log
        .iter()
        .filter(|(m, _, _)| m.eq_ignore_ascii_case("GET"))
        .collect::<Vec<_>>();
    RgaCounts {
        ranged: gets.iter().filter(|(_, has, _)| *has).count(),
        full: gets.iter().filter(|(_, has, _)| !*has).count(),
        total: log.len(),
    }
}

fn rga_delta(before: RgaCounts, after: RgaCounts) -> RgaCounts {
    RgaCounts {
        ranged: after.ranged - before.ranged,
        full: after.full - before.full,
        total: after.total - before.total,
    }
}

fn rga_rt() -> tokio::runtime::Runtime {
    tokio::runtime::Runtime::new().expect("tokio runtime")
}

fn rga_connect(base: &str) -> rust_lib_app::api::source::WebDavSession {
    rga_rt()
        .block_on(async { webdav_connect(base.to_string(), "u".into(), "p".into()).await })
        .expect("webdav_connect must succeed")
}

fn rga_open(
    session: u64,
    path: &str,
    strategy: &str,
) -> anyhow::Result<()> {
    let path = path.to_string();
    let strategy = strategy.to_string();
    rga_rt()
        .block_on(async { open_webdav_book(session, path, strategy).await })
        .map(|_| ())
}

fn rga_disconnect(session: u64) {
    rga_rt().block_on(async { webdav_disconnect(session).await });
}

/// 通过**生产**本地 document 路径读取页（`LocalFile` → `open_document` → `Reader::get_page`）。
fn rga_local_pages(local_path: &std::path::Path, doc_path: &str) -> Vec<Arc<Vec<u8>>> {
    let src = rust_lib_app::source::local::LocalFile::open(local_path).expect("open local file");
    let document = open_document(src, doc_path).expect("open document");
    let pages = document.page_count();
    let reader = Arc::new(Reader::new(document, &format!("rga-a2-{}", perf::now_us())));
    (0..pages).map(|i| reader.get_page(i).expect("get page")).collect()
}

/// A2-1：Range probe 收到 200 ⇒ unsupported ⇒ **进入 ADR-005 整包 fallback**，并原子发布。
#[test]
fn rg_a_a2_1_unsupported_range_triggers_full_download_fallback() {
    let body = build_cbz(4, 64 * 1024);
    let root = rga_tmp_root("a2_1");
    let cdn = MockCdn::start(body.clone(), 0).with_no_range_support();
    let base = rga_base(&cdn);

    let session = rga_connect(&base);
    // 分阶段：connect 的握手/probe 流量不计入 open 的证据。
    let before_open = rga_counts(&cdn.requests());
    let info = rga_open(session.id, "/file.cbz", "stream");
    let after_open = rga_counts(&cdn.requests());
    let open_delta = rga_delta(before_open, after_open);

    assert!(info.is_ok(), "open_webdav_book must succeed via fallback");
    assert_eq!(
        open_delta.full, 1,
        "the open phase must issue exactly one full-body GET (no Range header)"
    );
    assert!(
        open_delta.ranged >= 1,
        "the open phase must probe Range at least once"
    );

    let found = rga_find_named(&root, "file.cbz");
    assert_eq!(found.len(), 1, "fallback must publish exactly one raw cache file");
    assert_eq!(
        std::fs::metadata(&found[0]).unwrap().len(),
        body.len() as u64,
        "published raw cache must have the full object length"
    );
    assert!(
        rga_part_files(&root).is_empty(),
        "atomic publish must leave no .part-*"
    );

    rga_disconnect(session.id);
}

/// A2-2：fallback 产物**真的成为**后续本地 document authority（差分为准，不用 std::fs::read）。
#[test]
fn rg_a_a2_2_fallback_material_becomes_local_document_authority() {
    let body = build_cbz(3, 32 * 1024);
    let root = rga_tmp_root("a2_2");
    let cdn = MockCdn::start(body.clone(), 0).with_no_range_support();
    let base = rga_base(&cdn);

    let session = rga_connect(&base);
    rga_open(session.id, "/file.cbz", "stream").expect("fallback open");
    rga_disconnect(session.id);

    // 基准：同一 CBZ 的**直接本地**打开。
    let baseline_file = root.join("baseline.cbz");
    std::fs::write(&baseline_file, &body).unwrap();
    let baseline = rga_local_pages(&baseline_file, "baseline.cbz");

    // fallback 产物：走同一条生产本地 document 路径。
    let found = rga_find_named(&root, "file.cbz");
    assert_eq!(found.len(), 1, "fallback raw cache must exist");
    let fallback = rga_local_pages(&found[0], "file.cbz");

    assert_eq!(
        baseline.len(),
        fallback.len(),
        "page count must match the same CBZ opened locally"
    );
    for (index, (want, got)) in baseline.iter().zip(fallback.iter()).enumerate() {
        assert_eq!(want, got, "page {index} bytes must match the local baseline");
    }
}

/// A2-3：跨 session 复用 —— **分阶段计数**，handshake 流量不算 cache miss。
///
/// 契约：cache-first 位于 book-level probe **之前** ⇒ 第二次 `open_webdav_book` 阶段
/// 既无 full GET、也无 Range probe。
#[test]
fn rg_a_a2_3_cross_session_reuse_skips_probe_and_download() {
    let body = build_cbz(3, 32 * 1024);
    let root = rga_tmp_root("a2_3");
    let cdn = MockCdn::start(body.clone(), 0).with_no_range_support();
    let base = rga_base(&cdn);

    let first = rga_connect(&base);
    rga_open(first.id, "/file.cbz", "stream").expect("first open (fallback)");
    let published = rga_find_named(&root, "file.cbz");
    assert_eq!(published.len(), 1);
    rga_disconnect(first.id);

    // 新 session（新 client）。connect 自身会产生 PROPFIND / HEAD / probe —— 不计入断言。
    let second = rga_connect(&base);
    let before_open = rga_counts(&cdn.requests());
    let info = rga_open(second.id, "/file.cbz", "stream");
    let after_open = rga_counts(&cdn.requests());
    let open_delta = rga_delta(before_open, after_open);

    assert!(info.is_ok(), "cached open must succeed");
    assert_eq!(
        open_delta.full, 0,
        "cross-session reuse must not download the book body again"
    );
    assert_eq!(
        open_delta.ranged, 0,
        "cached object must skip book-level Range probing entirely"
    );
    assert_eq!(
        rga_find_named(&root, "file.cbz"),
        published,
        "the same raw cache path must be reused"
    );

    rga_disconnect(second.id);
}

/// A2-4：**fallback full GET 中途断流** ⇒ orchestrator 绝不把失败 fallback 当成缓存。
///
/// 与 Step 2 的区别：Step 2 证明 writer；本用例证明 **orchestrator** 的正确性。
#[test]
fn rg_a_a2_4_failed_fallback_cannot_become_cache_authority() {
    let body = build_cbz(4, 1024 * 1024); // ≈4 MiB，确保能在 2 MiB 处断流
    let root = rga_tmp_root("a2_4");
    let cdn = MockCdn::start(body.clone(), 0)
        .with_no_range_support()
        .with_abort_after_full_get(2 * 1024 * 1024);
    let base = rga_base(&cdn);

    // 第一次：probe 正常（Range 被忽略 ⇒ 200 unsupported），只有 full GET 断流。
    let session = rga_connect(&base);
    let failed = rga_open(session.id, "/file.cbz", "stream");
    assert!(
        failed.is_err(),
        "a truncated fallback body must make the orchestration fail"
    );
    assert!(
        rga_find_named(&root, "file.cbz").is_empty(),
        "failed fallback must NOT publish an authoritative raw cache path"
    );
    assert!(
        rga_part_files(&root).is_empty(),
        "failed fallback must clean up its .part-*"
    );

    // 第二次：关闭断流 ⇒ 必须重新整包下载并成功。
    cdn.set_abort_after_full_get(0);
    let before_retry = rga_counts(&cdn.requests());
    let info = rga_open(session.id, "/file.cbz", "stream");
    let after_retry = rga_counts(&cdn.requests());
    let retry_delta = rga_delta(before_retry, after_retry);

    assert!(info.is_ok(), "retry must succeed");
    assert_eq!(
        retry_delta.full, 1,
        "the retry must issue a fresh full-body GET (no false cache hit)"
    );
    let published = rga_find_named(&root, "file.cbz");
    assert_eq!(published.len(), 1);
    assert_eq!(
        std::fs::metadata(&published[0]).unwrap().len(),
        body.len() as u64
    );
    assert!(rga_part_files(&root).is_empty());
    // 产物必须能被生产本地 document 路径打开。
    let pages = rga_local_pages(&published[0], "file.cbz");
    assert_eq!(pages.len(), 4);

    rga_disconnect(session.id);
}

/// A2-5：可信 206 ⇒ 保持 Range-capable，**不触发** ADR-005 fallback。
#[test]
fn rg_a_a2_5_valid_partial_content_stays_range_backed() {
    let body = build_cbz(3, 32 * 1024);
    let root = rga_tmp_root("a2_5");
    let cdn = MockCdn::start(body.clone(), 0); // 正常 Range 服务器（206 + Content-Range）
    let base = rga_base(&cdn);

    let session = rga_connect(&base);
    let before_open = rga_counts(&cdn.requests());
    let info = rga_open(session.id, "/file.cbz", "stream");
    let after_open = rga_counts(&cdn.requests());
    let open_delta = rga_delta(before_open, after_open);

    assert!(info.is_ok(), "Range-backed open must succeed: {:?}", info.as_ref().err());
    assert!(
        open_delta.ranged >= 1,
        "Range-backed path must read via Range requests"
    );
    assert_eq!(
        open_delta.full, 0,
        "a valid 206 must never trigger a full-body download"
    );
    let stray = rga_find_named(&root, "file.cbz");
    assert!(
        stray.is_empty(),
        "Range-backed path must not create a full raw cache object; stray={:?}; open_delta={:?}; drift_total_log={:?}",
        stray,
        open_delta,
        cdn.requests()
    );
    assert!(rga_part_files(&root).is_empty());

    rga_disconnect(session.id);
}

/// A2-6：**非法 206** 必须 fail closed —— 绝不能被解释成 "unsupported" 而进入整包 fallback。
///
/// 断言点以**生产最早拒绝的位置**为准（connect 或 open），不人为指定层级。
#[test]
fn rg_a_a2_6_malformed_partial_content_fails_closed() {
    let body = build_cbz(3, 32 * 1024);
    let root = rga_tmp_root("a2_6");
    let cdn = MockCdn::start(body.clone(), 0).with_malformed_content_range();
    let base = rga_base(&cdn);

    let connected =
        rga_rt().block_on(async { webdav_connect(base.clone(), "u".into(), "p".into()).await });

    match connected {
        Err(err) => {
            // 生产在 connect 阶段就 fail closed（Range probe 在 check_and_probe 内）。
            let text = err.to_string().to_lowercase();
            assert!(
                text.contains("malformed") || text.contains("content_range"),
                "expected a MalformedResponse-class error, got: {err}"
            );
        }
        Ok(session) => {
            // 或者 connect 通过，则在 open 阶段拒绝。
            let opened = rga_open(session.id, "/file.cbz", "stream");
            assert!(
                opened.is_err(),
                "malformed 206 must be rejected rather than treated as unsupported"
            );
            rga_disconnect(session.id);
        }
    }

    // 无论在哪一层拒绝，都必须满足：
    let counts = rga_counts(&cdn.requests());
    assert_eq!(
        counts.full, 0,
        "a malformed 206 must NOT lead to a full-body download"
    );
    assert!(
        rga_find_named(&root, "file.cbz").is_empty(),
        "fail-closed must not create an authoritative raw cache path"
    );
    assert!(rga_part_files(&root).is_empty());
}
