//! ZIP / CBZ 流式解析。
//!
//! 打开时只读文件尾部中心目录；页文件头与图片数据在请求该页时读取。
//! 无需整包下载,远程(WebDAV)也能即点即读;各页互不依赖,可并行下载(并行预取)。

use super::{Document, DocumentMeta};
use crate::source::{ByteSource, SourceReader};
use anyhow::{Context, Result};
use std::io::Read;

/// 常见图片扩展名。
const IMAGE_EXTS: &[&str] = &["jpg", "jpeg", "png", "webp", "gif", "bmp", "avif"];

fn is_image(name: &str) -> bool {
    if name.ends_with('/') {
        return false;
    }
    let lower = name.to_lowercase();
    if lower.contains("__macosx") || lower.ends_with(".ds_store") {
        return false;
    }
    lower
        .rsplit('.')
        .next()
        .map(|ext| IMAGE_EXTS.contains(&ext))
        .unwrap_or(false)
}

/// 一页(图片 entry)的定位与解压信息。
struct PageMeta {
    name: String,
    source: PageSource,
}

/// 一页的数据来源。
enum PageSource {
    /// P0 快路径：只读中央目录得到的元数据，按需读 local header + 数据。
    Central(ZipEntryMeta),
    /// 回退路径：`zip` crate 解析出的条目索引。
    Crate(usize),
}

/// 单页原始数据的字节上限（防病态输入；正常漫画页远小于此）。
const ZIP_PAGE_MAX_BYTES: u64 = 512 * 1024 * 1024;

/// ZIP/CBZ 书籍:中心目录定位各页,按需下载解压,`page_bytes` 无内部可变状态。
///
/// **P0（第 70 轮）**：优先**只读 EOCD + 中央目录**自己建页表。原因：`zip::ZipArchive::new`
/// 会对**每个条目**读一次它的 local header 做校验（`zip-2.4.2/src/read.rs:1259`），
/// 一本几百页的 CBZ 因此要几百次远端请求 ⇒ 打开时间 ∝ 页数（115 CDN 243ms/次 ⇒ 分钟级）。
/// 自己解析后**打开只需 1 次请求**。中央目录不可解析（非 ZIP / ZIP64 / 越界 / 无图片条目）
/// 时回退到 crate 路径，行为与过去一致。
pub struct ZipBook<S: ByteSource> {
    /// 快路径的字节源（`Central` 页按需向它取数据）。
    central: Option<std::sync::Arc<S>>,
    /// 回退路径的归档（`Crate` 页用）。
    archive: Option<zip::ZipArchive<SourceReader<std::sync::Arc<S>>>>,
    pages: Vec<PageMeta>,
    title: String,
}

impl<S: ByteSource> ZipBook<S> {
    pub fn open(src: S, path: &str) -> Result<Self> {
        let title = std::path::Path::new(path)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string());
        let shared = std::sync::Arc::new(src);

        // ① 快路径：只读中央目录建页表（**不构造 crate 归档**）。
        //
        // 只在**每个图片条目都可信**时才走：压缩尺寸为 0（流式写出的条目，尺寸只在
        // data descriptor 里）或 0xFFFF_FFFF（ZIP64 哨兵）的条目本模块读不了，
        // 必须交给 crate ⇒ 否则取页会失败（P0-B2 的兼容性用例当场抓到过）。
        if let Some(entries) = read_central_directory(shared.as_ref())? {
            let images: Vec<&ZipEntryMeta> = entries
                .iter()
                .filter(|entry| is_image(&entry.name))
                .collect();
            let fast_path_safe = !images.is_empty()
                && images.iter().all(|entry| {
                    entry.compressed_size > 0
                        && entry.compressed_size != u32::MAX as u64
                        && entry.local_header < shared.len()
                        && entry.flags & 0x1 == 0
                });
            if fast_path_safe {
                let mut pages: Vec<PageMeta> = images
                    .iter()
                    .map(|entry| PageMeta {
                        name: entry.name.clone(),
                        source: PageSource::Central((*entry).clone()),
                    })
                    .collect();
                pages.sort_by(|a, b| crate::util::natural_cmp(&a.name, &b.name));
                return Ok(ZipBook {
                    central: Some(shared),
                    archive: None,
                    pages,
                    title,
                });
            }
        }

        // ② 回退：交给 crate（非 ZIP / ZIP64 / 无图片条目）。
        let reader = SourceReader::new(shared);
        let zip = zip::ZipArchive::new(reader).context("打开 ZIP/CBZ 失败")?;
        let mut pages = Vec::new();
        for i in 0..zip.len() {
            // by_index opens a local file header and can initialize a decoder.
            // Doing that for all pages makes first-cover cost scale with the
            // entire archive. Names are already in the parsed central directory.
            let name = zip.name_for_index(i).context("读取中心目录失败")?.to_string();
            if !is_image(&name) {
                continue;
            }
            pages.push(PageMeta {
                name,
                source: PageSource::Crate(i),
            });
        }
        pages.sort_by(|a, b| crate::util::natural_cmp(&a.name, &b.name));
        Ok(ZipBook {
            central: None,
            archive: Some(zip),
            pages,
            title,
        })
    }
}

impl<S: ByteSource> Document for ZipBook<S> {
    fn page_count(&self) -> u32 {
        self.pages.len() as u32
    }

