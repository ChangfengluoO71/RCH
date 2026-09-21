//! G1（2026-09-21）：**反复回到文件头的读，绝不能每次重新发起远端请求**。
//!
//! 真机签名（`D:/Temp/rch-perf-*.jsonl` + `scan_diag.log`）：
//! - 一次阅读会话里同一 `offset=0` 被重取 **108–135 次**，每次都是一份 RTT（实测 136–182 ms）；
//! - 一次 EPUB 会话 1265 次远端读里 **906 次请求 <512 B，却吃掉 62% 的读时长**。
//!
//! 契约：**自然取到的**、覆盖 `offset 0` 的那个窗口就地钉住、永不淘汰 ⇒
//! 之后的头部读必须**零新增请求**（且不多读一个字节 —— 钉住只是不丢已有数据）。

use rust_lib_app::source::{ByteSource, SourceReader};
use std::io::{Read, Seek, SeekFrom};
use std::sync::{Arc, Mutex};

/// 计数源：记录每次底层读的 `(offset, 请求长度)`。
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
        let offset = offset as usize;
        if offset >= self.data.len() {
            return Ok(0);
        }
        let n = buf.len().min(self.data.len() - offset);
        buf[..n].copy_from_slice(&self.data[offset..offset + n]);
        Ok(n)
    }
}

fn fixture() -> Vec<u8> {
    // 12 MB：足够大，能真实体现"大窗口来回换"的场景。
    (0..12 * 1024 * 1024).map(|i| (i % 251) as u8).collect()
}

#[test]
fn repeated_head_reads_do_not_refetch_after_big_window_moves() {
    let data = fixture();
    let reads = Arc::new(Mutex::new(Vec::new()));
    let mut reader = SourceReader::new(CountingSource {
        data: data.clone(),
        reads: Arc::clone(&reads),
    });

    // 第一次头部读：自然取到覆盖 offset 0 的窗口 ⇒ 钉住。
    let mut head = [0u8; 30];
    reader.read_exact(&mut head).expect("首次头部读");
    assert_eq!(&head[..], &data[..30], "头部内容必须正确");
    assert!(
        !reads.lock().unwrap().is_empty(),
        "首次头部读必须真的读过一次远端"
    );

    // 模拟翻页：连续 4 次跳到很远的偏移读一大块（每次都会换掉大窗口）。
    for page in 0..4_u64 {
        let start = 6 * 1024 * 1024 + page * 1024 * 1024;
        reader.seek(SeekFrom::Start(start)).expect("seek");
        let mut body = vec![0u8; 4096];
        reader.read_exact(&mut body).expect("正文读");
        assert_eq!(
            &body[..],
            &data[start as usize..start as usize + 4096],
            "正文内容必须正确"
        );
    }

    // 回到文件头：必须零新增请求。
    let before = reads.lock().unwrap().len();
    reader.seek(SeekFrom::Start(0)).expect("seek back");
    reader.read_exact(&mut head).expect("回到头部读");
    assert_eq!(&head[..], &data[..30], "回到头部的内容必须正确");
    let after = reads.lock().unwrap().len();
    assert_eq!(
        after, before,
        "回到文件头不得重新发起远端读（钉住的头窗口应命中）：before={before} after={after}"
    );

    // 头部**之外**的邻近读仍然走正常窗口：不得因为钉住而返回错数据。
    reader.seek(SeekFrom::Start(1024)).expect("seek");
    let mut probe = [0u8; 64];
    reader.read_exact(&mut probe).expect("头窗口内偏移读");
    assert_eq!(&probe[..], &data[1024..1088]);
}

#[test]
fn head_pin_does_not_change_content_or_over_fetch_for_small_sources() {
    // 小文件（<256 KiB）：钉住逻辑同样只是"留下已有的窗口"，绝不额外请求。
    let data = fixture()[..64 * 1024].to_vec();
    let reads = Arc::new(Mutex::new(Vec::new()));
    let mut reader = SourceReader::new(CountingSource {
        data: data.clone(),
        reads: Arc::clone(&reads),
    });

    let mut first = vec![0u8; 4096];
    reader.read_exact(&mut first).expect("首次读");
    assert_eq!(&first[..], &data[..4096]);
    let after_first = reads.lock().unwrap().len();

    // 同一个窗口内的第二次读：命中，不新增请求。
    let mut second = vec![0u8; 4096];
    reader.read_exact(&mut second).expect("窗口内续读");
    assert_eq!(&second[..], &data[4096..8192]);
    assert_eq!(
        reads.lock().unwrap().len(),
        after_first,
        "同一个已取窗口内的续读不得再发请求"
    );

    // 再次回到 0 读：命中钉住的窗口，不新增请求。
    reader.seek(SeekFrom::Start(0)).expect("seek");
    let mut again = vec![0u8; 4096];
    reader.read_exact(&mut again).expect("回头读");
    assert_eq!(&again[..], &data[..4096]);
    assert_eq!(
        reads.lock().unwrap().len(),
        after_first,
        "回到文件头不得重新发起远端读"
    );
}
