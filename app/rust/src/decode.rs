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

/// 图片魔数（只按文件头判定，**不依赖** `image` 的编译特性）。
///
/// 两个用途：
/// 1. 过滤"看起来是图片"的记录 —— MOBI 的 `image_records()` 只做了**非图片黑名单**
///    （FLIS/FCIS/INDX…），KF8/AZW3 里的 CSS/HTML/其它资源记录会被当成"页"，
///    于是 `page_bytes(0)` 不是图片、封面永远失败（③ 实测：33.9MB MOBI ⇒ `cover_decode_failed`）；
/// 2. 诊断日志里给出**安全标签**（只记格式名，不记任何内容或原文）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageMagic {
    Jpeg,
    Png,
    Gif,
    Webp,
    Bmp,
    Tiff,
    Ico,
    Heif,
    Zip,
    TextLike,
    Unknown,
}

/// 当前构建**真的能解码**的魔数（`image` 特性：jpeg/png/webp/gif）。
pub fn image_magic_decodable(magic: ImageMagic) -> bool {
    matches!(
        magic,
        ImageMagic::Jpeg | ImageMagic::Png | ImageMagic::Gif | ImageMagic::Webp
    )
}

/// 安全标签（写进诊断日志的固定枚举，绝不含内容）。
pub const fn image_magic_label(magic: ImageMagic) -> &'static str {
    match magic {
        ImageMagic::Jpeg => "jpeg",
        ImageMagic::Png => "png",
        ImageMagic::Gif => "gif",
        ImageMagic::Webp => "webp",
        ImageMagic::Bmp => "bmp",
        ImageMagic::Tiff => "tiff",
        ImageMagic::Ico => "ico",
        ImageMagic::Heif => "heif/avif",
        ImageMagic::Zip => "zip",
        ImageMagic::TextLike => "text-like",
        ImageMagic::Unknown => "unknown",
    }
}

/// 按魔数嗅探；识别不出可解码格式时也会给出**不可解码但可命名**的标签
/// （bmp/tiff/heif…），这正是"为什么解不开"的关键诊断信息。
pub fn sniff_image_magic(bytes: &[u8]) -> ImageMagic {
    let starts = |needle: &[u8]| bytes.len() >= needle.len() && &bytes[..needle.len()] == needle;
    if starts(&[0xFF, 0xD8, 0xFF]) {
        ImageMagic::Jpeg
    } else if starts(b"\x89PNG\r\n\x1a\n") {
        ImageMagic::Png
    } else if starts(b"GIF87a") || starts(b"GIF89a") {
        ImageMagic::Gif
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        ImageMagic::Webp
    } else if starts(b"BM") {
        ImageMagic::Bmp
    } else if starts(&[0x49, 0x49, 0x2A, 0x00]) || starts(&[0x4D, 0x4D, 0x00, 0x2A]) {
        ImageMagic::Tiff
    } else if starts(&[0x00, 0x00, 0x01, 0x00]) {
        ImageMagic::Ico
    } else if bytes.len() >= 12 && &bytes[4..8] == b"ftyp" {
        ImageMagic::Heif
    } else if starts(b"PK\x03\x04") {
        ImageMagic::Zip
    } else if starts(b"<") || starts(b"\xEF\xBB\xBF<") || starts(b"\xFF\xFE<") {
        ImageMagic::TextLike
    } else {
        ImageMagic::Unknown
    }
}

#[cfg(test)]
mod magic_tests {
    use super::*;

    #[test]
    fn sniffs_known_magics_and_names_undecodable_ones() {
        assert_eq!(
            sniff_image_magic(&[0xFF, 0xD8, 0xFF, 0xE0]),
            ImageMagic::Jpeg
        );
        assert_eq!(sniff_image_magic(b"\x89PNG\r\n\x1a\n\0\0"), ImageMagic::Png);
        assert_eq!(sniff_image_magic(b"GIF89a....."), ImageMagic::Gif);
        assert_eq!(
            sniff_image_magic(b"RIFF\x00\x00\x00\x00WEBPVP8 "),
            ImageMagic::Webp
        );
        // 关键：不可解码但**可命名** —— 这正是诊断"为什么解不开"的答案。
        assert_eq!(sniff_image_magic(b"BM\x36\x00\x00\x00"), ImageMagic::Bmp);
        assert_eq!(
            sniff_image_magic(&[0x49, 0x49, 0x2A, 0x00]),
            ImageMagic::Tiff
        );
        assert_eq!(sniff_image_magic(b"\x00\x00\x00\x18ftypavif"), ImageMagic::Heif);
        assert_eq!(sniff_image_magic(b"<!DOCTYPE html>"), ImageMagic::TextLike);
        assert_eq!(sniff_image_magic(b"PK\x03\x04zipdata"), ImageMagic::Zip);
        assert_eq!(sniff_image_magic(b"\x01\x02\x03"), ImageMagic::Unknown);
        assert_eq!(sniff_image_magic(b""), ImageMagic::Unknown);

        assert!(image_magic_decodable(ImageMagic::Jpeg));
        assert!(image_magic_decodable(ImageMagic::Webp));
        assert!(!image_magic_decodable(ImageMagic::Bmp));
        assert!(!image_magic_decodable(ImageMagic::TextLike));
        assert_eq!(image_magic_label(ImageMagic::Bmp), "bmp");
    }
}
