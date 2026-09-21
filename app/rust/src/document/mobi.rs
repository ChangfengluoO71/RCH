//! MOBI 格式解析。
//!
//! 用 mobi crate(v0.8.0) 纯 Rust 解析 MOBI 文件, 提取其中的图片记录作为书页。
//! 不需要 Calibre CLI。
//!
//! **第 81 轮：新增惰性按需读路径。** 过去 `open` 把整份文件读进内存
//! （`vec![0u8; len]`），再把每个图片记录**复制一份**进 `pages: Vec<Vec<u8>>`
//! ⇒ 峰值 ≈ 2× 文件大小；封面路径更是每本都要整包（桌面截图实证：74–79 MB 的有封面
//! 但代价是整本下载，143/146/180 MB 直接撞 `COVER_FETCH_LIMIT_BYTES = 128 MB` 被拒）。
//!
//! 惰性路径：PalmDB 头（78 B，含记录数）+ 记录偏移表（8 B/条）都在**文件头部**
//! ⇒ 一次小读拿到每条记录的 `(offset, len)`；再读 record 0 前 128 B 取
//! `first_image_index`；随后只读各候选记录的**头部 16 B** 做魔数过滤。
//! 取页时只读那一条记录。
//!
//! 字段偏移取自 **mobi crate 自身的解析序列**（`src/headers/mobih.rs` 的
//! `MobiHeader::parse` 按字段顺序顺序读取：identifier/header_length 之后依次是
//! 8×u32(mobi_type..index_keys)、extra_indices[6]、first_non_book_index、
//! name_offset、name_length、unused(u16)、locale(u8)、language_code(u8)、
//! input_language、output_language、format_version、**first_image_index**），
//! 再加 record 0 开头的 PalmDOC 头 16 字节 ⇒ 相对 record 0：
//! name_offset=+84、name_length=+88、first_image_index=+108。
//!
//! **不靠记忆**：任何一项看起来不合理（记录数越界 / 魔数不是 MOBI /
//! `first_image_index` 越界 / 一条可解码图片都没有），就整份回退到 crate 解析，
//! 与第 70/78 轮 ZIP/EPUB 的"先验后回退"同一套路，保证不劣化。

use super::{Document, DocumentMeta};
use crate::source::ByteSource;
use anyhow::{Context, Result};
use std::sync::Arc;

/// PalmDB 头长度：name[32]+attr(u16)+ver(u16)+3×date(u32)+modNum(u32)+appInfo(u32)+
/// sortInfo(u32)+type[4]+creator[4]+seed(u32)+nextList(u32)+numRecords(u16) = 78。
const PALMDB_HEADER_LEN: u64 = 78;
/// 记录表每条 8 字节：4B 偏移 + 1B 属性 + 3B 唯一 id。
const PALMDB_RECORD_INFO_LEN: u64 = 8;
/// MOBI 头在 record 0 内的起点（PalmDOC 头 16 字节之后）。
const MOBI_HEADER_IN_RECORD: u64 = 16;
/// 相对 record 0 的字段偏移。
const RECORD0_NAME_OFFSET: u64 = MOBI_HEADER_IN_RECORD + 68;
const RECORD0_NAME_LENGTH: u64 = MOBI_HEADER_IN_RECORD + 72;
const RECORD0_FIRST_IMAGE_INDEX: u64 = MOBI_HEADER_IN_RECORD + 92;
/// record 0 需要读入的字节数（覆盖到 first_image_index 之后）。
const RECORD0_PROBE_BYTES: u64 = 128;
/// 探测"本构建可解码的图片"所需头部字节数。
const MAGIC_PROBE_BYTES: usize = 16;

/// 魔数探测的并发度（2026-09-21："mobi 还是慢"）。
///
/// 真实瓶颈是 **RTT 而不是带宽**：实测每次远端读 136–182 ms，却只取约 29 字节
/// （200–300 条候选记录 ⇒ 串行打开要 30–40 s）。在 16 B/读 的量级上带宽完全不是约束，
/// 所以并发发这些小探测能把打开时间压到大约 1/workers。
///
/// ⚠️ 与第 81 轮续6 的受控 A/B **不矛盾**：那次是把**一次大读**拆成两半并发，瓶颈是
/// 账号/链路带宽 ⇒ 无收益（`PARALLEL_RANGE_MIN` 默认关闭）。这里并发的是**互不相干的
/// 小探测**（延迟受限），页数据读取仍然串行。
///
/// 可用 `RCH_MOBI_PROBE_WORKERS` 覆盖（受控 A/B 用），默认保守取 4。
const MAGIC_PROBE_WORKERS: usize = 4;
/// 书名长度上限（异常值不做大读）。
const TITLE_MAX_BYTES: u64 = 512;

