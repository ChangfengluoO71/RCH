//! EPUB(漫画)格式解析。
//!
//! 漫画 EPUB 本质是 ZIP + OPF(spine 定义阅读顺序) + 图片文件。
//! 实现:打开 ZIP → 解析 container.xml 找到 OPF 路径 → 解析 OPF 的 manifest + spine →
//! 按 spine 顺序获取每页对应的图片字节。
//! 不依赖排版引擎(漫画 EPUB 不需要 HTML 排版)。
//!
//! # 第 78 轮：打开成本与条目数解耦
//!
//! 过去 `open` 走 `zip::ZipArchive::new`，而后者会对**每个条目**读一次 local header 做校验
//! （`zip-2.4.2/src/read.rs:1259`）⇒ EPUB 条目数远多于 CBZ（每章一个 xhtml + 每个资源一个文件）
//! ⇒ 打开就是几百次远端往返（用户实测"EPUB 特别慢"）。现在三步同时落地：
//!
//! 1. **只读中央目录**（[`CdArchive`]）：打开只读尾部 EOCD + 中央目录，**一个 local header
//!    都不读**；中央目录不可解析（非 ZIP / ZIP64 / 越界 / 不支持的压缩方式）时整条路径回退
//!    crate 实现（**解析结果**与历史一致；回退路径的页读取按需走 `by_index`，并由 crate 做
//!    CRC32 校验 —— 比快路径与历史的手工读更严）。
//! 2. **条目按需读 + 起点惰性化**：打开期零 local header 读；某个条目**首次被访问**时才读它的
//!    local header —— 与数据合并成**一次** `read_at`（`zip::read_entry_bytes_tracked`），
//!    并把算出的数据区起点缓存起来供后续读取复用。
//! 3. **章节按需解析**：`open` 只登记 spine 表，章节 xhtml 首次翻到才读并缓存
//!    ⇒ 打开成本与章节数无关（300 条目 EPUB：改前 ≈O(条目数) 次读，改后 4 次读）。
//!
//! 有意取舍（详见 LOG 第 78 轮）：spine 里的章节各占一页（**能解析到的**章节条目数 = 页数；
//! 条目本身在归档里找不到的章节会被跳过），不再是"打开时把所有章节读一遍、只保留确实含
//! `<img>` 的章节"；章节不含图片（或 `src` 不是图片，例如 EPUB3 nav 文档混进 spine）时，
//! 该页读取会**报错而不是被静默跳过**。Manga EPUB 一章一图，常规 EPUB 不受影响。

use super::{Document, DocumentMeta};
use crate::source::{ByteSource, SourceReader};
use anyhow::{anyhow, Context, Result};
use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek};
use std::sync::{Arc, Mutex};

/// 单个条目允许解压的字节上限（防病态输入；正常 EPUB 的图片/章节远小于此）。
const EPUB_MAX_ENTRY_BYTES: u64 = 512 * 1024 * 1024;

// ============================================================
// 归档访问层：crate `ZipArchive` 与只读中央目录的 `CdArchive` 共用一套解析逻辑
// ============================================================
//
// 解析层（find_opf_path / read_zip_entry / 页表构建）真正只需要四件事：条目数、按名查索引、
// 按索引取名字、按索引读字节。名字一律取自**中央目录**（零 local header 读），字节一律
// **按需**读取；两个后端的"按名找条目"都走 `index_for_name_ignore_case`（先精确、再大小写
// 不敏感），解析逻辑只有一份实现 ⇒ 不存在"改了快路径、忘了回退路径"的调用点
// （第 77 轮两次接线失败的原因）。
//
// - `CdArchive`：只读 EOCD + 中央目录（打开 = 2 次读：EOCD 尾部 + 中央目录），条目字节走
//   `zip::read_entry_bytes_tracked`（local header + 数据合并为一次读）；
// - `zip::ZipArchive`：中央目录不可解析 / 条目不可读时的**回退**；解析结果与历史一致，
//   页读取按需走 `by_index`（每次 1 次 local header + 1 次数据，且由 crate 做 CRC32 校验；
//   快路径与历史的手工读都不校验 CRC）。
//
// 页表与 `page_bytes` 只保存/使用**条目索引**（`&self` 读，不需要 `&mut`），
// 因此并发预取（`Reader` 的多线程预取）天然安全。
trait EpubArchive {
    /// 条目数。
    fn len(&self) -> usize;
    /// 按名查条目索引（先精确命中，再由实现决定退化匹配）。
    fn index_for_name(&self, name: &str) -> Option<usize>;
    /// 中央目录里的条目名（**不读 local header**）。
    fn name_for_index(&self, index: usize) -> Option<String>;
    /// 读出一个条目的解压后字节（按需，不整包读）。
    fn read_index(&self, index: usize, max_bytes: u64) -> Result<Vec<u8>>;
}

