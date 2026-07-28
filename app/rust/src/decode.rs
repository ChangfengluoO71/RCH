//! 图片解码与降采样。

use anyhow::{Context, Result};
use image::imageops::FilterType;

/// 解码后的位图(RGBA8888)。
pub struct DecodedImage {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// 把编码图片字节(JPEG/PNG/WebP/GIF)解码为 RGBA。
/// `max_dim` 限制最长边:超过则按比例降采样,控制内存占用。
pub fn decode(bytes: &[u8], max_dim: Option<u32>) -> Result<DecodedImage> {
    let img = image::load_from_memory(bytes).context("图片解码失败")?;
    let img = match max_dim {
        Some(m) if m > 0 && img.width().max(img.height()) > m => img.thumbnail(m, m),
        _ => img,
    };
    let rgba = img.to_rgba8();
    let width = rgba.width();
    let height = rgba.height();
    Ok(DecodedImage {
        rgba: rgba.into_raw(),
        width,
        height,
    })
}

/// 生成封面位图:先按 `crop`(相对 0-1 的 x/y/w/h)裁剪,再等比缩放并中心裁剪到恰好 `w×h`。
pub fn decode_cover(
    bytes: &[u8],
    w: u32,
    h: u32,
    crop: Option<(f64, f64, f64, f64)>,
) -> Result<DecodedImage> {
    let img = image::load_from_memory(bytes).context("图片解码失败")?;
    let img = if let Some((x, y, cw, ch)) = crop {
        let (iw, ih) = (img.width(), img.height());
        let cx = ((x * iw as f64) as u32).min(iw.saturating_sub(1));
        let cy = ((y * ih as f64) as u32).min(ih.saturating_sub(1));
        let cw2 = ((cw * iw as f64) as u32).max(1).min(iw - cx);
        let ch2 = ((ch * ih as f64) as u32).max(1).min(ih - cy);
        img.crop_imm(cx, cy, cw2, ch2)
    } else {
        img
    };
    let filled = img.resize_to_fill(w.max(1), h.max(1), FilterType::Triangle);
    let rgba = filled.to_rgba8();
    let width = rgba.width();
    let height = rgba.height();
    Ok(DecodedImage {
        rgba: rgba.into_raw(),
        width,
        height,
    })
}