pub struct MobiBook {
    /// 惰性路径：源 + 每个图片记录的区间（取页时才读）。
    lazy: Option<LazyMobi>,
    /// 回退路径：整份解析后按页持有（历史行为，KF8/AZW3 等）。
    pages: Vec<Vec<u8>>,
    title: String,
}

struct LazyMobi {
    src: Arc<dyn ByteSource>,
    /// 每个图片记录的 `(offset, len)`，按记录顺序即页序。
    records: Vec<(u64, u64)>,
}

impl MobiBook {
    pub fn open(src: impl ByteSource + 'static, path: &str) -> Result<Self> {
        Self::open_with(src, path, false)
    }

    /// 封面专用打开（2026-09-21，修"MOBI 封面不显示 / 部分文件封面获取失败"）。
    ///
    /// 封面只用 **page 0**，而惰性打开要逐条探测候选记录的魔数——远端每条一次 Range 往返。
    /// 一本 300 页的漫画 MOBI 因此要 200–300 次往返（实测平均每次只取 28.8 字节、约 30 s）
    /// ⇒ 撞穿封面 30 s 挂钟预算，`cover_read_budget_exceeded` 在真机日志里累计 281 条。
    /// 封面只要**第一张**可解码图片 ⇒ 探测到它就停（通常 1–3 次）。
    ///
    /// **不改变阅读路径**：完整打开仍走 [`MobiBook::open`]（逐条探测全部候选，页序不变）。
    /// 注意：`page_count()` 在本入口下是 1，这是刻意的——调用方只用 page 0。
    pub fn open_cover(src: impl ByteSource + 'static, path: &str) -> Result<Self> {
        Self::open_with(src, path, true)
    }

    fn open_with(src: impl ByteSource + 'static, path: &str, cover_only: bool) -> Result<Self> {
        let started = std::time::Instant::now();
        let src: Arc<dyn ByteSource> = Arc::new(src);
        let stem = std::path::Path::new(path)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string());
        let file_len = src.len();

        if let Some((lazy, title)) = open_lazy(Arc::clone(&src), &stem, cover_only) {
            diag(&format!(
                "mobi_open mode={} pages={} size={} ms={} name={}",
                if cover_only { "cover-lazy" } else { "lazy" },
                lazy.records.len(),
                file_len,
                started.elapsed().as_millis(),
                stem
            ));
            return Ok(MobiBook {
                lazy: Some(lazy),
                pages: Vec::new(),
                title,
            });
        }

        // 回退：历史行为（整份读入 + crate 解析）。
        // ⚠️ 这一行是"为什么 MOBI 打开要十几秒"的关键嫌疑：回退 = 整本远端读。
        diag(&format!(
            "mobi_open mode=full-fallback size={} ms={} name={}",
            file_len,
            started.elapsed().as_millis(),
            stem
        ));
        let len = src.len() as usize;
        let mut data = vec![0u8; len];
        src.read_exact_at(0, &mut data)
            .context("读取 MOBI 文件失败")?;

        let mobi = mobi::Mobi::new(data).context("解析 MOBI 文件失败")?;

        let title = mobi.title();

        // 提取所有图片记录
        let image_records = mobi.image_records();

        if image_records.is_empty() {
            // 纯文字 MOBI: 尝试从 HTML 内容中解析图片引用 (data: URI)
            anyhow::bail!("MOBI 中没有图片。标题: {title}。此 MOBI 可能是纯文字小说, 暂不支持。");
        }

        let mut pages = Vec::with_capacity(image_records.len());
        for record in image_records {
            // `image_records()` 只挡了**非图片黑名单**（FLIS/FCIS/INDX/SRCS/RESC…），
            // KF8/AZW3 里的 CSS/HTML/其它资源记录会被混进来当"页"：既挤占页序，
            // 又让封面（page 0）永远解不开（③ 实测 33.9MB MOBI ⇒ `cover_decode_failed`）。
            // 这里要求魔数**确实是当前构建可解码的图片**。
            if !crate::decode::image_magic_decodable(crate::decode::sniff_image_magic(
                record.content,
            )) {
                continue;
            }
            pages.push(record.content.to_vec());
        }

        if pages.is_empty() {
            anyhow::bail!("MOBI 中没有可解码的图片记录。标题: {title}。");
        }

        Ok(MobiBook {
            lazy: None,
            pages,
            title,
        })
    }
}

