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
    let (start, len) = parse_range(&head, body.len() as u64);
    state
        .arrivals
        .lock()
        .unwrap()
        .push((perf::now_us(), start, len));

    let delay = state.delay_ms.load(Ordering::SeqCst);
    if delay > 0 {
        std::thread::sleep(Duration::from_millis(delay));
    }

    let forced = state.force_status.load(Ordering::SeqCst) as u16;
    if forced != 0 {
        let head = format!("HTTP/1.1 {forced} X\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        let _ = stream.write_all(head.as_bytes());
        let _ = stream.flush();
        return;
    }

    let total = body.len() as u64;
    let end = start.saturating_add(len).min(total);
    let slice = &body[start as usize..end as usize];
    let response_head = format!(
        "HTTP/1.1 206 Partial Content\r\n\
         Content-Length: {}\r\n\
         Content-Range: bytes {}-{}/{}\r\n\
         Accept-Ranges: bytes\r\n\
         Connection: close\r\n\r\n",
        slice.len(),
        start,
        end.saturating_sub(1),
        total
    );
    let _ = stream.write_all(response_head.as_bytes());
    let _ = stream.write_all(slice);
    let _ = stream.flush();
}

/// 解析 `Range: bytes=a-b`；缺失或不可解析时返回整段。
fn parse_range(head: &str, total: u64) -> (u64, u64) {
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
                    return (start, end - start + 1);
                }
            }
        }
    }
    (0, total)
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
