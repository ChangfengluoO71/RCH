//! E 站标签中文翻译（内置基线 + 缺失时按需更新）。
//!
//! 数据来源：[EhTagTranslation/Database](https://github.com/EhTagTranslation/Database)（GNU FDL，允许二次分发）。
//! 仓库内 `data/eh_tag_zh.json` 是**内置基线**（1369 条，按 `命名空间:原始标签` 为键），
//! 因此离线也能翻译；遇到基线里没有的标签时，才按命名空间去上游拉取一次并合并缓存。
//!
//! 为什么键要带命名空间：同一英文标签在不同命名空间可能不同义，裸标签名会互相覆盖
//! （实测用裸名做键会把 `male:*` 的大半条目吞掉：male 从 575 掉到 86）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64)";

/// 内置基线（离线可用）。
const BASELINE: &str = include_str!("../data/eh_tag_zh.json");

/// 上游按命名空间分文件，`{ns}` 为占位。
const UPSTREAM: &str =
    "https://raw.githubusercontent.com/EhTagTranslation/Database/master/database/{ns}.md";

/// 参与翻译的命名空间（上游文件名即命名空间）。
pub const NAMESPACES: &[&str] = &["female", "male", "mixed", "other", "language", "reclass"];

fn baseline() -> &'static HashMap<String, String> {
    static MAP: OnceLock<HashMap<String, String>> = OnceLock::new();
    MAP.get_or_init(|| serde_json::from_str(BASELINE).unwrap_or_default())
}

/// 运行时增量（联网更新到的条目），与基线分开存，便于区分来源。
fn overlay() -> &'static Mutex<HashMap<String, String>> {
    static MAP: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

fn key(namespace: &str, raw: &str) -> String {
    format!("{}:{}", namespace.trim().to_lowercase(), raw.trim().to_lowercase())
}

/// 查中文译名。命中基线或已更新的增量即返回。
pub fn translate(namespace: &str, raw: &str) -> Option<String> {
    let k = key(namespace, raw);
    if let Some(zh) = baseline().get(&k) {
        return Some(clean(zh));
    }
    overlay().lock().ok()?.get(&k).map(|zh| clean(zh))
}

/// 基线条目数（自检用）。
pub fn baseline_len() -> usize {
    baseline().len()
}

/// 清除译名里的装饰：上游部分条目带 emoji / HTML / 图片语法（实测 `kissing → 接吻💏`）。
pub fn clean(zh: &str) -> String {
    let no_html = strip_html(zh);
    let no_emoji = strip_emoji(&no_html);
    let trimmed = no_emoji.trim();
    if trimmed.is_empty() { zh.trim().to_string() } else { trimmed.to_string() }
}

fn strip_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}

/// 去掉 emoji 与变体选择符（保留中文/字母/数字/常规标点）。
fn strip_emoji(s: &str) -> String {
    s.chars()
        .filter(|c| {
            let u = *c as u32;
            !(0x1F300..=0x1FAFF).contains(&u)   // 各类 emoji
                && !(0x2600..=0x27BF).contains(&u) // 杂项符号/装饰
                && u != 0xFE0F                     // 变体选择符
                && u != 0x200D // 零宽连接符
        })
        .collect()
}

/// 解析上游 markdown 表格：`| 原始标签 | 中文名 | 描述 | 链接 |`。
///
/// 跳过：表头/分隔行、`== 分类 ==` 行、无中文名的行。
pub fn parse_upstream_markdown(namespace: &str, text: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if !line.starts_with('|') {
            continue;
        }
        let cells: Vec<&str> = line.trim_matches('|').split('|').map(str::trim).collect();
        if cells.len() < 3 {
            continue;
        }
        let (raw, zh) = (cells[0], cells[1]);
        if raw.is_empty() || raw == "原始标签" || raw.chars().all(|c| c == '-' || c == ' ') {
            continue;
        }
        if raw.starts_with("==") || zh.is_empty() || zh.starts_with("==") {
            continue;
        }
        out.insert(key(namespace, raw), clean(zh));
    }
    out
}

fn cache_path(dir: &str) -> PathBuf {
    Path::new(dir).join("eh_tag_zh_cache.json")
}

/// 读取磁盘缓存（上次联网更新得到的条目）并并入增量。
pub fn load_cache(dir: &str) {
    if dir.trim().is_empty() {
        return;
    }
    if let Ok(text) = std::fs::read_to_string(cache_path(dir)) {
        if let Ok(map) = serde_json::from_str::<HashMap<String, String>>(&text) {
            if let Ok(mut o) = overlay().lock() {
                for (k, v) in map {
                    o.insert(k, v);
                }
            }
        }
    }
}