/// 尝试惰性打开。返回 `(LazyMobi, 标题)`；不可行时返回 `None`（调用方整份回退）。
fn open_lazy(
    src: Arc<dyn ByteSource>,
    stem: &str,
    cover_only: bool,
) -> Option<(LazyMobi, String)> {
    let file_len = src.len();
    if file_len < PALMDB_HEADER_LEN + PALMDB_RECORD_INFO_LEN {
        return None;
    }

    // ① PalmDB 头：记录数在偏移 76（u16 BE）。
    let mut header = [0u8; PALMDB_HEADER_LEN as usize];
    src.read_exact_at(0, &mut header).ok()?;
    let record_count = u16::from_be_bytes([header[76], header[77]]) as u64;
    if record_count == 0 {
        return None;
    }

    // ② 记录表：每条 8 字节，前 4 字节为该记录在文件中的偏移（BE）。
    let table_len = record_count.checked_mul(PALMDB_RECORD_INFO_LEN)?;
    if PALMDB_HEADER_LEN.checked_add(table_len)? > file_len {
        return None;
    }
    let mut table = vec![0u8; table_len as usize];
    src.read_exact_at(PALMDB_HEADER_LEN, &mut table).ok()?;
    let mut offsets = Vec::with_capacity(record_count as usize);
    for i in 0..record_count as usize {
        let at = i * PALMDB_RECORD_INFO_LEN as usize;
        let offset =
            u32::from_be_bytes([table[at], table[at + 1], table[at + 2], table[at + 3]]) as u64;
        if offset >= file_len {
            return None;
        }
        offsets.push(offset);
    }

    // ③ MOBI 头的两个候选起点（第 81 轮真机修因）：
    //    · **crate 的权威位置**：`PalmDocHeader::parse` 的注释写明头在
    //      "byte 80 + 8 * num_records"（PalmDB 记录表之后还有 2 字节填充），
    //      mobi crate 就是从这个位置顺序读 PalmDoc 头 + MOBI 头的；
    //    · 记录表里 record 0 的偏移（多数文件与前者相同，但确实存在不一致的写入器）。
    //    谁先命中 `MOBI` 魔数就用谁。
    //
    //    为什么必须两个都试：真机证据显示封面代价**与整本大小成正比**
    //    （33.9/143/146/180 MB 全失败、≤79 MB 全成功）⇒ 说明惰性路径被弃权、
    //    退回了整份读 —— 只有"起点猜错"能解释这种弃权。
    let mut base = None;
    let mut probe_holder: Option<Vec<u8>> = None;
    let header_at = PALMDB_HEADER_LEN + PALMDB_RECORD_INFO_LEN * record_count + 2;
    for candidate in [header_at, offsets[0]] {
        if candidate.checked_add(RECORD0_FIRST_IMAGE_INDEX + 4).is_none()
            || candidate + RECORD0_FIRST_IMAGE_INDEX + 4 > file_len
        {
            continue;
        }
        let probe_len = RECORD0_PROBE_BYTES.min(file_len - candidate);
        let mut probe = vec![0u8; probe_len as usize];
        if src.read_exact_at(candidate, &mut probe).is_err() {
            continue;
        }
        if &probe[MOBI_HEADER_IN_RECORD as usize..MOBI_HEADER_IN_RECORD as usize + 4] == b"MOBI" {
            base = Some(candidate);
            probe_holder = Some(probe);
            break;
        }
    }
    let base = base?;
    let probe = probe_holder?;
    let record0_start = base;
    let first_image_index = u32::from_be_bytes([
        probe[RECORD0_FIRST_IMAGE_INDEX as usize],
        probe[RECORD0_FIRST_IMAGE_INDEX as usize + 1],
        probe[RECORD0_FIRST_IMAGE_INDEX as usize + 2],
        probe[RECORD0_FIRST_IMAGE_INDEX as usize + 3],
    ]) as u64;
    // 第一张图片必须是 record 0 之后的合法记录号，否则说明布局不是我们解析的那一种。
    if first_image_index == 0 || first_image_index >= record_count {
        return None;
    }

    // ④ 逐条区间：offset[i+1] - offset[i]，最后一条到文件尾。
    let range_of = |i: usize| -> Option<(u64, u64)> {
        let start = *offsets.get(i)?;
        let end = offsets.get(i + 1).copied().unwrap_or(file_len);
        if end <= start {
            return None;
        }
        Some((start, end - start))
    };

    // ⑤ 只读每条候选记录的头部 16 B 做魔数过滤（与回退路径同一套判定），
    //    保证页序与历史行为一致：仍是"从 first_image_index 起、可解码的图片记录"。
    // 候选区间（含每条记录的 `(offset, len)`）；任一条长度非法 ⇒ 与旧实现一致地整体弃权。
    let mut candidates: Vec<(u64, u64)> = Vec::new();
    for i in first_image_index as usize..record_count as usize {
        let Some(range) = range_of(i) else {
            return None;
        };
        candidates.push(range);
    }

    let decodable = if cover_only {
        // 封面只读第一张图：串行探测到第一张可解码图片就停（通常 1–3 次读）。
        serial_probe_until_first_image(&src, &candidates)?
    } else {
        // 完整打开：并发探测（见 `MAGIC_PROBE_WORKERS`）。
        concurrent_probe(&src, &candidates)?
    };
    let mut records: Vec<(u64, u64)> = Vec::new();
    for (index, (offset, len)) in candidates.iter().enumerate() {
        if decodable.get(index).copied().unwrap_or(false) {
            records.push((*offset, *len));
        }
    }
    if records.is_empty() {
        return None;
    }

    // ⑥ 标题：优先用 MOBI 头的 full name（偏移相对 record 0），越界/为空则退回文件名。
    let name_offset = u32::from_be_bytes([
        probe[RECORD0_NAME_OFFSET as usize],
        probe[RECORD0_NAME_OFFSET as usize + 1],
        probe[RECORD0_NAME_OFFSET as usize + 2],
        probe[RECORD0_NAME_OFFSET as usize + 3],
    ]) as u64;
    let name_length = u32::from_be_bytes([
        probe[RECORD0_NAME_LENGTH as usize],
        probe[RECORD0_NAME_LENGTH as usize + 1],
        probe[RECORD0_NAME_LENGTH as usize + 2],
        probe[RECORD0_NAME_LENGTH as usize + 3],
    ]) as u64;
    let title = read_title(&src, record0_start, name_offset, name_length)
        .unwrap_or_else(|| stem.to_string());

    Some((LazyMobi { src, records }, title))
}