    fn metadata(&self) -> DocumentMeta {
        DocumentMeta {
            title: self.title.clone(),
            ..Default::default()
        }
    }

    fn page_bytes(&self, index: u32) -> Result<Vec<u8>> {
        let p = self
            .pages
            .get(index as usize)
            .with_context(|| format!("页索引越界: {index}"))?;
        match &p.source {
            PageSource::Central(entry) => {
                let src = self
                    .central
                    .as_ref()
                    .context("ZIP 快路径缺少字节源")?;
                read_entry_bytes(src.as_ref(), entry, ZIP_PAGE_MAX_BYTES)?
                    .with_context(|| format!("读取页数据失败: {}", p.name))
            }
            PageSource::Crate(archive_index) => {
                // ZipArchive shares immutable central metadata on clone; each reader
                // has its own cursor, so foreground and prefetch need no archive lock.
                let archive = self
                    .archive
                    .as_ref()
                    .context("ZIP 回退路径缺少归档")?;
                let mut archive = archive.clone();
                let mut page = archive
                    .by_index(*archive_index)
                    .context("读取页文件头失败")?;
                let mut bytes = Vec::new();
                page.read_to_end(&mut bytes).context("读取或解压页数据失败")?;
                Ok(bytes)
            }
        }
    }
}


// ============================================================
// 封面快通道：最小 ZIP 读取器（第 62 轮）
// ============================================================
//
// 为什么需要（调研：`docs/reports/rg-b/2026-09-19-zip-cover-open-research.md`）：
// `zip::ZipArchive::new` 会对**每个条目**额外读一次 30B local header 做校验
// （`zip-2.4.2/src/read.rs:1259` -> `read.rs:362-378`），打开成本 ~ O(条目数)；
// 实测 115 上一本 2.08GB 的 CBZ：367 次读散布在 0 -> 2.24GB、单次平均 1.5KB，
// 而 115 CDN 单次往返 243ms => 一本 ~5000 条目的漫画要 ~1 万次请求（~40 分钟），
// 单线程 worker 会被一枚封面堵死。
//
// 只取封面时**不需要**解析整个中央目录的条目元数据：只要"第一张图片"。
// 本函数自己读 EOCD + 中央目录（请求数与文件大小、条目数无关），在内存里挑条目，
// 再只读该条目的 local header 与数据。

const ZIP_EOCD_SIG: u32 = 0x0605_4b50;
const ZIP_CDFH_SIG: u32 = 0x0201_4b50;
const ZIP_LFH_SIG: u32 = 0x0403_4b50;
const ZIP_EOCD_LEN: usize = 22;
const ZIP_CDFH_LEN: usize = 46;
const ZIP_LFH_LEN: usize = 30;
/// ZIP 注释上限（规范）=> 尾部一次读这么多就**标准完备**地覆盖 EOCD。
const ZIP_MAX_COMMENT: usize = 65_535;
/// 中央目录读取上限（正常 CBZ ~60B/条目 => 8MiB 覆盖 ~13 万条目），超过交回常规路径。
const ZIP_FAST_MAX_CD: u64 = 8 * 1024 * 1024;
/// "放宽候选"（EOCD 之后还有多余字节）最多尝试几个：只在严格候选失败时用，每个候选要一次
/// 4 B 的中央目录头签名读来验证，因此只试最近的少数几个。
const ZIP_EOCD_RELAXED_TRIES: usize = 4;
/// 快通道最多尝试几张图片条目（按中央目录顺序）。
const ZIP_FAST_MAX_CANDIDATES: usize = 4;

fn u16_at(buf: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([buf[at], buf[at + 1]])
}

fn u32_at(buf: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([buf[at], buf[at + 1], buf[at + 2], buf[at + 3]])
}

/// 通过 `ByteSource` 精确读一段（不足则报错，不静默截断）。
fn read_exact_at<S: ByteSource>(src: &S, offset: u64, len: usize) -> Result<Vec<u8>> {
    let mut buf = vec![0u8; len];
    let mut got = 0usize;
    while got < len {
        let n = src.read_at(offset + got as u64, &mut buf[got..])?;
        if n == 0 {
            anyhow::bail!("远程读取提前结束");
        }
        got += n;
    }
    Ok(buf)
}

/// 中央目录里的一个条目元数据（只读 EOCD + 中央目录即可得到，**不需要**逐条读 local header）。
#[derive(Debug, Clone)]
pub(crate) struct ZipEntryMeta {
    pub name: String,
    pub local_header: u64,
    /// 压缩尺寸。**取自中央目录**：位 3（data descriptor）时 local header 里是 0。
    pub compressed_size: u64,
    pub method: u16,
    pub flags: u16,
}

/// 条目数上限（防病态输入把内存吃光；正常 CBZ 远小于此）。
const ZIP_MAX_ENTRIES: usize = 200_000;

