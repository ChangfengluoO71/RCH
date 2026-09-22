//! 本地作品 → E 站画廊的匹配引擎（纯函数，可离线回归）。
//!
//! 结论来自 2026-09-22 的实测（见 `docs/research/eh-metadata-import-feasibility.md` §4）：
//! * **锚点必须用裸词**：`artist:"…"` / `group:"…"` 这类命名空间过滤实测全部返回 0 条，
//!   而裸词（如 `朝凪`、`Fatalpulse`）各返回 25 条且 top1 就是该画师作品。
//! * **只能比 `title_jpn`**：gdata 的 `title` 是罗马字，与种子名/本地名比对全部失败。
//! * **锚点顺序以作品名为主**：真实语料里 `semantic.work_title` 覆盖 389/389（100%），
//!   而 `semantic.creators` 仅 158/389（41%）；且作者锚点对冷门作品召回不足。
//!
//! 打分用**字符二元组 Dice 系数**（对日文/中文标题比词切分稳），阈值默认
//! [`MATCH_THRESHOLD`]。两个"不许猜"的保护：
//! * **同系列不同卷**：标题除数字外一致但数字不同 → `Ambiguous`（实测 `孕ませ屋2` vs
//!   `孕ませ屋4` 得 0.75，高于阈值，若不拦就会张冠李戴）。
//! * **并列接近**：最高分与次高分差距过小且是两个不同作品 → `Ambiguous`。

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// 判定命中的相似度阈值（调研结论：0.5 起步）。
pub const MATCH_THRESHOLD: f64 = 0.5;

/// 并列接近的容差：次高分与最高分之差小于该值且作品不同 → 需人工确认。
pub const TIE_MARGIN: f64 = 0.05;

/// 一个候选画廊的匹配结果。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MatchHit {
    pub gid: String,
    pub title: String,
    pub title_jpn: String,
    pub score: f64,
}

/// 匹配结论。`Ambiguous` 一律**不自动写入**。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum MatchDecision {
    /// 唯一命中。
    Matched(MatchHit),
    /// 同一作品的多个版本/语言（归一化标题一致）——都是同一部，可取最高分。
    Editions(Vec<MatchHit>),
    /// 疑似同系列不同卷，或与其他候选并列接近：需人工确认。
    Ambiguous(Vec<MatchHit>),
    /// 未命中（低于阈值）：不猜。
    Unmatched,
}

/// 匹配用的标题归一化：去掉方括号块（`[作者]`）、圆括号块（`(原作)`），
/// 再只保留字母/数字/日文假名/汉字并小写。
///
/// 与种子映射用的 [`crate::eh_subscription::normalize_for_match`] 分开：
/// 那边比的是种子文件名（保留更多字符更安全），这边比的是作品名（需要剥离标记）。
pub fn normalize_title(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut depth_square = 0i32;
    let mut depth_round = 0i32;
    for c in s.chars() {
        match c {
            '[' | '【' => depth_square += 1,
            ']' | '】' => depth_square = (depth_square - 1).max(0),
            '(' | '（' => depth_round += 1,
            ')' | '）' => depth_round = (depth_round - 1).max(0),
            _ if depth_square > 0 || depth_round > 0 => {}
            _ => {
                for lc in c.to_lowercase() {
                    if lc.is_alphanumeric() {
                        out.push(lc);
                    }
                }
            }
        }
    }
    out
}

fn bigrams(s: &str) -> BTreeSet<String> {
    let chars: Vec<char> = s.chars().collect();
    if chars.is_empty() {
        return BTreeSet::new();
    }
    if chars.len() == 1 {
        return BTreeSet::from([chars[0].to_string()]);
    }
    (0..chars.len() - 1)
        .map(|i| chars[i..i + 2].iter().collect::<String>())
        .collect()
}

/// 字符二元组 Dice 系数：`2|A∩B| / (|A|+|B|)`，范围 0..1。
pub fn dice(a: &str, b: &str) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let (a, b) = (bigrams(a), bigrams(b));
    let inter = a.intersection(&b).count();
    2.0 * inter as f64 / (a.len() + b.len()) as f64
}

/// 单个候选的得分：取 `title_jpn` 与罗马字 `title` 中较高者。
pub fn score_candidate(local_title: &str, candidate_jpn: &str, candidate_romaji: &str) -> f64 {
    let local = normalize_title(local_title);
    let a = dice(&local, &normalize_title(candidate_jpn));
    let b = if candidate_romaji.trim().is_empty() {
        0.0
    } else {
        dice(&local, &normalize_title(candidate_romaji))
    };
    a.max(b)
}