/// 缺失时按需更新：仅拉取**指定命名空间**，解析后并入增量并写回缓存。
///
/// 返回本次新增条数。网络失败只返回错误，不影响已有基线。
pub fn update_namespace(namespace: &str, cache_dir: &str) -> Result<usize, String> {
    let ns = namespace.trim().to_lowercase();
    if !NAMESPACES.contains(&ns.as_str()) {
        return Err(format!("未知命名空间：{ns}"));
    }
    let url = UPSTREAM.replace("{ns}", &ns);
    let text = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| format!("创建客户端失败：{e}"))?
        .get(&url)
        .header("User-Agent", UA)
        .send()
        .map_err(|e| format!("拉取失败：{e}"))?
        .text()
        .map_err(|e| format!("读取响应失败：{e}"))?;
    let parsed = parse_upstream_markdown(&ns, &text);
    let mut added = 0usize;
    if let Ok(mut o) = overlay().lock() {
        for (k, v) in parsed {
            if baseline().get(&k).is_none() && !o.contains_key(&k) {
                added += 1;
            }
            o.insert(k, v);
        }
        persist(cache_dir, &o);
    }
    Ok(added)
}

fn persist(dir: &str, map: &HashMap<String, String>) {
    if dir.trim().is_empty() {
        return;
    }
    if let Some(parent) = cache_path(dir).parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(text) = serde_json::to_string(map) {
        let _ = std::fs::write(cache_path(dir), text);
    }
}

/// 翻译入口：先查基线/增量；仍缺失且允许联网时，拉一次该命名空间再查。
pub fn translate_with_update(
    namespace: &str,
    raw: &str,
    allow_network: bool,
    cache_dir: &str,
) -> Option<String> {
    if let Some(zh) = translate(namespace, raw) {
        return Some(zh);
    }
    if !allow_network {
        return None;
    }
    let ns = namespace.trim().to_lowercase();
    if !NAMESPACES.contains(&ns.as_str()) {
        return None;
    }
    let _ = update_namespace(&ns, cache_dir);
    translate(namespace, raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baseline_is_complete_and_namespaced() {
        // 1369 = female 613 + male 575 + language 87 + other 60 + mixed 23 + reclass 11
        assert_eq!(baseline_len(), 1369, "内置基线条目数应符合预期");
        assert_eq!(translate("female", "big breasts").as_deref(), Some("巨乳"));
        assert_eq!(translate("male", "sole male").as_deref(), Some("单男主"));
        assert_eq!(translate("other", "uncensored").as_deref(), Some("无修正"));
        assert_eq!(translate("language", "chinese").as_deref(), Some("汉语"));
    }

    #[test]
    fn lookup_is_case_and_space_insensitive() {
        assert_eq!(translate("Female", "  BIG BREASTS ").as_deref(), Some("巨乳"));
        assert!(translate("female", "definitely-not-a-tag").is_none());
    }

    #[test]
    fn namespaced_key_prevents_collisions() {
        // 裸标签名做键会把同名跨命名空间条目吞掉（male 575 → 86）；
        // 这里确认两个命名空间可各自命中原生条目。
        assert!(translate("male", "blindfold").is_some(), "male 条目应能命中");
        assert!(translate("female", "blindfold").is_some(), "female 同名条目也应能命中");
    }

    #[test]
    fn parses_upstream_markdown_rows() {
        let md = r#"
| 原始标签 | 名称 | 描述 | 外部链接 |
| -------- | ---- | ---- | -------- |
|  | == Age == | == 年龄 == |  |
| age progression | 年龄增长 | 描述 |  |
| lolicon | 萝莉💏 | 描述 |  |
|  | 分类标题但无英文名 | 描述 |  |
"#;
        let m = parse_upstream_markdown("female", md);
        assert_eq!(m.get("female:age progression").map(String::as_str), Some("年龄增长"));
        // emoji 应被清洗
        assert_eq!(m.get("female:lolicon").map(String::as_str), Some("萝莉"));
        // `== 分类 ==` 行与无英文名的行应被跳过
        assert!(!m.keys().any(|k| k.contains("==")));
        assert_eq!(m.len(), 2);
    }

    #[test]
    fn clean_strips_emoji_and_html() {
        assert_eq!(clean("接吻💏"), "接吻");
        assert_eq!(clean("萝莉<br>"), "萝莉");
        assert_eq!(clean("无修正"), "无修正");
        // 全是 emoji 时退回原文，避免产出空标签
        assert_eq!(clean("💏"), "💏");
    }

    #[test]
    fn unknown_namespace_is_rejected_without_network() {
        assert!(update_namespace("nope", "").is_err());
        assert!(translate_with_update("nope", "x", true, "").is_none());
    }

    /// 可选网络测试：验证"缺失时按需更新"这条链路真的可用。
    /// 默认 `#[ignore]`，需要时手动跑：
    /// `cargo test --lib eh_tag_translation -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn upstream_update_path_works() {
        let dir = std::env::temp_dir().join("rch_eh_tag_cache_test");
        let dir = dir.to_string_lossy().to_string();
        let _ = std::fs::remove_file(cache_path(&dir));
        // 先确认基线里没有的键（构造一个不可能存在的标签）会走更新流程
        assert!(translate("female", "zzz-not-a-real-tag").is_none());
        let added = update_namespace("female", &dir).expect("上游拉取应成功");
        println!("本次新增 {added} 条；缓存文件存在 = {}", cache_path(&dir).exists());
        // 真实标签应从上游拿到（即便基线缺失也能译出）
        let zh = translate("female", "lolicon");
        println!("female:lolicon -> {zh:?}");
        assert!(zh.is_some(), "上游更新后应能译出常见标签");
        assert!(cache_path(&dir).exists(), "应写出缓存文件");
    }

}