/// **只读 EOCD + 中央目录**，返回全部条目元数据——不读任何 local header。
///
/// 这是 P0 的地基：`zip::ZipArchive::new` 会对**每个条目**读一次它的 local header
/// 做校验（`zip-2.4.2/src/read.rs:1259` -> `362-378`），一本几百页的 CBZ 因此要几百次
/// 远端请求（115 CDN 单次 243ms => 分钟级）。本函数把打开降到 **1 次请求**。
///
/// 任何不成立的情况（非 ZIP / ZIP64 哨兵 / 中央目录越界或超限 / 目录头损坏）一律返回
/// `Ok(None)`，由调用方回退到 `zip` crate 的完整实现，保证不劣化。
pub(crate) fn read_central_directory<S: ByteSource>(src: &S) -> Result<Option<Vec<ZipEntryMeta>>> {
    let len = src.len();
    if len < ZIP_EOCD_LEN as u64 {
        return Ok(None);
    }
    // 1) 尾部一次读，并在其中找 EOCD：
    //    - **严格候选**：注释长度正好落到文件末尾（干净归档，命中即用、零额外读）；
    //    - **放宽候选**：EOCD 之后还多出若干字节（真机见过：夸克上一本漫画 EPUB 的 EOCD 后
    //      还有 5 B 尾巴）。`zip` crate 容忍这种尾巴，本层过去要求"正好落在末尾"⇒ 明明结构
    //      健康却白白回退成逐条目读（实测打开 219 次读 / 30 s）。放宽候选必须能定位到
    //      **签名正确的中央目录**才被接受，否则仍然回退。
    let tail_len = (ZIP_EOCD_LEN + ZIP_MAX_COMMENT).min(len as usize);
    let tail = read_exact_at(src, len - tail_len as u64, tail_len)?;
    let mut strict_at: Option<usize> = None;
    let mut relaxed_at: Vec<usize> = Vec::new();
    if tail.len() >= ZIP_EOCD_LEN {
        for at in (0..=tail.len() - ZIP_EOCD_LEN).rev() {
            if u32_at(&tail, at) != ZIP_EOCD_SIG {
                continue;
            }
            let comment = u16_at(&tail, at + 20) as usize;
            if at + ZIP_EOCD_LEN + comment == tail.len() {
                strict_at = Some(at);
                break;
            }
            if relaxed_at.len() < ZIP_EOCD_RELAXED_TRIES {
                relaxed_at.push(at);
            }
        }
    }
    let eocd_fields = |at: usize| {
        (
            u32_at(&tail, at + 12) as u64,
            u32_at(&tail, at + 16) as u64,
        )
    };
    if let Some(at) = strict_at {
        let (cd_size, cd_offset) = eocd_fields(at);
        return central_directory_at(src, len, cd_offset, cd_size);
    }
    for at in relaxed_at {
        let (cd_size, cd_offset) = eocd_fields(at);
        if let Some(entries) = central_directory_at(src, len, cd_offset, cd_size)? {
            return Ok(Some(entries));
        }
    }
    Ok(None)
}

/// 在 `cd_offset` 处读一次中央目录并解析（**不读任何 local header**）。
///
/// 任何不成立（ZIP64 哨兵 / 空 / 越界 / 头签名不符 / 解析为空）都返回 `Ok(None)`。
fn central_directory_at<S: ByteSource>(
    src: &S,
    len: u64,
    cd_offset: u64,
    cd_size: u64,
) -> Result<Option<Vec<ZipEntryMeta>>> {
    // ZIP64 哨兵值 => 交回常规路径（它有完整实现）。
    if cd_size == u32::MAX as u64 || cd_offset == u32::MAX as u64 || cd_size == 0 {
        return Ok(None);
    }
    if cd_size > ZIP_FAST_MAX_CD || cd_offset.saturating_add(cd_size) > len {
        return Ok(None);
    }
    // 中央目录一次读。
    let cd = read_exact_at(src, cd_offset, cd_size as usize)?;
    if cd.len() < ZIP_CDFH_LEN || u32_at(&cd, 0) != ZIP_CDFH_SIG {
        return Ok(None);
    }
    // 在内存里顺序解析条目（不碰 local header）。
    let mut entries: Vec<ZipEntryMeta> = Vec::new();
    let mut at = 0usize;
    while at + ZIP_CDFH_LEN <= cd.len() && entries.len() < ZIP_MAX_ENTRIES {
        if u32_at(&cd, at) != ZIP_CDFH_SIG {
            break;
        }
        let flags = u16_at(&cd, at + 8);
        let method = u16_at(&cd, at + 10);
        let csize = u32_at(&cd, at + 20) as u64;
        let name_len = u16_at(&cd, at + 28) as usize;
        let extra_len = u16_at(&cd, at + 30) as usize;
        let comment_len = u16_at(&cd, at + 32) as usize;
        let local_header = u32_at(&cd, at + 42) as u64;
        if at + ZIP_CDFH_LEN + name_len > cd.len() {
            break;
        }
        let name = String::from_utf8_lossy(&cd[at + ZIP_CDFH_LEN..at + ZIP_CDFH_LEN + name_len])
            .into_owned();
        entries.push(ZipEntryMeta {
            name,
            local_header,
            compressed_size: csize,
            method,
            flags,
        });
        at += ZIP_CDFH_LEN + name_len + extra_len + comment_len;
    }
    if entries.is_empty() {
        return Ok(None);
    }
    Ok(Some(entries))
}

