//! M2 AI 超分引擎 — CLI 目录批量模式。
//!
//! `realesrgan-ncnn-vulkan.exe` 不支持 stdin/stdout，采用文件/目录参数：
//! 单页 = 单次 CLI 调用；整本 = 一次 CLI 调用处理整个输入目录（Phase 2）。
//! 所有子进程调用带 60s 超时、Windows 无黑窗、唯一临时文件防并发踩踏。

use crate::cache::CacheDir;
use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

const MODEL_NAME: &str = "realesr-animevideov3";
const PROCESS_TIMEOUT: Duration = Duration::from_secs(60);
const JPEG_QUALITY: u8 = 90;

static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

fn next_seq() -> u64 {
    TEMP_SEQ.fetch_add(1, Ordering::Relaxed)
}

/// 定位 exe
fn exe_path() -> Result<PathBuf> {
    let exe = std::env::current_exe().context("无法获取可执行文件路径")?;
    let exe_dir = exe.parent().context("无法获取可执行文件目录")?;
    for p in &[
        exe_dir.join("data").join("ai").join("realesrgan-ncnn-vulkan.exe"),
        exe_dir.join("ai").join("realesrgan-ncnn-vulkan.exe"),
    ] {
        if p.exists() {
            return Ok(p.clone());
        }
    }
    bail!("未找到 AI 引擎可执行文件")
}

fn models_dir() -> Result<PathBuf> {
    let exe = exe_path()?;
    Ok(exe.parent().unwrap().join("models"))
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    format!("{:x}", h.finalize())
}

/// 缓存 key：sha256(原始字节)_模型名_倍率.ai（模型名参与 key，换模型不串缓存）。
fn cache_key(hash: &str, scale: u32) -> String {
    format!("{hash}_{MODEL_NAME}_{scale}x.ai")
}

/// 读取缓存；损坏/半成品缓存视为未命中并删除。
fn read_cached(hash: &str, scale: u32) -> Result<Option<Vec<u8>>> {
    let p = CacheDir::Ai.ensure()?.join(cache_key(hash, scale));
    match std::fs::read(&p) {
        Ok(data) => Ok(Some(data)),
        Err(_) => {
            let _ = std::fs::remove_file(&p);
            Ok(None)
        }
    }
}

/// 原子写缓存：先写临时文件再 rename，避免中断留下损坏缓存。
fn write_cache_atomic(dir: &PathBuf, key: &str, data: &[u8]) -> Result<()> {
    let target = dir.join(key);
    let tmp = dir.join(format!("{key}.tmp"));
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, &target)?;
    Ok(())
}

/// 带超时（60s + kill）与 Windows 无黑窗的 CLI 调用。
/// 返回 (是否成功, stderr 文本)。
fn run_cli(mut cmd: std::process::Command) -> Result<(bool, String)> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let mut child = cmd
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("启动 AI 引擎失败")?;

    // 后台线程排空管道，防止子进程因管道缓冲满而阻塞。
    let out_reader = child.stdout.take().map(|mut s| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = s.read_to_end(&mut buf);
            buf
        })
    });
    let err_reader = child.stderr.take().map(|mut s| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = s.read_to_end(&mut buf);
            String::from_utf8_lossy(&buf).into_owned()
        })
    });

    let start = Instant::now();
    let status = loop {
        if let Some(st) = child.try_wait().context("等待 AI 引擎失败")? {
            break st;
        }
        if start.elapsed() >= PROCESS_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            bail!("AI 超分超时（超过 {}s）", PROCESS_TIMEOUT.as_secs());
        }
        std::thread::sleep(Duration::from_millis(50));
    };

    let _ = out_reader.map(|h| h.join());
    let err_text = match err_reader {
        Some(h) => h.join().unwrap_or_default(),
        None => String::new(),
    };
    Ok((status.success(), err_text))
}

/// 单张超分。
pub fn super_resolve(page_bytes: &[u8], scale: u32) -> Result<Vec<u8>> {
    let hash = sha256_hex(page_bytes);
    if let Some(cached) = read_cached(&hash, scale)? {
        return Ok(cached);
    }

    let img = image::load_from_memory(page_bytes).context("无法解码页面图片")?;
    let temp = CacheDir::Temp.ensure()?;
    let seq = next_seq();
    let input = temp.join(format!("rch_in_{hash}_{seq}.png"));
    let output = temp.join(format!("rch_out_{hash}_{seq}.png"));
    img.save(&input)?;

    let ai = exe_path()?;
    let models = models_dir()?;
    let mut cmd = std::process::Command::new(&ai);
    cmd.args([
        "-i",
        input.to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
        "-s",
        &scale.to_string(),
        "-n",
        MODEL_NAME,
        "-m",
        models.to_str().unwrap(),
    ]);

    let (ok, err_text) = match run_cli(cmd) {
        Ok(r) => r,
        Err(e) => {
            let _ = std::fs::remove_file(&input);
            let _ = std::fs::remove_file(&output);
            return Err(e);
        }
    };
    if !ok {
        let _ = std::fs::remove_file(&input);
        let _ = std::fs::remove_file(&output);
        bail!("AI 超分失败: {}", err_text.trim());
    }

    let rimg = match image::open(&output) {
        Ok(i) => i,
        Err(e) => {
            let _ = std::fs::remove_file(&input);
            let _ = std::fs::remove_file(&output);
            return Err(e).context("读取超分结果失败");
        }
    };
    let _ = std::fs::remove_file(&input);
    let _ = std::fs::remove_file(&output);

    let mut buf = Vec::new();
    let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, JPEG_QUALITY);
    rimg.write_with_encoder(enc)?;
    let dir = CacheDir::Ai.ensure()?;
    write_cache_atomic(&dir, &cache_key(&hash, scale), &buf)?;
    Ok(buf)
}