// ============================================================
// 只读中央目录的 EPUB 归档适配层（第 77-78 轮）
// ============================================================
//
// 为什么需要：`EpubBook::open` 过去走 `zip::ZipArchive::new`，而后者会对**每个条目**
// 读一次 local header 做校验（`zip-2.4.2/src/read.rs:1259`）⇒ EPUB 条目数远多于 CBZ
// （每章一个 xhtml + 每个资源一个文件）⇒ 打开就是几百次远端往返（用户实测"EPUB 特别慢"）。
//
// 本层复用 ZIP 优化时抽好的构件（`read_central_directory` / `read_entry_bytes`），
// 把"打开"降到 **1 次请求**（尾部 EOCD + 中央目录），条目内容按需读。
//
// 中央目录不可解析（非 ZIP / ZIP64 / 流式条目 / 越界）时 `new` 返回 `Ok(None)`，
// 由调用方回退到现有的 crate 实现，保证不劣化。
pub(crate) struct CdArchive<S: ByteSource> {
    src: Arc<S>,
    entries: Vec<super::zip::ZipEntryMeta>,
    by_name: HashMap<String, usize>,
    /// 惰性 `data_start` 缓存：**首次访问该条目**时才读一次 local header（每个条目最多 1 次）。
    ///
    /// 打开阶段一个 local header 都不读 —— 这正是消除 O(条目数) 的关键。读数据时也会把
    /// 算出的起点写回这里（同一次 `read_at` 里顺带得到），所以正常路径上"读一个条目"只付 1 次读。
    starts: Mutex<HashMap<usize, u64>>,
}

impl<S: ByteSource> CdArchive<S> {
    /// 只读 EOCD + 中央目录；不可用时返回 `Ok(None)`（调用方回退 crate）。
    pub(crate) fn new(src: Arc<S>) -> Result<Option<Self>> {
        let Some(entries) = super::zip::read_central_directory(src.as_ref())? else {
            return Ok(None);
        };
        let mut by_name = HashMap::with_capacity(entries.len());
        for (index, entry) in entries.iter().enumerate() {
            by_name.entry(entry.name.clone()).or_insert(index);
        }
        Ok(Some(CdArchive {
            src,
            entries,
            by_name,
            starts: Mutex::new(HashMap::new()),
        }))
    }

    /// 快路径前置条件：**每个条目**都能被本层读懂，否则整体回退 crate。
    ///
    /// 本层只支持 stored(0) / deflate(8)、不加密、非 ZIP64 哨兵尺寸的条目。只要有一个条目
    /// 不满足就回退：宁可慢（crate 路径 = 历史行为），不可错。
    ///
    /// 空条目（压缩尺寸 0，含目录条目）不在这里排除：它们不需要任何读（见 `read_entry`）。
    fn fast_path_safe(&self) -> bool {
        let len = self.src.len();
        self.entries.iter().all(|entry| {
            matches!(entry.method, 0 | 8)
                && entry.flags & 0x1 == 0
                && entry.compressed_size != u32::MAX as u64
                && entry.local_header < len
        })
    }

    fn entry(&self, index: usize) -> Result<&super::zip::ZipEntryMeta> {
        self.entries
            .get(index)
            .ok_or_else(|| anyhow!("EPUB 条目索引越界: {index}"))
    }

    /// 精确命中（与 crate `ZipArchive::index_for_name` 一致）。
    ///
    /// 刻意**不做"后缀匹配"**：当请求路径只与某个条目的尾部相同时，后缀匹配会静默返回另一个
    /// 条目（错页比报错更糟）。大小写不敏感匹配由 [`index_for_name_ignore_case`] 统一提供
    /// ⇒ 快路径与回退路径的"按名找条目"语义完全一致。
    fn exact_index(&self, name: &str) -> Option<usize> {
        self.by_name.get(name).copied()
    }

    /// 已缓存的数据区起点（由一次**已校验的 local header 读**写入，见 `read_entry`）。
    fn cached_start(&self, index: usize) -> Option<u64> {
        self.starts.lock().unwrap().get(&index).copied()
    }

    /// 读出一个条目的解压后字节（解析层与 `page_bytes` 共用）。
    ///
    /// **冷路径**（首次访问该条目）：这时才读它的 local header —— 与数据合并成一次 `read_at`
    /// （远端少一次往返），并把算出的数据区起点缓存起来；打开阶段因此一个 local header 都不读。
    /// **热路径**（同一条目再次读取）：起点已知 ⇒ 只读数据区（少取 ~1 KB 的 local header 余量；
    /// 请求数不变，仍是 1 次）。
    fn read_entry(&self, index: usize, max_bytes: u64) -> Result<Vec<u8>> {
        let entry = self.entry(index)?;
        // 空条目：中央目录就是权威，连 local header 都不用读。
        if entry.compressed_size == 0 {
            return Ok(Vec::new());
        }
        if let Some(start) = self.cached_start(index) {
            return super::zip::read_entry_at(self.src.as_ref(), entry, start, max_bytes)?
                .ok_or_else(|| anyhow!("读取 EPUB 条目失败: {}", entry.name));
        }
        let (bytes, start) =
            super::zip::read_entry_bytes_tracked(self.src.as_ref(), entry, max_bytes)?
                .ok_or_else(|| anyhow!("读取 EPUB 条目失败: {}", entry.name))?;
        self.starts.lock().unwrap().insert(index, start);
        Ok(bytes)
    }
}

impl<S: ByteSource> EpubArchive for CdArchive<S> {
    fn len(&self) -> usize {
        self.entries.len()
    }