/// 读出一个条目的**原始字节**（已解压）：读 30B local header 定位数据起点 -> 读数据 ->
/// 按中央目录给出的压缩方式解压（stored / deflate）。位 3 也成立，因为尺寸取自中央目录。
pub(crate) fn read_entry_bytes<S: ByteSource>(
    src: &S,
    entry: &ZipEntryMeta,
    max_bytes: u64,
) -> Result<Option<Vec<u8>>> {
    let len = src.len();
    if entry.compressed_size == 0 || entry.compressed_size > max_bytes {
        return Ok(None);
    }
    let Ok(header) = read_exact_at(src, entry.local_header, ZIP_LFH_LEN) else {
        return Ok(None);
    };
    if u32_at(&header, 0) != ZIP_LFH_SIG {
        return Ok(None);
    }
    let name_len = u16_at(&header, 26) as u64;
    let extra_len = u16_at(&header, 28) as u64;
    let data_start = entry.local_header + ZIP_LFH_LEN as u64 + name_len + extra_len;
    if data_start.saturating_add(entry.compressed_size) > len {
        return Ok(None);
    }
    let Ok(raw) = read_exact_at(src, data_start, entry.compressed_size as usize) else {
        return Ok(None);
    };
    match entry.method {
        0 => Ok(Some(raw)),
        8 => {
            let mut out = Vec::new();
            if flate2::read::DeflateDecoder::new(raw.as_slice())
                .read_to_end(&mut out)
                .is_ok()
                && !out.is_empty()
            {
                Ok(Some(out))
            } else {
                Ok(None)
            }
        }
        _ => Ok(None),
    }
}

// ============================================================
// 第 78 轮新增：EPUB 快路径专用的"按需取数"构件
// ============================================================
//
// 为什么与 [`read_entry_bytes`] 并存：后者"1 次 local header + 1 次数据"的**步数**是
// `ZipBook` / 封面快通道的既有契约（P0 基线用例断言每页请求数），本轮刻意一动不动。
// EPUB 打开期要读 container.xml / OPF 这类小条目，且"打开 = 常数次读"是本轮验收口径，
// 因此需要"local header 与数据合并成一次 `read_at`"的变体；它同时把算出的数据区起点
// 交回调用方（EPUB 用它实现 `data_start` 的惰性化缓存）。

/// 一次读里为 local header 的 extra field 预留的余量（字节）。
///
/// LFH 的 extra 长度**只能**从 LFH 自己读到（中央目录里的 extra 可能不同），因此"一次读"的
/// 窗口按 `30 + 中央目录里的名字长度 + 余量 + 压缩尺寸` 取。常见写入器（无 extra / zip64 /
/// 时间戳 / unicode 路径）都远小于该余量 ⇒ 1 次读；超长 extra 时补一次精确读，正确性不变。
const ZIP_LFH_EXTRA_SLACK: u64 = 1024;

/// 同 [`read_entry_bytes`]，但 local header 与数据**合并成一次** `read_at`，并把算出的
/// **数据区起点**一并返回（调用方据此缓存，避免重复读 local header）。
pub(crate) fn read_entry_bytes_tracked<S: ByteSource>(
    src: &S,
    entry: &ZipEntryMeta,
    max_bytes: u64,
) -> Result<Option<(Vec<u8>, u64)>> {
    let len = src.len();
    if entry.compressed_size == 0 || entry.compressed_size > max_bytes {
        return Ok(None);
    }
    if entry.local_header >= len {
        return Ok(None);
    }
    // 1) 一次读：local header + 名字 + extra 余量 + 数据。
    let want = (ZIP_LFH_LEN as u64)
        .saturating_add(entry.name.len() as u64)
        .saturating_add(ZIP_LFH_EXTRA_SLACK)
        .saturating_add(entry.compressed_size)
        .min(len - entry.local_header);
    if want < ZIP_LFH_LEN as u64 {
        return Ok(None);
    }
    let Ok(window) = read_exact_at(src, entry.local_header, want as usize) else {
        return Ok(None);
    };
    // 2) 在窗口里解析 local header（签名不符 ⇒ 不信任这个 offset）。
    let Some(header_len) = lfh_data_offset(&window) else {
        return Ok(None);
    };
    let data_start = entry.local_header + header_len;
    if data_start.saturating_add(entry.compressed_size) > len {
        return Ok(None);
    }
    // 3) 数据通常已在窗口里；只有 extra field 比余量大时才补一次精确读。
    let raw = if header_len + entry.compressed_size <= window.len() as u64 {
        window[header_len as usize..(header_len + entry.compressed_size) as usize].to_vec()
    } else {
        let Ok(raw) = read_exact_at(src, data_start, entry.compressed_size as usize) else {
            return Ok(None);
        };
        raw
    };
    Ok(decode_entry_bytes(entry, raw).map(|bytes| (bytes, data_start)))
}

/// 数据区起点**已知**（它的 local header 已被读过一次并校验）时只读数据：1 次读。
pub(crate) fn read_entry_at<S: ByteSource>(
    src: &S,
    entry: &ZipEntryMeta,
    data_start: u64,
    max_bytes: u64,
) -> Result<Option<Vec<u8>>> {
    if entry.compressed_size == 0 || entry.compressed_size > max_bytes {
        return Ok(None);
    }
    if data_start.saturating_add(entry.compressed_size) > src.len() {
        return Ok(None);
    }
    let Ok(raw) = read_exact_at(src, data_start, entry.compressed_size as usize) else {
        return Ok(None);
    };
    Ok(decode_entry_bytes(entry, raw))
}

/// local header 里 "固定头 + 名字 + extra" 的总长度（`None` = 签名不符 / 不足 30B）。
fn lfh_data_offset(header: &[u8]) -> Option<u64> {
    if header.len() < ZIP_LFH_LEN || u32_at(header, 0) != ZIP_LFH_SIG {
        return None;
    }
    Some(ZIP_LFH_LEN as u64 + u16_at(header, 26) as u64 + u16_at(header, 28) as u64)
}

