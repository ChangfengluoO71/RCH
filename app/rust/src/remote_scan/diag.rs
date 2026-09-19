//! 扫描 / 封面**诊断日志**（`<数据根>/scan_diag.log`）。
//!
//! # 为什么需要
//!
//! 第 52–55 轮真实库审查发现：应用连续运行 2.5 小时、产生 225 个封面失败，
//! 而 `errors.log`（只记未捕获异常）与 `scan_diag.log` **一行都没有新增** ——
//! "盯日志"这条通道当时是空的，只能翻数据库。生产问题的第一手证据必须在日志里。
//!
//! # 约束（与 Dart 侧 `appendScanDiag` 同一文件、同一约定）
//!
//! * **行为中立**：写日志失败绝不影响业务（全部 `let _ =`）；目录不存在就静默放弃。
//! * **脱敏**：只写**安全枚举码**、源标识与短哈希；绝不写路径、凭据、直链、响应正文。
//! * 时间戳：Rust 侧写 **UTC + `Z`**（无第三方依赖）；Dart 侧既有行是**本地时间无时区**。
//!   两者都保留 ISO8601 前缀，便于直接排序阅读。

use std::io::Write;

/// 追加一行诊断（自动加 UTC 时间戳前缀）。失败即返回，不 panic、不影响调用方。
pub(crate) fn note(line: &str) {
    let path = crate::cache::cache_root().join("scan_diag.log");
    let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    else {
        return;
    };
    let _ = writeln!(file, "{} {line}", utc_stamp());
}

/// 资产标识的**安全标签**：本身就是 64 位十六进制哈希时取前 12 位（便于回查数据库），
/// 否则对其取 SHA-256 前 12 位（避免把可能含路径的标识写进日志）。
pub(crate) fn safe_asset_label(asset_id: &str) -> String {
    if asset_id.len() == 64 && asset_id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return asset_id[..12].to_string();
    }
    use sha2::Digest;
    let digest = format!("{:x}", sha2::Sha256::digest(asset_id.as_bytes()));
    digest[..12].to_string()
}

/// `YYYY-MM-DDTHH:MM:SSZ`（UTC，civil-from-days，无第三方依赖）。
fn utc_stamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { yoe + era * 400 + 1 } else { yoe + era * 400 };
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3_600,
        (rem % 3_600) / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamp_is_iso8601_utc_without_dependencies() {
        let stamp = utc_stamp();
        assert_eq!(stamp.len(), 20, "stamp must be YYYY-MM-DDTHH:MM:SSZ");
        assert!(stamp.ends_with('Z'));
        assert_eq!(&stamp[4..5], "-");
        assert_eq!(&stamp[10..11], "T");
        assert_eq!(&stamp[13..14], ":");
        // 年份必须是 2020+（防止 civil 换算写反导致 1970 之类的回归）
        let year: i32 = stamp[..4].parse().unwrap();
        assert!(year >= 2020, "unexpected year in {stamp}");
    }

    #[test]
    fn asset_label_never_leaks_a_path_shaped_id() {
        // 64 位十六进制哈希：直接取前 12 位，便于回查数据库行。
        let hashed = "a".repeat(64);
        assert_eq!(safe_asset_label(&hashed), "aaaaaaaaaaaa");
        // 形如 `type|source|/私有/路径.cbz` 的标识：只留哈希前缀，绝不透传路径。
        let path_shaped = "quark|source-1|/私人/漫画/某本.cbz";
        let label = safe_asset_label(path_shaped);
        assert_eq!(label.len(), 12);
        assert!(!label.contains('/') && !label.contains("漫画"));
        assert_ne!(label, safe_asset_label("quark|source-1|/私人/漫画/另一本.cbz"));
    }
}