/// 串行探测：从候选区间的**头部**逐条取 16 B 魔数，命中第一张可解码图片就停。
///
/// 返回与 `candidates` 等长的"可解码"标记（封面路径只会点亮前若干条）。
/// 任一次读失败返回 `None` —— 与历史行为一致（整体弃权、交调用方回退整本解析）。
fn serial_probe_until_first_image(
    src: &Arc<dyn ByteSource>,
    candidates: &[(u64, u64)],
) -> Option<Vec<bool>> {
    let mut decodable = vec![false; candidates.len()];
    for (index, (offset, len)) in candidates.iter().enumerate() {
        let probe_len = (MAGIC_PROBE_BYTES as u64).min(*len) as usize;
        if probe_len == 0 {
            continue;
        }
        let mut magic = [0u8; MAGIC_PROBE_BYTES];
        src.read_exact_at(*offset, &mut magic[..probe_len]).ok()?;
        if !crate::decode::image_magic_decodable(crate::decode::sniff_image_magic(&magic[..probe_len]))
        {
            continue;
        }
        decodable[index] = true;
        break;
    }
    Some(decodable)
}

/// 并发探测（`MAGIC_PROBE_WORKERS` 个 worker）：顺序无关的小读并发取，结果按**原索引**
/// 写回，所以页序与串行版本逐字一致；任一次读失败仍整体返回 `None`（语义不变）。
fn concurrent_probe(
    src: &Arc<dyn ByteSource>,
    candidates: &[(u64, u64)],
) -> Option<Vec<bool>> {
    use std::sync::atomic::{AtomicUsize, Ordering};

    if candidates.is_empty() {
        return Some(Vec::new());
    }
    let workers = std::env::var("RCH_MOBI_PROBE_WORKERS")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(MAGIC_PROBE_WORKERS)
        .min(candidates.len())
        .max(1);

    let next = AtomicUsize::new(0);
    let failed = std::sync::atomic::AtomicBool::new(false);
    let buckets: Vec<std::sync::Mutex<Vec<(usize, bool)>>> =
        (0..workers).map(|_| std::sync::Mutex::new(Vec::new())).collect();

    std::thread::scope(|scope| {
        for bucket in &buckets {
            scope.spawn(|| {
                let mut local: Vec<(usize, bool)> = Vec::new();
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    if index >= candidates.len() || failed.load(Ordering::Relaxed) {
                        break;
                    }
                    let (offset, len) = candidates[index];
                    let probe_len = (MAGIC_PROBE_BYTES as u64).min(len) as usize;
                    if probe_len == 0 {
                        continue;
                    }
                    let mut magic = [0u8; MAGIC_PROBE_BYTES];
                    match src.read_exact_at(offset, &mut magic[..probe_len]) {
                        Ok(()) => local.push((
                            index,
                            crate::decode::image_magic_decodable(
                                crate::decode::sniff_image_magic(&magic[..probe_len]),
                            ),
                        )),
                        Err(_) => {
                            failed.store(true, Ordering::Relaxed);
                            break;
                        }
                    }
                }
                if let Ok(mut slot) = bucket.lock() {
                    *slot = local;
                }
            });
        }
    });

    if failed.load(Ordering::Relaxed) {
        return None;
    }
    let mut decodable = vec![false; candidates.len()];
    for bucket in buckets {
        for (index, ok) in bucket.into_inner().ok()? {
            if let Some(slot) = decodable.get_mut(index) {
                *slot = ok;
            }
        }
    }
    Some(decodable)
}