    fn index_for_name(&self, name: &str) -> Option<usize> {
        self.exact_index(name)
    }

    fn name_for_index(&self, index: usize) -> Option<String> {
        self.entries.get(index).map(|entry| entry.name.clone())
    }

    fn read_index(&self, index: usize, max_bytes: u64) -> Result<Vec<u8>> {
        self.read_entry(index, max_bytes)
    }
}

impl<R: Read + Seek + Clone> EpubArchive for zip::ZipArchive<R> {
    fn len(&self) -> usize {
        zip::ZipArchive::len(self)
    }

    fn index_for_name(&self, name: &str) -> Option<usize> {
        zip::ZipArchive::index_for_name(self, name)
    }

    fn name_for_index(&self, index: usize) -> Option<String> {
        zip::ZipArchive::name_for_index(self, index).map(str::to_owned)
    }

    fn read_index(&self, index: usize, _max_bytes: u64) -> Result<Vec<u8>> {
        // `ZipArchive` 的 clone 只复制游标、共享中央目录元数据，因此这里可以按需克隆出一个
        // 独立读取器：并发预取不会互相阻塞（与 `ZipBook` 的回退路径同一手法）。
        //
        // 回退路径**不设单条目上限**：`by_index` + `read_to_end` 与历史行为一致；
        // `EPUB_MAX_ENTRY_BYTES` 只约束本轮新增的快路径。副作用：这条路径由 crate 做 CRC32
        // 校验（历史的手工读与快路径都不校验），方向是更严。
        let mut archive = self.clone();
        let mut file = archive
            .by_index(index)
            .with_context(|| format!("ZIP 条目打开失败: {index}"))?;
        let mut buf = Vec::with_capacity(file.size() as usize);
        file.read_to_end(&mut buf)?;
        Ok(buf)
    }
}

/// 一页的来源。
enum PageSlot {
    /// 已定位的图片条目（spine 直接给图片，或退化扫描得到的图片）。
    Image(usize),
    /// 章节页（xhtml/html）：**首次翻到才解析**。
    Chapter(ChapterPage),
}

/// 章节页：`open` 只登记「章节条目 + 目录」，章节内容在首次读到该页时才读。
struct ChapterPage {
    /// 章节文件（xhtml/html）的条目索引。
    chapter: usize,
    /// 章节文件的逻辑路径（错误信息用）。
    path: String,
    /// 章节文件所在目录：`<img src>` 相对它解析（不是 OPF 目录）。
    dir: String,
    /// 已解析出的图片条目；只缓存成功结果（读取失败不落永久状态，下次仍可重试）。
    ///
    /// 锁只在读写缓存那一瞬间持有，**不跨越任何 IO**：并发预取同一章节最多重复读一次
    /// （幂等），不会阻塞也不会返回错页。
    image: Mutex<Option<usize>>,
}

impl ChapterPage {
    /// 解析该章节 `<img src>` 指向的图片条目（**首次访问才读章节文件**，成功结果缓存）。
    fn image_entry<S: ByteSource>(&self, book: &EpubBook<S>) -> Result<usize> {
        if let Some(index) = *self.image.lock().unwrap() {
            return Ok(index);
        }
        let html = book.read_entry(self.chapter)?;
        let src = extract_img_src(&html)
            .ok_or_else(|| anyhow!("EPUB 章节里没有 <img>: {}", self.path))?;
        let img_path = resolve_path(&self.dir, &src);
        if !is_image_ext(&img_path) {
            return Err(anyhow!("EPUB 章节引用的不是图片: {img_path}"));
        }
        let index = book
            .entry_index(&img_path)
            .ok_or_else(|| anyhow!("EPUB 中找不到图片: {img_path}"))?;
        *self.image.lock().unwrap() = Some(index);
        Ok(index)
    }
}

/// EPUB 书籍。
pub struct EpubBook<S: ByteSource> {
    /// 快路径：只读中央目录的归档（页表索引指向它）。
    central: Option<CdArchive<S>>,
    /// 回退路径：crate 归档（页表索引指向它；与历史行为一致）。
    archive: Option<zip::ZipArchive<SourceReader<Arc<S>>>>,
    /// 按阅读顺序排列的页（章节页惰性解析）。
    pages: Vec<PageSlot>,
    title: String,
}

impl<S: ByteSource> EpubBook<S> {
    /// 打开 EPUB：优先"只读中央目录"的快路径，不可用时回退 crate `ZipArchive`。
    pub fn open(src: S, path: &str) -> Result<Self> {
        Self::open_inner(src, path, true)
    }

    /// **强制走历史 crate 路径**打开（逐条目读 local header）。
    ///
    /// 只为 A/B 量化而存在（`examples/read_profile.rs --legacy`）：同一份归档下
    /// "改前 vs 改后"的读次数可以直接对比，不必切换二进制。
    pub fn open_legacy(src: S, path: &str) -> Result<Self> {
        Self::open_inner(src, path, false)
    }

    fn open_inner(src: S, path: &str, prefer_central: bool) -> Result<Self> {
        let title = std::path::Path::new(path)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string());
        let shared = Arc::new(src);