/// 提取数字串（用于识别"第几卷"这类差异）。
fn digit_runs(s: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut cur = String::new();
    for c in s.chars() {
        if c.is_ascii_digit() {
            cur.push(c);
        } else if !cur.is_empty() {
            out.insert(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        out.insert(cur);
    }
    out
}

/// 是否"疑似同系列不同卷"：除数字外一致，但数字不同。
///
/// 一侧完全没有数字时**不算冲突**（本地名可能不带卷号，而候选是基础作品）。
pub fn volume_conflict(local_norm: &str, candidate_norm: &str) -> bool {
    let (a, b) = (digit_runs(local_norm), digit_runs(candidate_norm));
    if a.is_empty() || b.is_empty() {
        return false;
    }
    a != b
}

/// 判定。`candidates` 为 `(gid, title, title_jpn)`。
pub fn decide(local_title: &str, candidates: &[(String, String, String)]) -> MatchDecision {
    let local_norm = normalize_title(local_title);
    if local_norm.is_empty() || candidates.is_empty() {
        return MatchDecision::Unmatched;
    }

    let mut hits: Vec<MatchHit> = candidates
        .iter()
        .filter_map(|(gid, title, jpn)| {
            let score = score_candidate(local_title, jpn, title);
            (score >= MATCH_THRESHOLD).then(|| MatchHit {
                gid: gid.clone(),
                title: title.clone(),
                title_jpn: jpn.clone(),
                score,
            })
        })
        .collect();

    if hits.is_empty() {
        return MatchDecision::Unmatched;
    }
    hits.sort_by(|x, y| y.score.partial_cmp(&x.score).unwrap_or(std::cmp::Ordering::Equal));

    // 保护 1：同系列不同卷 → 交人工确认，不自动采纳
    let conflicting: Vec<MatchHit> = hits
        .iter()
        .filter(|h| volume_conflict(&local_norm, &normalize_title(&h.title_jpn)))
        .cloned()
        .collect();
    if !conflicting.is_empty() {
        return MatchDecision::Ambiguous(conflicting);
    }

    // 保护 2：最高分与次高分接近，且归一化标题不同（= 两个不同作品）
    if hits.len() >= 2 {
        let top_norm = normalize_title(&hits[0].title_jpn);
        let second = &hits[1];
        let second_norm = normalize_title(&second.title_jpn);
        if second_norm != top_norm && hits[0].score - second.score < TIE_MARGIN {
            return MatchDecision::Ambiguous(hits);
        }
    }

    // 归一化标题一致的多个候选 = 同一作品的不同版本/语言
    let top_norm = normalize_title(&hits[0].title_jpn);
    let same_work: Vec<MatchHit> = hits
        .iter()
        .filter(|h| normalize_title(&h.title_jpn) == top_norm)
        .cloned()
        .collect();
    if same_work.len() > 1 {
        MatchDecision::Editions(same_work)
    } else {
        MatchDecision::Matched(hits.remove(0))
    }
}

/// 生成搜索锚点（**按顺序尝试**）：作品名优先，其次创作者名。
///
/// 依据：真实语料 `work_title` 覆盖 100%、`creators` 仅 41%；且实测作者锚点对冷门作品
/// 召回不足（`いっぱいわけてね` 用两种锚点都搜不到），所以先作品名。
pub fn search_anchors(work_title: &str, creators: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let work = strip_markers(work_title);
    if !work.is_empty() {
        out.push(work);
    }
    for c in creators {
        let name = strip_markers(c);
        if !name.is_empty() && !out.iter().any(|x| x.eq_ignore_ascii_case(&name)) {
            out.push(name);
        }
    }
    out
}

/// 去掉 `[..]` / `(..)` 标记块，保留其余原文（搜索锚点用原文，不做小写与去符号）。
fn strip_markers(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let (mut ds, mut dr) = (0i32, 0i32);
    for c in s.chars() {
        match c {
            '[' | '【' => ds += 1,
            ']' | '】' => ds = (ds - 1).max(0),
            '(' | '（' => dr += 1,
            ')' | '）' => dr = (dr - 1).max(0),
            _ if ds > 0 || dr > 0 => {}
            _ => out.push(c),
        }
    }
    // 去掉常见的语言/版本标记后再收尾
    let mut cleaned = out;
    for marker in ["[中国翻訳]", "[無修正]", "[DL版]", "[Digital]"] {
        cleaned = cleaned.replace(marker, " ");
    }
    cleaned.split_whitespace().collect::<Vec<_>>().join(" ").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(gid: &str, jpn: &str, romaji: &str) -> (String, String, String) {
        (gid.into(), romaji.into(), jpn.into())
    }

    #[test]
    fn normalization_strips_markers_and_keeps_cjk() {
        // 注意：`to_lowercase()` 只影响拉丁字母，片假名不做假名转换
        assert_eq!(normalize_title("[KAROMIX (karory)] 清楚ビッチな巫女先輩"), "清楚ビッチな巫女先輩");
        assert_eq!(normalize_title("[デジタル] Full Color Vol.2"), "fullcolorvol2");
        assert_eq!(normalize_title(""), "");
    }

    #[test]
    fn dice_is_symmetric_and_bounded() {
        let a = normalize_title("人生リサイクル");
        assert!((dice(&a, &a) - 1.0).abs() < 1e-9, "自比应为 1");
        assert_eq!(dice(&a, ""), 0.0);
        assert!((dice(&a, &normalize_title("全然違う作品")) - 0.0).abs() < 1.0);
    }

    /// 回归：调研里 7 个真实样本的命中/不命中（候选标题取自实测搜索结果）。
    #[test]
    fn real_world_samples_match_or_fall_back() {
        // 1) 清楚ビッチな巫女先輩1 ← [KAROMIX (karory)] 清楚ビッチな巫女先輩（实测 0.95）
        let d = decide(
            "清楚ビッチな巫女先輩1",
            &[hit("1", "[KAROMIX (karory)] 清楚ビッチな巫女先輩 [中国翻訳] [無修正]", "")],
        );
        assert!(matches!(d, MatchDecision::Matched(_)), "应唯一命中，实得 {d:?}");

        // 2) ヒミツの睡眠学習 ← 同名（实测 1.00）
        let d = decide(
            "ヒミツの睡眠学習",
            &[hit("2", "[Bicolor (黒白音子)] ヒミツの睡眠学習 [ドイツ翻訳] [無修正] [DL版]", "")],
        );
        assert!(matches!(d, MatchDecision::Matched(_)));

        // 3) 人生リサイクル ← 同名（实测 1.00）
        let d = decide(
            "人生リサイクル",
            &[hit("3", "[Fatalpulse (朝凪)] 人生リサイクル [中国翻訳] [無修正] [DL版]", "")],
        );
        assert!(matches!(d, MatchDecision::Matched(_)));

        // 4) 同系列不同卷：孕ませ屋2 vs 孕ませ屋4（实测 0.75，高于阈值）→ 必须交人工确认
        let d = decide(
            "孕ませ屋2",
            &[hit("4", "[Digital Lover (なかじまゆか)] 孕ませ屋4 [中国翻訳]", "")],
        );
        match d {
            MatchDecision::Ambiguous(v) => assert_eq!(v.len(), 1),
            other => panic!("同系列不同卷必须判 Ambiguous，实得 {other:?}"),
        }

        // 5) 完全不相关的候选 → 未命中（实测 0.00）
        let d = decide(
            "いっぱいわけてね",
            &[hit("5", "[Bicolor (黒白音子)] ヒミツの睡眠学習 [ドイツ翻訳]", "")],
        );
        assert_eq!(d, MatchDecision::Unmatched);
    }

    #[test]
    fn same_work_multiple_editions_are_grouped() {
        // 同一作品的中文/韩文/日文版本：归一化标题一致 → Editions，而不是"并列接近"
        let d = decide(
            "人生リサイクル",
            &[
                hit("a", "[Fatalpulse (朝凪)] 人生リサイクル [中国翻訳] [無修正]", ""),
                hit("b", "[Fatalpulse (朝凪)] 人生リサイクル [韓国翻訳] [無修正]", ""),
                hit("c", "[Fatalpulse (朝凪)] 人生リサイクル [日本語]", ""),
            ],
        );
        match d {
            MatchDecision::Editions(v) => assert_eq!(v.len(), 3),
            other => panic!("应归并为同一作品的多个版本，实得 {other:?}"),
        }
    }

    #[test]
    fn near_tie_between_different_works_is_ambiguous() {
        // 上下卷：与本地名"孕ませ屋"的相似度相同（同分），但归一化标题不同 → 不能擅自挑一个
        let d = decide(
            "孕ませ屋",
            &[
                hit("a", "孕ませ屋 総集編 上", ""),
                hit("b", "孕ませ屋 総集編 下", ""),
            ],
        );
        match d {
            MatchDecision::Ambiguous(v) => assert_eq!(v.len(), 2, "两个接近候选都应列出待确认"),
            other => panic!("并列接近时必须判 Ambiguous，实得 {other:?}"),
        }
    }

    #[test]
    fn clearly_better_candidate_wins_even_if_others_pass_threshold() {
        // 明显更优（分差 > TIE_MARGIN）时可以直接采纳最高分
        let d = decide(
            "人生リサイクル",
            &[
                hit("best", "[Fatalpulse (朝凪)] 人生リサイクル [中国翻訳] [無修正]", ""),
                hit("worse", "[Fatalpulse (朝凪)] 人生リサイクル 総集編 完全版 限定", ""),
            ],
        );
        match d {
            MatchDecision::Matched(h) => assert_eq!(h.gid, "best"),
            other => panic!("分差明显时应采纳最高分，实得 {other:?}"),
        }
    }

    #[test]
    fn anchors_prefer_work_title_then_creators() {
        let a = search_anchors(
            "[赤月屋 (赤月みゅうと)] 僕にしか触れないサキュバス三姉妹に搾られる話4 [中国翻訳]",
            &["赤月みゅうと".into(), "赤月屋".into()],
        );
        assert_eq!(a[0], "僕にしか触れないサキュバス三姉妹に搾られる話4", "首个锚点应为作品名");
        assert!(a.contains(&"赤月みゅうと".to_string()));
        assert!(a.contains(&"赤月屋".to_string()));
        // 去重：重复创作者名只出现一次
        let a2 = search_anchors("T", &["x".into(), "x".into()]);
        assert_eq!(a2, vec!["T".to_string(), "x".to_string()]);
    }

    #[test]
    fn empty_inputs_are_unmatched_not_panic() {
        assert_eq!(decide("", &[]), MatchDecision::Unmatched);
        assert_eq!(decide("x", &[]), MatchDecision::Unmatched);
    }
}