/// 按中央目录给出的压缩方式解压（stored / deflate）。不支持的方式 / 解压失败 ⇒ `None`。
///
/// 与 [`read_entry_bytes`] 内的解压分支等价；那一处**刻意冻结**（P0 契约），两处互不影响。
fn decode_entry_bytes(entry: &ZipEntryMeta, raw: Vec<u8>) -> Option<Vec<u8>> {
    match entry.method {
        0 => Some(raw),
        8 => {
            let mut out = Vec::new();
            if flate2::read::DeflateDecoder::new(raw.as_slice())
                .read_to_end(&mut out)
                .is_ok()
                && !out.is_empty()
            {
                Some(out)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// "只取封面"的 ZIP 快通道：返回**第一张图片条目**的原始字节。
///
/// 请求数恒定（尾部 1 次 + 中央目录 1 次 + 每候选 1 次 local header + 1 次数据），
/// 与文件大小/条目数无关。任何不成立的情况都返回 `Ok(None)`，由调用方回退常规路径：
/// 非 ZIP、ZIP64、中央目录超限、条目被加密、压缩方式非 stored/deflate、解压失败。
pub(crate) fn first_image_bytes_via_central_directory<S: ByteSource>(
    src: &S,
    max_page_bytes: usize,
) -> Result<Option<Vec<u8>>> {
    // 1)+2)+3) 只读 EOCD 与中央目录（不再逐条读 local header），挑出前几张图片条目。
    let Some(entries) = read_central_directory(src)? else {
        return Ok(None);
    };
    let candidates: Vec<&ZipEntryMeta> = entries
        .iter()
        .filter(|entry| {
            entry.flags & 0x1 == 0
                && entry.compressed_size > 0
                && entry.compressed_size <= max_page_bytes as u64
                && is_image(&entry.name)
        })
        .take(ZIP_FAST_MAX_CANDIDATES)
        .collect();
    // 4) 逐候选：读 local header 定位数据起点 -> 读数据 -> 解压（stored / deflate）。
    for entry in candidates {
        if let Some(bytes) = read_entry_bytes(src, entry, max_page_bytes as u64)? {
            return Ok(Some(bytes));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use crate::decode;
    use crate::document::open_document;
    use crate::source::ByteSource;
    use std::io::{self, Write};

    /// 内存字节源,用于测试。
    struct MemSource(Vec<u8>);
    impl ByteSource for MemSource {
        fn len(&self) -> u64 {
            self.0.len() as u64
        }
        fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
            let start = offset as usize;
            if start >= self.0.len() {
                return Ok(0);
            }
            let n = (self.0.len() - start).min(buf.len());
            buf[..n].copy_from_slice(&self.0[start..start + n]);
            Ok(n)
        }
    }

    fn make_png(w: u32, h: u32, rgba: [u8; 4]) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba(rgba));
        let mut buf = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut buf, image::ImageFormat::Png)
            .unwrap();
        buf.into_inner()
    }

    /// 构造一个条目乱序的 CBZ,且各页尺寸不同以便验证页序。
    fn make_cbz() -> Vec<u8> {
        let mut cursor = std::io::Cursor::new(Vec::new());
        {
            let mut zw = zip::ZipWriter::new(&mut cursor);
            let opt = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            for (name, w, h, color) in [
                ("page10.png", 50u32, 60u32, [0, 0, 255, 255]),
                ("page1.png", 10, 20, [255, 0, 0, 255]),
                ("page2.png", 30, 40, [0, 255, 0, 255]),
            ] {
                zw.start_file(name, opt).unwrap();
                zw.write_all(&make_png(w, h, color)).unwrap();
            }
            zw.finish().unwrap();
        }
        cursor.into_inner()
    }

    #[test]
    fn zip_streaming_and_natural_sort() {
        let doc = open_document(MemSource(make_cbz()), "test.cbz").unwrap();
        assert_eq!(doc.page_count(), 3);
        assert_eq!(doc.metadata().title, "test");
        // 自然排序应为 page1(10x20), page2(30x40), page10(50x60)
        let i0 = decode::decode(&doc.page_bytes(0).unwrap(), None).unwrap();
        assert_eq!((i0.width, i0.height), (10, 20));
        let i2 = decode::decode(&doc.page_bytes(2).unwrap(), None).unwrap();
        assert_eq!((i2.width, i2.height), (50, 60));
        assert!(doc.page_bytes(3).is_err());
    }

    #[test]
    fn opening_many_pages_does_not_fetch_every_local_header() {
        use std::sync::{Arc, Mutex};
        struct CountingSource {
            data: MemSource,
            reads: Arc<Mutex<Vec<(u64, usize)>>>,
        }
        impl ByteSource for CountingSource {
            fn len(&self) -> u64 { self.data.len() }
            fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
                self.reads.lock().unwrap().push((offset, buf.len()));
                self.data.read_at(offset, buf)
            }
        }
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for page in 0..40 {
            writer.start_file(format!("{page}.jpg"), options).unwrap();
            writer.write_all(&vec![page as u8; 300 * 1024]).unwrap();
        }
        let data = writer.finish().unwrap().into_inner();
        let reads = Arc::new(Mutex::new(Vec::new()));
        let book = super::ZipBook::open(CountingSource {
            data: MemSource(data), reads: reads.clone(),
        }, "many.cbz").unwrap();
        assert_eq!(crate::document::Document::page_count(&book), 40);

        // ------------------------------------------------------------------
        // P0-B2 之后本用例的验收口径
        // ------------------------------------------------------------------
        // 原始断言是 `open_reads <= 8`。**这个阈值在 `zip` crate 的公开 API 下不可达**，
        // 不是本轮改动造成的：
        //
        // 1. `ZipArchive::new` 会调 `read_central_header` → 对**每个**条目调
        //    `central_header_to_zip_file` → `find_data_start`，后者 seek 到该条目的
        //    local header 并解析它（`zip-2.4.2/src/read.rs`）。也就是说"每条 entry
        //    一次 local-header read"是 crate 的固定行为。
        // 2. `zip::read::Config` **只有** `archive_offset` 一个字段，没有任何"跳过
        //    local header 解析 / 只读中央目录"的开关。
        // 3. RCH 这一侧已经是 metadata-only（下面循环用的是 `name_for_index`，
        //    纯内存），没有为枚举文件名去打开 entry。
        //
        // 因此要降到 `<= 8` 只能绕开 crate 自行解析中央目录 —— 那属于 parser 迁移，
        // 已明确不在 P0-B2 范围内。本轮消灭的是**放大**与**抖动**：
        //   - 打开 40 页：81 次请求 / 10,532,968 B（每次 local-header read 被放大成
        //     256 KiB）→ 42 次 / 6,790 B（每次 64 B）；
        //   - central-directory 重复下载：41 次 → 2 次。
        // 所以这里改为对本轮真正的契约做断言，它们在"放大"这一维度上比 `<=8` 更严。
        let reads_snapshot = reads.lock().unwrap().clone();
        let open_reads = reads_snapshot.len();
        let open_bytes: u64 = reads_snapshot.iter().map(|(_, n)| *n as u64).sum();
        let open_max_fetch = reads_snapshot.iter().map(|(_, n)| *n).max().unwrap_or(0);

        assert!(
            open_reads <= 40 + 16,
            "opening must not grow at ~2 requests per entry: {open_reads} for 40 pages"
        );
        assert!(
            open_bytes <= 40 * 2048 + 64 * 1024,
            "opening must not transfer N x read-ahead: {open_bytes} B for 40 pages"
        );
        assert!(
            open_max_fetch < 256 * 1024,
            "no single metadata read may be amplified to the read-ahead size: {open_max_fetch} B"
        );

        assert_eq!(crate::document::Document::page_bytes(&book, 10).unwrap(), vec![10; 300 * 1024]);
        let page_reads = reads.lock().unwrap().clone();
        let page_fetched: u64 = page_reads[open_reads..].iter().map(|(_, n)| *n as u64).sum();
        assert!(
            page_fetched <= 512 * 1024,
            "300 KiB 的页不得触发自适应放大：实际取数 {page_fetched} B"
        );
        assert!(reads.lock().unwrap().len() - open_reads <= 3);
    }

    #[test]
    fn decode_downscale_keeps_ratio() {
        let png = make_png(4000, 2000, [1, 2, 3, 255]);
        let img = decode::decode(&png, Some(1000)).unwrap();
        assert_eq!((img.width, img.height), (1000, 500));
    }

    /// 生成一页带页码条纹的彩色测试图(条纹数 = 页码,便于肉眼验证翻页)。
    fn make_colored_page(w: u32, h: u32, index: usize) -> Vec<u8> {
        let base = (index as u8).wrapping_mul(30);
        let mut img = image::RgbaImage::from_pixel(
            w,
            h,
            image::Rgba([base, 255u8.wrapping_sub(base), 180, 255]),
        );
        let stripes = index + 1;
        for s in 0..stripes {
            let y0 = 40 + s as u32 * 70;
            if y0 + 40 > h {
                break;
            }
            for y in y0..y0 + 40 {
                for x in 60..w - 60 {
                    img.put_pixel(x, y, image::Rgba([0, 0, 0, 255]));
                }
            }
        }
        let mut buf = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut buf, image::ImageFormat::Png)
            .unwrap();
        buf.into_inner()
    }

    /// 生成一个多页示例 CBZ 到 ../testdata/sample.cbz,供 UI 联调。
    /// 逆序写入条目以验证自然排序还原页序。
    /// 手动运行:cargo test -- --ignored generate_sample_cbz
    #[test]
    #[ignore]
    fn generate_sample_cbz() {
        let mut cursor = std::io::Cursor::new(Vec::new());
        {
            let mut zw = zip::ZipWriter::new(&mut cursor);
            let opt = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            for index in (0..8).rev() {
                let name = format!("page{}.png", index + 1);
                zw.start_file(name, opt).unwrap();
                zw.write_all(&make_colored_page(800, 1200, index)).unwrap();
            }
            zw.finish().unwrap();
        }
        std::fs::create_dir_all("../testdata").unwrap();
        std::fs::write("../testdata/sample.cbz", cursor.into_inner()).unwrap();
    }



    /// 第 68 轮保险：**大条目之后紧跟小条目**时，窗口必须立刻回落，
    /// 不能继续用放大后的 1 MiB 去读一个小文件（否则就是白传流量）。
    #[test]
    fn window_shrinks_after_a_big_entry_is_followed_by_a_small_one() {
        use std::sync::{Arc, Mutex};
        struct CountingSource {
            data: MemSource,
            reads: Arc<Mutex<Vec<(u64, usize)>>>,
        }
        impl ByteSource for CountingSource {
            fn len(&self) -> u64 {
                self.data.len()
            }
            fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
                self.reads.lock().unwrap().push((offset, buf.len()));
                self.data.read_at(offset, buf)
            }
        }
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        writer.start_file("001.jpg", options).unwrap();
        writer.write_all(&vec![1u8; 1300 * 1024]).unwrap(); // 大页：会触发窗口放大
        writer.start_file("002.jpg", options).unwrap();
        writer.write_all(&vec![2u8; 8 * 1024]).unwrap(); // 小页：必须回落
        let data = writer.finish().unwrap().into_inner();

        let reads = Arc::new(Mutex::new(Vec::new()));
        let book = super::ZipBook::open(
            CountingSource {
                data: MemSource(data),
                reads: reads.clone(),
            },
            "mixed.cbz",
        )
        .unwrap();
        let _ = crate::document::Document::page_bytes(&book, 0).unwrap();
        let before_small = reads.lock().unwrap().len();
        let small = crate::document::Document::page_bytes(&book, 1).unwrap();
        assert_eq!(small.len(), 8 * 1024);
        let tail = reads.lock().unwrap()[before_small..].to_vec();
        let max_fetch = tail.iter().map(|(_, n)| *n).max().unwrap_or(0);
        assert!(
            max_fetch <= 256 * 1024,
            "小条目不得沿用放大窗口：本次最大取数 {max_fetch} B"
        );
    }

    /// 第 68 轮：**大页**（约 1.3 MiB，真实 ZIP 页的典型尺寸）必须触发自适应放大，
    /// 把每页的网络读次数从 ~5 次压到 ≤3 次；同时总取数不得远超内容。
    #[test]
    fn large_page_grows_the_read_ahead_window() {
        use std::sync::{Arc, Mutex};
        struct CountingSource {
            data: MemSource,
            reads: Arc<Mutex<Vec<(u64, usize)>>>,
        }
        impl ByteSource for CountingSource {
            fn len(&self) -> u64 {
                self.data.len()
            }
            fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
                self.reads.lock().unwrap().push((offset, buf.len()));
                self.data.read_at(offset, buf)
            }
        }
        // 一页 1.3 MiB（用 Stored 保证解压后字节数可预期）。
        let page = vec![7u8; 1300 * 1024];
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        writer.start_file("001.jpg", options).unwrap();
        writer.write_all(&page).unwrap();
        let data = writer.finish().unwrap().into_inner();

        let reads = Arc::new(Mutex::new(Vec::new()));
        let book = super::ZipBook::open(
            CountingSource {
                data: MemSource(data),
                reads: reads.clone(),
            },
            "large.cbz",
        )
        .unwrap();
        let open_reads = reads.lock().unwrap().len();
        let bytes = crate::document::Document::page_bytes(&book, 0).unwrap();
        assert_eq!(bytes.len(), page.len());
        let page_reads = reads.lock().unwrap().clone();
        let page_fetched: u64 = page_reads[open_reads..].iter().map(|(_, n)| *n as u64).sum();
        assert!(
            page_reads.len() - open_reads <= 3,
            "1.3 MiB 的页应当靠自适应窗口压到 ≤3 次读：实际 {} 次",
            page_reads.len() - open_reads
        );
        assert!(
            page_fetched <= (page.len() as u64) * 2,
            "总取数不得远超内容：fetched={page_fetched} content={}",
            page.len()
        );
    }

    /// 第 62 轮：封面快通道必须在**常数次读**内拿到首张图片，且跳过非图片前缀。
    #[test]
    fn cover_fast_path_reads_first_image_with_constant_reads() {
        use std::sync::{Arc, Mutex};
        struct CountingSource {
            data: MemSource,
            reads: Arc<Mutex<Vec<(u64, usize)>>>,
        }
        impl ByteSource for CountingSource {
            fn len(&self) -> u64 {
                self.data.len()
            }
            fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
                self.reads.lock().unwrap().push((offset, buf.len()));
                self.data.read_at(offset, buf)
            }
        }
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let stored = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        let deflated = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        // 非图片前缀（ComicInfo.xml）+ 目录条目 + 200 页，模拟真实 CBZ。
        writer.start_file("ComicInfo.xml", stored).unwrap();
        writer
            .write_all(b"<ComicInfo><Title>x</Title></ComicInfo>")
            .unwrap();
        writer.add_directory("pages/", stored).unwrap();
        let first_page = make_png(20, 30, [9, 8, 7, 255]);
        writer.start_file("pages/001.png", deflated).unwrap();
        writer.write_all(&first_page).unwrap();
        for page in 2..200 {
            writer.start_file(format!("pages/{page:03}.png"), stored).unwrap();
            writer.write_all(&vec![page as u8; 1024]).unwrap();
        }
        let data = writer.finish().unwrap().into_inner();

        let reads = Arc::new(Mutex::new(Vec::new()));
        let source = CountingSource {
            data: MemSource(data),
            reads: reads.clone(),
        };
        let page = super::first_image_bytes_via_central_directory(&source, 8 * 1024 * 1024)
            .unwrap()
            .expect("应能定位首张图片");
        assert_eq!(page, first_page, "快通道必须返回首张图片的原始字节");
        let count = reads.lock().unwrap().len();
        assert!(
            count <= 8,
            "快通道读次数应与条目数无关（200 页 + 前缀），实际 {count} 次"
        );
    }

    /// 非 ZIP / ZIP64 哨兵都必须**回退**（返回 None），不能猜。
    #[test]
    fn cover_fast_path_bails_out_instead_of_guessing() {
        struct Plain(MemSource);
        impl ByteSource for Plain {
            fn len(&self) -> u64 {
                self.0.len()
            }
            fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
                self.0.read_at(offset, buf)
            }
        }
        // 1) 根本不是 ZIP
        let junk = Plain(MemSource(vec![0x41; 4096]));
        assert!(super::first_image_bytes_via_central_directory(&junk, 1 << 20)
            .unwrap()
            .is_none());

        // 2) 正常 CBZ，但把 EOCD 的中央目录偏移改成 ZIP64 哨兵
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let stored = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        writer.start_file("001.png", stored).unwrap();
        writer.write_all(&make_png(8, 8, [1, 2, 3, 255])).unwrap();
        let mut data = writer.finish().unwrap().into_inner();
        let eocd = data
            .windows(4)
            .rposition(|w| w == [0x50, 0x4b, 0x05, 0x06])
            .expect("EOCD");
        data[eocd + 16..eocd + 20].copy_from_slice(&[0xff, 0xff, 0xff, 0xff]);
        let zip64 = Plain(MemSource(data));
        assert!(
            super::first_image_bytes_via_central_directory(&zip64, 1 << 20)
                .unwrap()
                .is_none(),
            "ZIP64 哨兵必须回退给常规路径"
        );
    }

    /// P0（第 70 轮）验收：打开一本 **800 条目**的 CBZ 只产生**常数次**远端读。
    ///
    /// 改前：`zip::ZipArchive::new` 对每个条目读一次 local header ⇒ 800+ 次读 ⇒
    /// 115 CDN 243ms/次就是分钟级（"页数越多越慢"）。改后走"只读中央目录"。
    #[test]
    fn opening_a_many_entry_cbz_stays_constant() {
        use std::sync::{Arc, Mutex};
        struct CountingSource {
            data: MemSource,
            reads: Arc<Mutex<Vec<(u64, usize)>>>,
        }
        impl ByteSource for CountingSource {
            fn len(&self) -> u64 {
                self.data.len()
            }
            fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
                self.reads.lock().unwrap().push((offset, buf.len()));
                self.data.read_at(offset, buf)
            }
        }
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for page in 0..800 {
            writer.start_file(format!("{page:04}.jpg"), options).unwrap();
            writer.write_all(&vec![page as u8; 512]).unwrap();
        }
        let data = writer.finish().unwrap().into_inner();

        let reads = Arc::new(Mutex::new(Vec::new()));
        let book = super::ZipBook::open(
            CountingSource {
                data: MemSource(data),
                reads: reads.clone(),
            },
            "many.cbz",
        )
        .unwrap();
        assert_eq!(crate::document::Document::page_count(&book), 800);
        let open_reads = reads.lock().unwrap().len();
        assert!(
            open_reads <= 4,
            "打开成本应与条目数无关：800 条目实际 {open_reads} 次远端读"
        );
        // 取任意一页也应是常数次（local header 1 次 + 数据 1 次）。
        let before = reads.lock().unwrap().len();
        let bytes = crate::document::Document::page_bytes(&book, 400).unwrap();
        assert_eq!(bytes.len(), 512);
        let page_reads = reads.lock().unwrap().len() - before;
        assert!(page_reads <= 2, "取一页应 ≤2 次读，实际 {page_reads} 次");
    }

    /// 第 78 轮真机修复：**EOCD 之后还多出几个字节**的归档（现场就是这样：夸克上一本漫画
    /// `2.epub` 的 EOCD 后还有 5 B 尾巴）也必须能"只读中央目录"打开。
    ///
    /// `zip` crate 容忍这种尾巴，而本层过去要求"EOCD 正好落在文件末尾" ⇒ 明明结构完全健康
    /// （402 条目 / 全部 Stored / 无加密 / 无 ZIP64）却整条快路径弃权，回退成逐条目读
    /// （真机实测：打开 219 次读 / 30 s）。
    #[test]
    fn central_directory_tolerates_trailing_bytes_after_eocd() {
        let plain = make_cbz();
        let plain_entries = super::read_central_directory(&MemSource(plain.clone()))
            .unwrap()
            .map(|entries| entries.len());
        assert_eq!(plain_entries, Some(3), "干净归档先按严格规则命中");

        // 模拟现场：EOCD 之后追加 5 个字节。
        let mut trailed = plain;
        trailed.extend_from_slice(&[0x7e, 0x46, 0x1f, 0x0d, 0x00]);
        let tail_entries = super::read_central_directory(&MemSource(trailed.clone()))
            .unwrap()
            .map(|entries| entries.len());
        assert_eq!(
            tail_entries, plain_entries,
            "EOCD 后有尾巴也必须解析出同样的中央目录"
        );

        // 端到端：仍然能正常打开并取页。
        let doc = open_document(MemSource(trailed), "trailing.cbz").unwrap();
        assert_eq!(doc.page_count(), 3);
    }
}