        // ① 快路径：只读 EOCD + 中央目录，**不构造 crate 归档**。
        //    前置条件不成立、中央目录读不出来、或解析失败，都回退 ②（快路径只做加速，
        //    不引入新的失败点；`CdArchive::new` 的读取错误也在此吞掉并交给 ② 决定成败）。
        if prefer_central {
            if let Ok(Some(central)) = CdArchive::new(Arc::clone(&shared)) {
                if central.fast_path_safe() {
                    if let Ok(pages) = build_pages(&central) {
                        return Ok(EpubBook {
                            central: Some(central),
                            archive: None,
                            pages,
                            title,
                        });
                    }
                }
            }
        }

        // ② 回退：crate `ZipArchive`（逐条目读 local header —— 历史行为，非 ZIP/ZIP64 也能开）。
        let reader = SourceReader::new(Arc::clone(&shared));
        let zip = zip::ZipArchive::new(reader).context("打开 EPUB(ZIP)失败")?;
        let pages = build_pages(&zip)?;
        Ok(EpubBook {
            central: None,
            archive: Some(zip),
            pages,
            title,
        })
    }

    /// 按条目索引读出解压后的字节（`page_bytes` 与章节惰性解析共用）。
    fn read_entry(&self, index: usize) -> Result<Vec<u8>> {
        if let Some(central) = &self.central {
            return central.read_entry(index, EPUB_MAX_ENTRY_BYTES);
        }
        if let Some(archive) = &self.archive {
            return archive.read_index(index, EPUB_MAX_ENTRY_BYTES);
        }
        Err(anyhow!("EPUB 归档缺失"))
    }

    /// 按名字找条目索引（章节惰性解析用）。
    ///
    /// 两个后端走**同一个**规则（先精确、再大小写不敏感），因此"章节里 `<img src>` 的大小写
    /// 与归档条目不一致"这类历史可用的书，在快路径上同样可用。
    fn entry_index(&self, name: &str) -> Option<usize> {
        if let Some(central) = &self.central {
            return index_for_name_ignore_case(central, name);
        }
        if let Some(archive) = &self.archive {
            return index_for_name_ignore_case(archive, name);
        }
        None
    }
}

impl<S: ByteSource> Document for EpubBook<S> {
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
        let slot = self
            .pages
            .get(index as usize)
            .with_context(|| format!("页索引越界: {index}"))?;
        let entry = match slot {
            PageSlot::Image(entry) => *entry,
            PageSlot::Chapter(chapter) => chapter
                .image_entry(self)
                .with_context(|| format!("解析 EPUB 章节失败: {}", chapter.path))?,
        };
        self.read_entry(entry)
    }
}

// ---------- helpers ----------

/// 建立页表（**不读章节内容**）：spine 里的章节只登记条目，图片直接登记。
fn build_pages<A: EpubArchive>(archive: &A) -> Result<Vec<PageSlot>> {
    // 1. 解析 container.xml
    let opf_path = find_opf_path(archive)?;

    // 2. 解析 OPF
    let opf_xml = read_zip_entry(archive, &opf_path)
        .with_context(|| format!("读取 OPF 失败: {opf_path}"))?;
    let (manifest, spine) = parse_opf(&opf_xml)?;

    // 3. 按 spine 顺序登记页
    let opf_dir = opf_path
        .rsplit_once('/')
        .map(|(d, _)| format!("{}/", d))
        .unwrap_or_default();

    let mut pages = Vec::new();
    let mut seen = HashSet::new();
    for idref in &spine {
        if let Some(href) = manifest.get(idref) {
            let full = resolve_path(&opf_dir, href);
            let lower = full.to_lowercase();
            if lower.ends_with(".xhtml") || lower.ends_with(".html") || lower.ends_with(".htm") {
                // 章节：**不读内容**，只登记（打开成本因此与章节数无关）。
                let Some(chapter) = index_for_name_ignore_case(archive, &full) else {
                    continue;
                };
                let dir = full
                    .rsplit_once('/')
                    .map(|(d, _)| format!("{}/", d))
                    .unwrap_or_default();
                pages.push(PageSlot::Chapter(ChapterPage {
                    chapter,
                    path: full,
                    dir,
                    image: Mutex::new(None),
                }));
            } else if is_image_ext(&full) && seen.insert(full.clone()) {
                let entry = index_for_name_ignore_case(archive, &full)
                    .with_context(|| format!("EPUB 中找不到图片: {full}"))?;
                pages.push(PageSlot::Image(entry));
            }
        }
    }

    // 4. 退化:spine 没给出图片时，扫描归档里所有图片（名字取自中央目录，0 次 local header 读）
    if pages.is_empty() {
        let mut all_images = Vec::new();
        for index in 0..archive.len() {
            let Some(name) = archive.name_for_index(index) else {
                continue;
            };
            if is_image_ext(&name)
                && !name.contains("__MACOSX")
                && !name.ends_with(".DS_Store")
            {
                all_images.push((name, index));
            }
        }
        all_images.sort_by(|(a, _), (b, _)| crate::util::natural_cmp(a, b));
        pages = all_images
            .into_iter()
            .map(|(_, index)| PageSlot::Image(index))
            .collect();
    }

    if pages.is_empty() {
        return Err(anyhow!("EPUB 中没有找到图片"));
    }
    Ok(pages)
}