/// 现场诊断：把 MOBI 打开/取页的关键读数追加到 `<cache_root>/mobi_diag.log`。
///
/// 为什么需要（2026-09-21 用户："mobi 流式阅读加载时间还是长，有时候十几秒"）：
/// MOBI 路径此前**零埋点**（PDF 有 `pdf_diag.log`），"到底走了惰性还是回退整本读"、
/// "一次打开探测了多少条候选记录"都无从查证 —— 与 `pdf_diag` 同一套路子。
/// 只记计数与耗时；超过上限自动截断，绝不影响打开流程。
fn diag(line: &str) {
    use std::io::Write;
    const MOBI_DIAG_MAX_BYTES: u64 = 1024 * 1024;
    let path = crate::cache::cache_root().join("mobi_diag.log");
    if std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0) > MOBI_DIAG_MAX_BYTES {
        let _ = std::fs::remove_file(&path);
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let _ = writeln!(file, "{now} {line}");
    }
}

/// 读 MOBI 头里的书名（长度有上限，避免异常值导致大读）。
fn read_title(
    src: &Arc<dyn ByteSource>,
    record0_start: u64,
    name_offset: u64,
    name_length: u64,
) -> Option<String> {
    if name_length == 0 || name_length > TITLE_MAX_BYTES {
        return None;
    }
    let start = record0_start.checked_add(name_offset)?;
    let mut buf = vec![0u8; name_length as usize];
    src.read_exact_at(start, &mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf).trim().to_string();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

impl Document for MobiBook {
    fn page_count(&self) -> u32 {
        match &self.lazy {
            Some(lazy) => lazy.records.len() as u32,
            None => self.pages.len() as u32,
        }
    }

    fn page_bytes(&self, index: u32) -> Result<Vec<u8>> {
        if let Some(lazy) = &self.lazy {
            let started = std::time::Instant::now();
            let (offset, len) = lazy
                .records
                .get(index as usize)
                .copied()
                .with_context(|| format!("MOBI 页越界: {index}"))?;
            let mut buf = vec![0u8; len as usize];
            lazy.src
                .read_exact_at(offset, &mut buf)
                .with_context(|| format!("读取 MOBI 第 {index} 页失败"))?;
            // 现场读数：一次取页 = **一条记录**（可能 5–15 MB）的远端读，见 `mobi_diag.log`。
            diag(&format!(
                "mobi_page index={} bytes={} ms={}",
                index,
                buf.len(),
                started.elapsed().as_millis()
            ));
            return Ok(buf);
        }
        self.pages
            .get(index as usize)
            .cloned()
            .with_context(|| format!("MOBI 页越界: {index}"))
    }

    fn metadata(&self) -> DocumentMeta {
        DocumentMeta {
            title: self.title.clone(),
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::ByteSource;
    use std::io;
    use std::sync::{Arc, Mutex};

    /// 内存源（测试用）：记录每次读的字节数，用于断言"打开只读少量字节"。
    struct CountingSource {
        data: Vec<u8>,
        read_bytes: Mutex<u64>,
        read_calls: Mutex<u64>,
        /// 当前在途读 / 历史最大并发（用于断言"探测是否真的并发"）。
        in_flight: Mutex<u64>,
        max_in_flight: Mutex<u64>,
        /// 人为延迟，模拟远端 RTT（并发才有效）。
        delay: std::time::Duration,
    }

    impl CountingSource {
        fn new(data: Vec<u8>) -> Self {
            Self::with_delay(data, std::time::Duration::ZERO)
        }
        fn with_delay(data: Vec<u8>, delay: std::time::Duration) -> Self {
            CountingSource {
                data,
                read_bytes: Mutex::new(0),
                read_calls: Mutex::new(0),
                in_flight: Mutex::new(0),
                max_in_flight: Mutex::new(0),
                delay,
            }
        }
        fn max_in_flight(&self) -> u64 {
            *self.max_in_flight.lock().unwrap()
        }
        fn total_read(&self) -> u64 {
            *self.read_bytes.lock().unwrap()
        }
        /// 第 82 轮补：**读次数**才是远端成本的真身（每次 = 一个 Range 往返）。
        fn total_calls(&self) -> u64 {
            *self.read_calls.lock().unwrap()
        }
    }

    impl ByteSource for CountingSource {
        fn len(&self) -> u64 {
            self.data.len() as u64
        }
        fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
            {
                let mut current = self.in_flight.lock().unwrap();
                *current += 1;
                let mut peak = self.max_in_flight.lock().unwrap();
                if *current > *peak {
                    *peak = *current;
                }
            }
            if !self.delay.is_zero() {
                std::thread::sleep(self.delay);
            }
            let offset = offset as usize;
            let n = if offset >= self.data.len() {
                0
            } else {
                let n = buf.len().min(self.data.len() - offset);
                buf[..n].copy_from_slice(&self.data[offset..offset + n]);
                n
            };
            *self.in_flight.lock().unwrap() -= 1;
            if n > 0 {
                *self.read_bytes.lock().unwrap() += n as u64;
                *self.read_calls.lock().unwrap() += 1;
            }
            Ok(n)
        }
    }

    /// 造一个最小可解析的 PalmDB/MOBI：头 + 记录表 + record0(MOBI 头) + N 条图片记录。
    fn synth_mobi(image_records: usize, pad_per_record: usize) -> Vec<u8> {
        let record_count = 1 + image_records;
        let table_len = PALMDB_HEADER_LEN as usize + record_count * 8;
        let record0_len = 256usize;
        let mut offsets = vec![table_len as u32];
        let mut next = table_len + record0_len;
        for _ in 0..image_records {
            offsets.push(next as u32);
            next += pad_per_record;
        }
        let mut data = vec![0u8; next];
        // PalmDB 头：记录数（偏移 76，u16 BE）
        data[76..78].copy_from_slice(&(record_count as u16).to_be_bytes());
        // 记录表
        for (i, offset) in offsets.iter().enumerate() {
            let at = PALMDB_HEADER_LEN as usize + i * 8;
            data[at..at + 4].copy_from_slice(&offset.to_be_bytes());
        }
        // record 0：PalmDOC 头 16 B + "MOBI" + 字段；first_image_index = 1
        let r0 = table_len;
        data[r0 + MOBI_HEADER_IN_RECORD as usize..r0 + MOBI_HEADER_IN_RECORD as usize + 4]
            .copy_from_slice(b"MOBI");
        data[r0 + RECORD0_FIRST_IMAGE_INDEX as usize
            ..r0 + RECORD0_FIRST_IMAGE_INDEX as usize + 4]
            .copy_from_slice(&1u32.to_be_bytes());
        let title = b"Fixture MOBI Title";
        data[r0 + RECORD0_NAME_OFFSET as usize..r0 + RECORD0_NAME_OFFSET as usize + 4]
            .copy_from_slice(&200u32.to_be_bytes());
        data[r0 + RECORD0_NAME_LENGTH as usize..r0 + RECORD0_NAME_LENGTH as usize + 4]
            .copy_from_slice(&(title.len() as u32).to_be_bytes());
        data[r0 + 200..r0 + 200 + title.len()].copy_from_slice(title);
        // 图片记录：PNG 魔数
        for i in 0..image_records {
            let at = offsets[1 + i] as usize;
            data[at..at + 8].copy_from_slice(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        }
        data
    }

    fn first_image_offset(fixture: &[u8]) -> usize {
        let at = PALMDB_HEADER_LEN as usize + 8; // 记录表第 2 条（索引 1）
        u32::from_be_bytes([fixture[at], fixture[at + 1], fixture[at + 2], fixture[at + 3]])
            as usize
    }

    /// 第 81 轮核心断言：惰性打开只读"头 + 记录表 + 探测字节"，
    /// **不随图片记录体积增长**（过去是整份读入，还要把每张图再复制一份）。
    #[test]
    fn lazy_open_reads_only_the_header_and_probes() {
        let fixture = synth_mobi(3, 4 * 1024 * 1024); // 3 条各约 4 MB
        let file_len = fixture.len() as u64;
        let src = Arc::new(CountingSource::new(fixture));
        let (lazy, title) =
            open_lazy(Arc::clone(&src) as Arc<dyn ByteSource>, "t", false).expect("合成文件应走惰性路径");
        assert_eq!(lazy.records.len(), 3, "三条图片记录都要进页表");
        assert_eq!(title, "Fixture MOBI Title");
        let read = src.total_read();
        assert!(
            read < 4096,
            "打开只应读头部与探测字节，实际读了 {read} 字节（文件 {file_len}）"
        );
    }

    /// 第 82 轮补：**封面专用打开**只探测到第一张可解码图片就停。
    ///
    /// 真机签名：完整惰性打开对每条候选记录各发一次远端 Range 探测，300 页的漫画 MOBI
    /// ⇒ 200–300 次往返、约 30 s ⇒ 撞穿封面 30 s 预算（`cover_read_budget_exceeded` 281 条）。
    /// 夹具刻意把记录间隔设成 512 KB（"图很大"的真实形状）：**合并读窗口救不了这种形状**
    /// （记录头相距几百 KB，一次窗口只能覆盖一条），唯一的杠杆是"少探测"。
    #[test]
    fn cover_open_probes_only_until_the_first_image() {
        let src = Arc::new(CountingSource::new(synth_mobi(40, 512 * 1024)));
        let cover = MobiBook::open_cover(Arc::clone(&src) as Arc<dyn ByteSource>, "t.mobi")
            .expect("封面专用打开应走惰性路径");
        assert_eq!(cover.page_count(), 1, "封面入口只保证 page 0");
        let calls = src.total_calls();
        assert!(
            calls <= 6,
            "封面打开只该探测到第一张图为止（头/记录表/record0/首条魔数），实际 {calls} 次读"
        );
        let bytes = src.total_read();
        assert!(bytes < 4096, "封面打开读字节应保持 KB 级，实际 {bytes}");

        // 对照：完整打开仍逐条探测全部候选（40 条 ⇒ ≥40 次读），
        // 证明封面入口没有把**阅读**路径的页序/探测语义改掉。
        let src_full = Arc::new(CountingSource::new(synth_mobi(40, 512 * 1024)));
        let full = MobiBook::open(Arc::clone(&src_full) as Arc<dyn ByteSource>, "t.mobi")
            .expect("完整打开");
        assert_eq!(full.page_count(), 40, "完整打开的页表不得缩水");
        assert!(
            src_full.total_calls() >= 40,
            "完整打开必须逐条探测全部候选（既有语义，封面入口不改变它）"
        );
    }

    /// 2026-09-21（用户："mobi 还是慢"）：**完整打开**的逐条魔数探测必须并发取
    /// （瓶颈是 RTT：实测每次 136–182 ms 却只取 ~29 字节），而**封面入口**必须保持串行
    /// （探测到第一张就停，绝不能为了并发把 40 条都探一遍）。
    /// 页序/页数语义与串行版本一致 —— 由既有测试（跳非图片记录、按记录取页）共同锁定。
    #[test]
    fn full_open_probes_concurrently_while_cover_open_stays_serial() {
        let delay = std::time::Duration::from_millis(3);

        let full_src = Arc::new(CountingSource::with_delay(synth_mobi(40, 512 * 1024), delay));
        let full = MobiBook::open(Arc::clone(&full_src) as Arc<dyn ByteSource>, "t.mobi")
            .expect("完整打开");
        assert_eq!(full.page_count(), 40, "页表不得缩水");
        assert!(
            full_src.max_in_flight() >= 2,
            "完整打开的探测必须并发（观测到的最大并发 = {}）",
            full_src.max_in_flight()
        );

        let cover_src = Arc::new(CountingSource::with_delay(synth_mobi(40, 512 * 1024), delay));
        let cover = MobiBook::open_cover(Arc::clone(&cover_src) as Arc<dyn ByteSource>, "t.mobi")
            .expect("封面打开");
        assert_eq!(cover.page_count(), 1);
        assert_eq!(
            cover_src.max_in_flight(),
            1,
            "封面入口必须保持串行：探测到第一张图就停"
        );
        assert!(cover_src.total_calls() <= 6, "封面打开仍应是常数级读次数");

        // 并发取回的结果必须逐条就位：第 0 页内容与夹具里第一条图片记录一致。
        let page = full_page_bytes(&full, 0);
        assert_eq!(&page[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
    }

    fn full_page_bytes(book: &MobiBook, index: u32) -> Vec<u8> {
        use crate::document::Document;
        book.page_bytes(index).expect("页字节")
    }

    /// 惰性取页只读那一条记录，内容与源一致。
    #[test]
    fn lazy_page_bytes_reads_only_that_record() {
        let mut fixture = synth_mobi(2, 4096);
        let second = first_image_offset(&fixture) + 4096; // 第 2 条图片记录
        fixture[second + 8] = 0xAB; // 魔数之后（前 8 字节是 PNG 魔数，不能动）
        let src = Arc::new(CountingSource::new(fixture));
        let (lazy, _) =
            open_lazy(Arc::clone(&src) as Arc<dyn ByteSource>, "t", false).expect("惰性路径");
        let (offset, len) = lazy.records[1];
        let mut buf = vec![0u8; len as usize];
        src.read_exact_at(offset, &mut buf).unwrap();
        assert_eq!(buf.len(), 4096);
        assert_eq!(buf[8], 0xAB);
    }

    /// 魔数不是图片的记录必须被跳过（与回退路径同一判定，保证页序一致）。
    #[test]
    fn lazy_open_skips_records_that_are_not_decodable_images() {
        let mut fixture = synth_mobi(2, 512);
        let first = first_image_offset(&fixture);
        fixture[first..first + 8].fill(0); // 擦掉第一条的 PNG 魔数
        let src = Arc::new(CountingSource::new(fixture));
        let (lazy, _) =
            open_lazy(Arc::clone(&src) as Arc<dyn ByteSource>, "t", false).expect("仍有一条可解码图片");
        assert_eq!(lazy.records.len(), 1, "不可解码的那条必须被跳过");
    }

    /// 布局不符合预期（record 0 没有 MOBI 魔数）必须放弃惰性路径，交给回退。
    #[test]
    fn lazy_open_declines_when_layout_is_unexpected() {
        let mut fixture = synth_mobi(1, 512);
        let table_len = PALMDB_HEADER_LEN as usize + 2 * 8;
        fixture[table_len + MOBI_HEADER_IN_RECORD as usize] = b'X'; // 破坏 "MOBI"
        let src = Arc::new(CountingSource::new(fixture));
        assert!(
            open_lazy(Arc::clone(&src) as Arc<dyn ByteSource>, "t", false).is_none(),
            "魔数不对时必须返回 None ⇒ 调用方整份回退"
        );
    }

    /// **第 81 轮真机修因的回归**：MOBI 头在 crate 的权威位置 `80 + 8×记录数`
    /// （PalmDB 记录表后有 2 字节填充），而记录表里 record 0 的偏移可能与之不同。
    /// 只认后者会让真机上的书"弃权 ⇒ 退回整份读"（封面代价与整本大小成正比）。
    #[test]
    fn lazy_open_accepts_crate_style_header_position() {
        // 手工搭一个"头在 80+8N"的布局：记录表的 record0 偏移故意指向别处。
        let record_count = 3usize;
        let table_len = PALMDB_HEADER_LEN as usize + record_count * 8;
        let header_at = PALMDB_HEADER_LEN as usize + record_count * 8 + 2; // crate 的位置
        let image_at = header_at + 512;
        let mut data = vec![0u8; image_at + 1024];
        data[76..78].copy_from_slice(&(record_count as u16).to_be_bytes());
        // 记录表：record0 指向一个"假"位置（不是 MOBI 头），图片记录指向 image_at
        let put = |data: &mut Vec<u8>, idx: usize, value: u32| {
            let at = PALMDB_HEADER_LEN as usize + idx * 8;
            data[at..at + 4].copy_from_slice(&value.to_be_bytes());
        };
        put(&mut data, 0, table_len as u32); // 假 record0：文件内的合法偏移，但内容全 0（无 MOBI 魔数）
        put(&mut data, 1, image_at as u32);
        put(&mut data, 2, (image_at + 512) as u32);
        // crate 位置的头：PalmDOC 16 B + "MOBI" + first_image_index = 1
        data[header_at + MOBI_HEADER_IN_RECORD as usize
            ..header_at + MOBI_HEADER_IN_RECORD as usize + 4]
            .copy_from_slice(b"MOBI");
        data[header_at + RECORD0_FIRST_IMAGE_INDEX as usize
            ..header_at + RECORD0_FIRST_IMAGE_INDEX as usize + 4]
            .copy_from_slice(&1u32.to_be_bytes());
        // 两条图片记录（PNG 魔数）
        for at in [image_at, image_at + 512] {
            data[at..at + 8].copy_from_slice(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        }
        let src = Arc::new(CountingSource::new(data));
        let (lazy, _) = open_lazy(Arc::clone(&src) as Arc<dyn ByteSource>, "t", false)
            .expect("crate 式布局必须被接受（否则真机上会退回整份读）");
        assert_eq!(lazy.records.len(), 2);
        assert_eq!(lazy.records[0].0, image_at as u64);
    }
}