/// 批量超分 — 一次 CLI 调用处理整个目录（Phase 2）。
///
/// 返回与输入对齐的结果：成功页为 JPEG 字节，失败页为空 Vec；
/// 全部未缓存页均失败时才返回 Err。已缓存页直接命中。
pub fn super_resolve_batch(pages: &[Vec<u8>], scale: u32) -> Result<Vec<Vec<u8>>> {
    if pages.is_empty() {
        return Ok(vec![]);
    }
    let dir = CacheDir::Ai.ensure()?;
    let mut results: Vec<Vec<u8>> = vec![Vec::new(); pages.len()];
    let mut uncached: Vec<(usize, String)> = Vec::new();

    for (i, b) in pages.iter().enumerate() {
        let h = sha256_hex(b);
        let key = cache_key(&h, scale);
        match std::fs::read(dir.join(&key)) {
            Ok(data) => results[i] = data,
            Err(_) => {
                let _ = std::fs::remove_file(dir.join(&key));
                uncached.push((i, h));
            }
        }
    }
    if uncached.is_empty() {
        return Ok(results);
    }

    let temp = CacheDir::Temp.ensure()?;
    let seq = next_seq();
    let in_dir = temp.join(format!("batch_in_{seq}"));
    let out_dir = temp.join(format!("batch_out_{seq}"));
    std::fs::create_dir_all(&in_dir)?;
    std::fs::create_dir_all(&out_dir)?;

    // 写入输入：解码失败的页跳过（结果保持空，由调用方计数为失败）。
    for (idx, hash) in &uncached {
        if let Ok(img) = image::load_from_memory(&pages[*idx]) {
            let _ = img.save(in_dir.join(format!("{hash}.png")));
        }
    }
    let has_inputs = std::fs::read_dir(&in_dir)?.next().is_some();
    if !has_inputs {
        let _ = std::fs::remove_dir_all(&in_dir);
        let _ = std::fs::remove_dir_all(&out_dir);
        return Ok(results);
    }

    let ai = exe_path()?;
    let models = models_dir()?;
    let mut cmd = std::process::Command::new(&ai);
    cmd.args([
        "-i",
        in_dir.to_str().unwrap(),
        "-o",
        out_dir.to_str().unwrap(),
        "-s",
        &scale.to_string(),
        "-n",
        MODEL_NAME,
        "-m",
        models.to_str().unwrap(),
    ]);

    let cli_ok = match run_cli(cmd) {
        Ok((ok, _)) => ok,
        Err(_) => false,
    };
    let _ = std::fs::remove_dir_all(&in_dir);

    if !cli_ok {
        let _ = std::fs::remove_dir_all(&out_dir);
        let all_failed = uncached.iter().all(|(i, _)| results[*i].is_empty());
        if all_failed {
            bail!("AI 批量超分失败");
        }
        return Ok(results);
    }

    for (idx, hash) in &uncached {
        let out_file = out_dir.join(format!("{hash}.png"));
        if let Ok(img) = image::open(&out_file) {
            let mut buf = Vec::new();
            let enc =
                image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, JPEG_QUALITY);
            if img.write_with_encoder(enc).is_ok() {
                let _ = write_cache_atomic(&dir, &cache_key(hash, scale), &buf);
                results[*idx] = buf;
            }
        }
    }
    let _ = std::fs::remove_dir_all(&out_dir);

    let all_failed = uncached.iter().all(|(i, _)| results[*i].is_empty());
    if all_failed {
        bail!("AI 批量超分失败：所有未缓存页面均失败");
    }
    Ok(results)
}

/// 查询 AI 超分缓存，命中返回 JPEG，未命中返回 None。
pub fn lookup_cache(page_bytes: &[u8], scale: u32) -> Option<Vec<u8>> {
    let hash = sha256_hex(page_bytes);
    let p = CacheDir::Ai.path().join(cache_key(&hash, scale));
    std::fs::read(&p).ok()
}

/// 删除某页的 AI 超分缓存（取消整本超分时按页清理，不影响其他书）。
pub fn delete_ai_cache_for_page(page_bytes: &[u8], scale: u32) {
    let hash = sha256_hex(page_bytes);
    let _ = std::fs::remove_file(CacheDir::Ai.path().join(cache_key(&hash, scale)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash() {
        assert_eq!(sha256_hex(b"hello").len(), 64);
    }

    #[test]
    fn test_key() {
        let k = cache_key("abc", 2);
        assert!(k.starts_with("abc_"));
        assert!(k.contains(MODEL_NAME));
        assert!(k.ends_with("_2x.ai"));
    }

    #[test]
    fn test_key_differs_by_scale() {
        assert_ne!(cache_key("abc", 2), cache_key("abc", 4));
    }

    #[test]
    fn webp_decode_probe() {
        // 临时探针：验证 image crate 能否解码真实 WebP 页面（用户漫画页为 WebP）。
        let path = std::env::temp_dir().join("rch_probe_page.webp");
        let Ok(bytes) = std::fs::read(&path) else {
            eprintln!("跳过：缺少探针文件 {:?}", path);
            return;
        };
        let img = image::load_from_memory(&bytes).expect("WEBP 解码失败");
        println!("WEBP 解码成功: {}x{}", img.width(), img.height());
    }
}