fn is_image_ext(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.ends_with(".jpg")
        || lower.ends_with(".jpeg")
        || lower.ends_with(".png")
        || lower.ends_with(".webp")
        || lower.ends_with(".gif")
        || lower.ends_with(".bmp")
        || lower.ends_with(".avif")
}

fn index_for_name_ignore_case<A: EpubArchive>(archive: &A, name: &str) -> Option<usize> {
    if let Some(idx) = archive.index_for_name(name) {
        return Some(idx);
    }
    for i in 0..archive.len() {
        if let Some(found) = archive.name_for_index(i) {
            if found.eq_ignore_ascii_case(name) {
                return Some(i);
            }
        }
    }
    None
}

fn find_opf_path<A: EpubArchive>(archive: &A) -> Result<String> {
    let xml = read_zip_entry(archive, "META-INF/container.xml")
        .context("EPUB 缺少 META-INF/container.xml")?;
    let s = String::from_utf8_lossy(&xml);
    let mut pos = 0;
    while pos < s.len() {
        if let Some(start) = s[pos..].find("full-path") {
            let abs = pos + start;
            if let Some(q_start) = s[abs..].find('"') {
                let after_quote = abs + q_start + 1;
                if let Some(q_end) = s[after_quote..].find('"') {
                    return Ok(s[after_quote..after_quote + q_end].to_string());
                }
            }
            pos = abs + 1;
        } else {
            break;
        }
    }
    Err(anyhow!("container.xml 中未找到 rootfile full-path"))
}

fn parse_opf(xml: &[u8]) -> Result<(HashMap<String, String>, Vec<String>)> {
    let s = String::from_utf8_lossy(xml);
    let mut manifest = HashMap::new();
    let mut spine = Vec::new();
    let mut in_manifest = false;
    let mut in_spine = false;

    let mut pos = 0;
    let bytes = s.as_bytes();
    while pos < bytes.len() {
        if bytes[pos] != b'<' {
            pos += 1;
            continue;
        }
        let tag_end = match bytes[pos..].iter().position(|&b| b == b'>') {
            Some(i) => pos + i + 1,
            None => break,
        };
        let tag = &s[pos..tag_end];

        if tag.starts_with("<manifest") {
            in_manifest = true;
        } else if tag.starts_with("</manifest") {
            in_manifest = false;
        } else if tag.starts_with("<spine") {
            in_spine = true;
        } else if tag.starts_with("</spine") {
            in_spine = false;
        } else if in_manifest && tag.starts_with("<item") {
            let id = extract_attr(tag, "id");
            let href = extract_attr(tag, "href");
            if let (Some(i), Some(h)) = (id, href) {
                manifest.insert(i, h);
            }
        } else if in_spine && tag.starts_with("<itemref") {
            if let Some(idref) = extract_attr(tag, "idref") {
                spine.push(idref);
            }
        }
        pos = tag_end;
    }

    if manifest.is_empty() || spine.is_empty() {
        let mut imgs: Vec<_> = manifest
            .values()
            .filter(|h| is_image_ext(h))
            .cloned()
            .collect();
        imgs.sort_by(|a, b| crate::util::natural_cmp(a, b));
        return Ok((manifest, imgs));
    }

    Ok((manifest, spine))
}

