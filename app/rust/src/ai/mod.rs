//! M2 AI 超分引擎 — Phase 2 CLI 常驻 Worker 版。
//!
//! 保留 CLI 进程，但改为 spawn 一次、多张图批量 stdin/stdout 通信。
//! 因为 realesrgan-ncnn-vulkan.exe 不支持 stdin 传图，沿用目录批量模式：
//! 一次 CLI 调用处理整个输入目录的所有 PNG。

use crate::cache::CacheDir;
use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use std::path::PathBuf;

const MODEL_NAME: &str = "realesr-animevideov3";

/// 定位 exe
fn exe_path() -> Result<PathBuf> {
    let exe = std::env::current_exe().context("无法获取可执行文件路径")?;
    let exe_dir = exe.parent().context("无法获取可执行文件目录")?;
    for p in &[
        exe_dir.join("data").join("ai").join("realesrgan-ncnn-vulkan.exe"),
        exe_dir.join("ai").join("realesrgan-ncnn-vulkan.exe"),
    ] {
        if p.exists() { return Ok(p.clone()); }
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

fn cache_key(hash: &str, scale: u32) -> String { format!("{hash}_rv3_{scale}x.ai") }

/// 单张超分。
pub fn super_resolve(page_bytes: &[u8], scale: u32) -> Result<Vec<u8>> {
    let hash = sha256_hex(page_bytes);
    let key = cache_key(&hash, scale);
    let dir = CacheDir::Ai.ensure()?;
    let cached = dir.join(&key);
    if cached.exists() { return std::fs::read(&cached).context("读缓存失败"); }

    let img = image::load_from_memory(page_bytes)?;
    let temp = CacheDir::Temp.ensure()?;
    let input = temp.join(format!("rch_in_{hash}.png"));
    let output = temp.join(format!("rch_out_{hash}.png"));
    img.save(&input)?;

    let ai = exe_path()?;
    let models = models_dir()?;
    let mut cmd = std::process::Command::new(&ai);
    cmd.args(["-i", input.to_str().unwrap(), "-o", output.to_str().unwrap(),
              "-s", &scale.to_string(), "-n", MODEL_NAME, "-m", models.to_str().unwrap()]);
    let child = cmd.spawn()?;
    let out = child.wait_with_output()?;
    let _ = std::fs::remove_file(&input);
    if !out.status.success() {
        let _ = std::fs::remove_file(&output);
        bail!("AI 超分失败: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    let rimg = image::open(&output)?;
    let _ = std::fs::remove_file(&output);
    let mut buf = Vec::new();
    rimg.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Jpeg)?;
    let _ = std::fs::write(&cached, &buf);
    Ok(buf)
}

/// 批量超分 — 一次 CLI 调用处理整个目录。
pub fn super_resolve_batch(pages: &[Vec<u8>], scale: u32) -> Result<Vec<Vec<u8>>> {
    if pages.is_empty() { return Ok(vec![]); }
    let dir = CacheDir::Ai.ensure()?;
    let mut results = vec![None; pages.len()];
    let mut uncached: Vec<(usize, String)> = vec![];

    for (i, b) in pages.iter().enumerate() {
        let h = sha256_hex(b);
        let c = dir.join(cache_key(&h, scale));
        if c.exists() { results[i] = std::fs::read(&c).ok(); continue; }
        uncached.push((i, h));
    }
    if uncached.is_empty() { return results.into_iter().map(|r| r.ok_or_else(|| anyhow::anyhow!("missing"))).collect(); }

    let temp = CacheDir::Temp.ensure()?;
    let batch_id = format!("{:x}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos());
    let in_dir = temp.join(format!("batch_in_{batch_id}"));
    let out_dir = temp.join(format!("batch_out_{batch_id}"));
    std::fs::create_dir_all(&in_dir)?;
    std::fs::create_dir_all(&out_dir)?;

    for (idx, hash) in &uncached {
        let img = image::load_from_memory(&pages[*idx])?;
        img.save(in_dir.join(format!("{hash}.png")))?;
    }

    let ai = exe_path()?;
    let models = models_dir()?;
    let mut cmd = std::process::Command::new(&ai);
    cmd.args(["-i", in_dir.to_str().unwrap(), "-o", out_dir.to_str().unwrap(),
              "-s", &scale.to_string(), "-n", MODEL_NAME, "-m", models.to_str().unwrap()]);
    let child = cmd.spawn()?;
    let out = child.wait_with_output()?;
    let _ = std::fs::remove_dir_all(&in_dir);

    if !out.status.success() {
        let _ = std::fs::remove_dir_all(&out_dir);
        bail!("AI 批量超分失败: {}", String::from_utf8_lossy(&out.stderr).trim());
    }

    for (idx, hash) in &uncached {
        let out_file = out_dir.join(format!("{hash}.png"));
        if out_file.exists() {
            match (|| -> Result<Vec<u8>> {
                let img = image::open(&out_file)?;
                let mut buf = Vec::new();
                img.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Jpeg)?;
                Ok(buf)
            })() {
                Ok(jpeg) => {
                    let _ = std::fs::write(dir.join(cache_key(hash, scale)), &jpeg);
                    results[*idx] = Some(jpeg);
                }
                Err(_) => {}
            }
        }
    }
    let _ = std::fs::remove_dir_all(&out_dir);

    results.into_iter().map(|r| r.ok_or_else(|| anyhow::anyhow!("超分失败"))).collect()
}

/// 查询 AI 超分缓存，命中返回 JPEG，未命中返回 None。
pub fn lookup_cache(page_bytes: &[u8], scale: u32) -> Option<Vec<u8>> {
    let hash = sha256_hex(page_bytes);
    let key = cache_key(&hash, scale);
    let dir = CacheDir::Ai.path();
    let cached = dir.join(&key);
    if cached.exists() {
        std::fs::read(&cached).ok()
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn test_hash() { assert_eq!(sha256_hex(b"hello").len(), 64); }
    #[test] fn test_key() { assert!(cache_key("abc", 2).ends_with(".ai")); }
}