fn extract_attr(tag: &str, attr: &str) -> Option<String> {
    let pat = format!("{}=\"", attr);
    let start = tag.find(&pat)? + pat.len();
    let rest = &tag[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

fn extract_img_src(html: &[u8]) -> Option<String> {
    let s = String::from_utf8_lossy(html);
    let lower = s.to_lowercase();
    let img_start = lower.find("<img")?;
    let tag_slice = &s[img_start..];
    let close = tag_slice.find('>')?;
    let img_tag = &tag_slice[..=close];
    extract_attr(img_tag, "src")
}

fn resolve_path(base: &str, path: &str) -> String {
    if path.starts_with('/') || path.starts_with("http") {
        return path.to_string();
    }
    let mut parts: Vec<&str> = base.split('/').filter(|s| !s.is_empty()).collect();
    for seg in path.split('/') {
        match seg {
            "." => {}
            ".." => {
                parts.pop();
            }
            _ => parts.push(seg),
        }
    }
    parts.join("/")
}

fn read_zip_entry<A: EpubArchive>(archive: &A, path: &str) -> Result<Vec<u8>> {
    let idx = index_for_name_ignore_case(archive, path)
        .with_context(|| format!("ZIP 中找不到: {path}"))?;
    archive.read_index(idx, EPUB_MAX_ENTRY_BYTES)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 计数字节源：把每次 `read_at` 记下来（1 次读 = 远端 1 次 Range 往返）。
    struct CountingSource {
        data: Vec<u8>,
        reads: Arc<Mutex<Vec<(u64, usize)>>>,
    }

    impl ByteSource for CountingSource {
        fn len(&self) -> u64 {
            self.data.len() as u64
        }

        fn read_at(&self, offset: u64, buf: &mut [u8]) -> std::io::Result<usize> {
            self.reads.lock().unwrap().push((offset, buf.len()));
            let start = offset as usize;
            if start >= self.data.len() {
                return Ok(0);
            }
            let n = (self.data.len() - start).min(buf.len());
            buf[..n].copy_from_slice(&self.data[start..start + n]);
            Ok(n)
        }
    }

    /// 构造计数字节源，返回它和读数表。
    fn counting(data: Vec<u8>) -> (CountingSource, Arc<Mutex<Vec<(u64, usize)>>>) {
        let reads = Arc::new(Mutex::new(Vec::new()));
        (
            CountingSource {
                data,
                reads: Arc::clone(&reads),
            },
            reads,
        )
    }

    #[test]
    fn test_resolve_path() {
        assert_eq!(
            resolve_path("OEBPS/", "images/001.jpg"),
            "OEBPS/images/001.jpg"
        );
        assert_eq!(
            resolve_path("OEBPS/", "../META-INF/container.xml"),
            "META-INF/container.xml"
        );
        assert_eq!(
            resolve_path("OEBPS/", "/absolute/path.jpg"),
            "/absolute/path.jpg"
        );
    }

    #[test]
    fn test_extract_attr() {
        let tag = r#"<item id="cover" href="images/cover.jpg" media-type="image/jpeg"/>"#;
        assert_eq!(extract_attr(tag, "id"), Some("cover".to_string()));
        assert_eq!(
            extract_attr(tag, "href"),
            Some("images/cover.jpg".to_string())
        );
    }

    #[test]
    fn test_extract_img_src() {
        let html = br#"<html><body><img src="page001.jpg" alt="page"/></body></html>"#;
        assert_eq!(extract_img_src(html), Some("page001.jpg".to_string()));
    }

    /// 回归：OPF 在根目录、HTML 在 content/、图片在 content/resources/ 的漫画 EPUB。
    /// img src 应相对 HTML 目录解析（曾按 OPF 目录解析成 resources/P00001.jpg 而找不到）。
    #[test]
    fn open_epub_with_html_subdir_images() {
        use crate::source::local::LocalFile;
        use std::io::Write;

        let dir = std::env::temp_dir().join(format!(
            "rch_epub_test_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("sample.epub");
        {
            let file = std::fs::File::create(&path).unwrap();
            let mut w = zip::ZipWriter::new(file);
            let opt = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            w.start_file("mimetype", opt).unwrap();
            w.write_all(b"application/epub+zip").unwrap();
            w.start_file("META-INF/container.xml", opt).unwrap();
            w.write_all(
                br#"<?xml version="1.0"?><container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="metadata.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#,
            )
            .unwrap();
            w.start_file("metadata.opf", opt).unwrap();
            w.write_all(
                br#"<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf" version="2.0"><metadata><dc:title xmlns:dc="http://purl.org/dc/elements/1.1/">t</dc:title></metadata><manifest><item id="id1" href="content/resources/P00001.jpg" media-type="image/jpeg"/><item id="id2" href="content/index_P00001.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="id2"/></spine></package>"#,
            )
            .unwrap();
            w.start_file("content/index_P00001.xhtml", opt).unwrap();
            w.write_all(br#"<html><body><img src="resources/P00001.jpg"/></body></html>"#)
                .unwrap();
            w.start_file("content/resources/P00001.jpg", opt).unwrap();
            w.write_all(&[0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10]).unwrap();
            w.finish().unwrap();
        }

        let src = LocalFile::open(&path).unwrap();
        let book = EpubBook::open(src, "sample.epub").unwrap();
        assert_eq!(book.page_count(), 1);
        let bytes = book.page_bytes(0).unwrap();
        assert_eq!(bytes, [0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 第 78 轮验收：打开一本 **300 条目**的漫画 EPUB（container + OPF + 148 章节 +
    /// 148 图片 + 1 css）只产生**常数次**远端读。
    ///
    /// 改前：`zip::ZipArchive::new` 对每个条目读一次 local header（≈O(条目数) 次读），
    /// 且 spine 里每个章节 xhtml 都要在 open 阶段读一遍 ⇒ 115 CDN 243 ms/次就是分钟级。
    /// 改后：只读 EOCD + 中央目录 + container.xml + OPF；章节内容首次翻到才读。
    #[test]
    fn opening_a_many_entry_epub_stays_constant() {
        const CHAPTERS: usize = 148; // 2 * 148 + 4 = 300 条目
        let data = build_many_entry_epub(CHAPTERS);

        // "改前"：同一份归档走历史 crate 路径（逐条目读 local header）。
        let (legacy_source, legacy_reads) = counting(data.clone());
        let legacy = zip::ZipArchive::new(SourceReader::new(legacy_source));
        assert!(legacy.is_ok(), "历史 crate 路径也应当能打开该归档");
        let legacy_open_reads = legacy_reads.lock().unwrap().len();

        // "改后"：只读中央目录 + 惰性章节。
        let (source, reads) = counting(data);
        let book = EpubBook::open(source, "many.epub").unwrap();
        let open_reads = reads.lock().unwrap().len();
        println!(
            "EPUB-METRIC entries=300 legacy_open_reads={legacy_open_reads} open_reads={open_reads}"
        );

        assert_eq!(book.page_count(), CHAPTERS as u32);
        assert!(
            open_reads <= 4,
            "打开成本应与条目数无关：300 条目实际 {open_reads} 次远端读"
        );
        assert!(
            legacy_open_reads > open_reads,
            "改后读次数必须低于改前：legacy={legacy_open_reads} now={open_reads}"
        );

        // 首次翻到第 0 页：读章节 xhtml + 读图片（各 1 次读）。
        let before = reads.lock().unwrap().len();
        assert_eq!(book.page_bytes(0).unwrap(), page_image(0));
        let first_reads = reads.lock().unwrap().len() - before;
        assert!(
            first_reads <= 2,
            "首次翻页应 ≤2 次读（章节 + 图片），实际 {first_reads} 次"
        );

        // 再读同一页：章节解析结果已缓存 ⇒ 只剩图片 1 次读，且**只读数据区**
        //（起点已知，不再带 `30 + 名字 + 1024` 的 local header 余量）。
        let before = reads.lock().unwrap().len();
        assert_eq!(book.page_bytes(0).unwrap(), page_image(0));
        let snapshot = reads.lock().unwrap().clone();
        let repeat_window = &snapshot[before..];
        assert!(
            repeat_window.len() <= 1,
            "重复翻同一页应 ≤1 次读（章节已缓存），实际 {} 次",
            repeat_window.len()
        );
        assert!(
            repeat_window
                .iter()
                .all(|(_, size)| *size <= page_image(0).len() + 256),
            "热路径不应再取 local header 余量（请求长度应贴近压缩尺寸）：{repeat_window:?}"
        );

        // 末页同样只碰它自己的条目。
        let before = reads.lock().unwrap().len();
        assert_eq!(
            book.page_bytes(CHAPTERS as u32 - 1).unwrap(),
            page_image(CHAPTERS - 1)
        );
        let last_reads = reads.lock().unwrap().len() - before;
        assert!(
            last_reads <= 2,
            "末页首次读取应 ≤2 次读，实际 {last_reads} 次"
        );
    }

    /// 第 78 轮：打开快路径**只**读文件尾部（EOCD + 中央目录），一个 local header 都不读 ——
    /// 这是"打开成本与条目数无关"的根基（若打开期逐条读 local header，读数会 ≈ 条目数）。
    #[test]
    fn opening_cd_archive_reads_only_the_tail() {
        /// ZIP 注释上限 + EOCD 长度：尾部一次读的窗口。
        const TAIL_WINDOW: u64 = 65_535 + 22;

        // 必须用**够大**的夹具：小文件整个落在尾部窗口里，offset 断言就失去分辨力。
        let data = build_many_entry_epub(148); // 300 条目 ≈ 650 KB
        let len = data.len() as u64;
        assert!(len > TAIL_WINDOW, "夹具要大于尾部窗口才有分辨力");
        let (source, reads) = counting(data);
        let archive = CdArchive::new(Arc::new(source))
            .unwrap()
            .expect("应当能只读中央目录打开");
        assert_eq!(archive.len(), 300);

        let snapshot = reads.lock().unwrap().clone();
        assert!(
            snapshot.len() <= 2,
            "打开只应读尾部 EOCD + 中央目录，实际 {} 次：{snapshot:?}",
            snapshot.len()
        );
        for (offset, size) in &snapshot {
            assert!(
                offset.saturating_add(TAIL_WINDOW) >= len,
                "打开阶段的每次读都必须落在文件尾部（EOCD/中央目录区）：offset={offset} len={len}"
            );
            assert!(
                *size as u64 <= TAIL_WINDOW,
                "打开阶段的单次读不应超过尾部窗口：{size} B"
            );
        }
    }

    /// 回归：章节里的 `<img src>` 与归档条目**大小写不一致**时，快路径也必须能找到图片
    /// （历史行为是大小写不敏感；快路径若只做精确匹配，这类书会从"能看"变成"找不到图片"）。
    #[test]
    fn open_epub_with_mismatched_case_in_img_src() {
        use std::io::Write;

        let mut cursor = std::io::Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut cursor);
            let opt = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            w.start_file("mimetype", opt).unwrap();
            w.write_all(b"application/epub+zip").unwrap();
            w.start_file("META-INF/container.xml", opt).unwrap();
            w.write_all(
                br#"<?xml version="1.0"?><container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="metadata.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#,
            )
            .unwrap();
            w.start_file("metadata.opf", opt).unwrap();
            w.write_all(
                br#"<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf" version="2.0"><metadata><dc:title xmlns:dc="http://purl.org/dc/elements/1.1/">case</dc:title></metadata><manifest><item id="i1" href="resources/p00001.jpg" media-type="image/jpeg"/><item id="c1" href="content/p00001.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="c1"/></spine></package>"#,
            )
            .unwrap();
            // 条目名全小写，章节里引用全大写。
            w.start_file("content/p00001.xhtml", opt).unwrap();
            w.write_all(br#"<html><body><img src="../RESOURCES/P00001.JPG"/></body></html>"#)
                .unwrap();
            w.start_file("resources/p00001.jpg", opt).unwrap();
            w.write_all(&page_image(7)).unwrap();
            w.finish().unwrap();
        }

        // 快路径与回退路径都必须能解析同样的大小写差异。
        let data = cursor.into_inner();
        let (source, _reads) = counting(data.clone());
        let book = EpubBook::open(source, "case.epub").unwrap();
        assert_eq!(book.page_count(), 1);
        assert_eq!(book.page_bytes(0).unwrap(), page_image(7));

        let (legacy_source, _legacy_reads) = counting(data);
        let legacy = EpubBook::open_legacy(legacy_source, "case.epub").unwrap();
        assert_eq!(legacy.page_count(), 1);
        assert_eq!(legacy.page_bytes(0).unwrap(), page_image(7));
    }

    /// 第 78 轮：快路径**不可用**（中央目录里有本层读不懂的条目）时必须整体回退 crate，
    /// 且回退路径照样能正常翻页 —— 这是"快路径只做加速、不引入新失败点"的证据。
    #[test]
    fn opening_falls_back_when_an_entry_is_unreadable() {
        // 把非页条目（OEBPS/style.css）的中央目录压缩方式改成 bzip2(12)：本层只支持
        // stored/deflate ⇒ `fast_path_safe()` 为假 ⇒ 整条快路径作废，走 crate
        //（crate 不需要读这个条目）。
        let mut data = build_many_entry_epub(4);
        let name = b"OEBPS/style.css";
        let name_at = data
            .windows(name.len())
            .rposition(|w| w == name)
            .expect("中央目录里应当有该条目名");
        let cdfh_at = name_at - 46; // 中央目录条目头固定 46 B，条目名紧跟其后
        assert_eq!(
            &data[cdfh_at..cdfh_at + 4],
            b"PK\x01\x02",
            "定位到的应当是中央目录条目头"
        );
        data[cdfh_at + 10..cdfh_at + 12].copy_from_slice(&12_u16.to_le_bytes());

        let (source, reads) = counting(data);
        let book = EpubBook::open(source, "fallback.epub").unwrap();
        // 回退路径逐条目读 local header ⇒ **打开阶段**的读数就远大于快路径的常数次（4），
        // 这个断言必须在读页之前采样，否则快路径也能凑出 >4。
        let open_reads = reads.lock().unwrap().len();
        assert!(
            open_reads > 6,
            "应当在打开阶段就走 crate 回退路径（逐条目读 local header），实际 {open_reads} 次读"
        );
        assert_eq!(book.page_count(), 4);
        assert_eq!(book.page_bytes(0).unwrap(), page_image(0));
        assert_eq!(book.page_bytes(3).unwrap(), page_image(3));
    }

    /// 构造一本"章节 xhtml + 图片"的漫画 EPUB（真实 EPUB 的形状），返回 ZIP 字节。
    ///
    /// 条目数 = 2 * `chapters` + 4（mimetype / container.xml / content.opf / style.css）。
    fn build_many_entry_epub(chapters: usize) -> Vec<u8> {
        use std::io::Write;

        let mut cursor = std::io::Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut cursor);
            let stored = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            let deflated = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);

            w.start_file("mimetype", stored).unwrap();
            w.write_all(b"application/epub+zip").unwrap();
            w.start_file("META-INF/container.xml", deflated).unwrap();
            w.write_all(
                br#"<?xml version="1.0"?><container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#,
            )
            .unwrap();

            let mut manifest = String::new();
            let mut spine = String::new();
            for index in 0..chapters {
                manifest.push_str(&format!(
                    r#"<item id="c{index}" href="text/p{index:03}.xhtml" media-type="application/xhtml+xml"/><item id="i{index}" href="images/p{index:03}.jpg" media-type="image/jpeg"/>"#
                ));
                spine.push_str(&format!(r#"<itemref idref="c{index}"/>"#));
            }
            w.start_file("OEBPS/content.opf", deflated).unwrap();
            w.write_all(
                format!(
                    r#"<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf" version="2.0"><metadata><dc:title xmlns:dc="http://purl.org/dc/elements/1.1/">many</dc:title></metadata><manifest>{manifest}</manifest><spine>{spine}</spine></package>"#
                )
                .as_bytes(),
            )
            .unwrap();

            for index in 0..chapters {
                w.start_file(format!("OEBPS/text/p{index:03}.xhtml"), deflated)
                    .unwrap();
                w.write_all(
                    format!(r#"<html><body><img src="../images/p{index:03}.jpg"/></body></html>"#)
                        .as_bytes(),
                )
                .unwrap();
            }
            for index in 0..chapters {
                w.start_file(format!("OEBPS/images/p{index:03}.jpg"), deflated)
                    .unwrap();
                w.write_all(&page_image(index)).unwrap();
            }
            // 一个非图片资源，凑成真实 EPUB 的条目构成。
            w.start_file("OEBPS/style.css", deflated).unwrap();
            w.write_all(b"body{margin:0}").unwrap();
            w.finish().unwrap();
        }
        cursor.into_inner()
    }

    /// 第 `index` 页的图片字节（内容可判别，便于断言"读到的确实是这一页"）。
    fn page_image(index: usize) -> Vec<u8> {
        let mut bytes = vec![0xff, 0xd8, 0xff, 0xe0]; // JPEG 魔数
        bytes.extend_from_slice(&(index as u32).to_le_bytes());
        let mut state = 0x2545_f491_4f6c_dd1du64 ^ (index as u64).wrapping_mul(0x9e37_79b9);
        while bytes.len() < 4096 {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            bytes.push((state >> 33) as u8);
        }
        bytes
    }
}
