//! E 站元数据 → 本地标签/元数据的**导入规划器（影子模式）**。
//!
//! 本模块**只计算、不写入**：给定"本地现状快照 + E 站语义层 + 匹配结论"，产出
//! [`ImportPlan`]（将新增的标签、将填补的空白字段、以及被跳过项及原因），
//! 供界面先展示给人确认，再决定是否落库。
//!
//! 规则（用户 2026-09-22 确认的方案 B）：
//! * **命名空间前缀**：`源:e站`、`女性:巨乳`、`作者:朝凪`、`原作:…` —— 沿用项目既有
//!   前缀命名约定（`TagRepository.isVisibleInTagManager` 已识别 `resource:` / `sequence:` 等），
//!   因此**零表结构变更**，且"按前缀清除"可直接复用 `removeBookTagsByPrefix`。
//! * **只增不覆盖**：`author` / `series` / `summary` 只填**空白**；已有值一律跳过并记录原因。
//! * **不许猜**：`Ambiguous` / `Unmatched` 不产出任何写入项。
//! * **可解释**：所有跳过项都带原因（已存在什么、为什么不动）。

use serde::{Deserialize, Serialize};

use crate::eh_match::MatchDecision;
use crate::eh_subscription::EhSemantic;

/// E 站来源标记（置顶一行、点击可隐藏该书导入标签的锚点）。
pub const SOURCE_TAG: &str = "源:e站";

/// 本地现状快照（由调用方从 LibraryStore/TagRepository 取出后传入，
/// 保持本模块纯净、可离线测试）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BookSnapshot {
    pub author: String,
    pub series: String,
    pub summary: String,
    /// 该书当前已有的标签名（含前缀形式）。
    pub tags: Vec<String>,
}

/// 将新增的标签，带来源与中文名便于界面展示。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlannedTag {
    /// 实际写入的标签名（带前缀，如 `女性:巨乳`）。
    pub name: String,
    /// 原始命名空间（`female` / `artist` …）。
    pub namespace: String,
    /// 原始英文标签（无前空间则为空）。
    pub raw: String,
}

/// 将填补的字段。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlannedField {
    pub field: String,
    pub value: String,
}

/// 被跳过的项及原因（可解释性要求）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkippedItem {
    pub what: String,
    pub reason: String,
}

/// 导入计划（影子模式产物）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ImportPlan {
    /// `matched` | `editions` | `ambiguous` | `unmatched`
    pub status: String,
    pub gid: Option<String>,
    pub title_jpn: Option<String>,
    pub score: Option<f64>,
    pub tags: Vec<PlannedTag>,
    pub fields: Vec<PlannedField>,
    pub skipped: Vec<SkippedItem>,
}

impl ImportPlan {
    fn empty(status: &str, reason: &str) -> Self {
        Self {
            status: status.into(),
            gid: None,
            title_jpn: None,
            score: None,
            tags: Vec::new(),
            fields: Vec::new(),
            skipped: vec![SkippedItem { what: "整个导入".into(), reason: reason.into() }],
        }
    }

    /// 是否有实际写入项。
    pub fn has_changes(&self) -> bool {
        !self.tags.is_empty() || !self.fields.is_empty()
    }
}

/// 命名空间 → 中文前缀（用户可见的分类名）。
pub fn namespace_prefix(ns: &str) -> &'static str {
    match ns {
        "female" => "女性",
        "male" => "男性",
        "mixed" => "混合",
        "other" => "属性",
        "reclass" => "重分类",
        "language" => "语言",
        "artist" => "作者",
        "group" => "社团",
        "parody" => "原作",
        "character" => "角色",
        _ => "其他",
    }
}

/// 规划导入：给定本地现状与 E 站语义层，产出"将要写入什么"。
pub fn plan_import(snapshot: &BookSnapshot, semantic: &EhSemantic, decision: &MatchDecision) -> ImportPlan {
    let (status, hit) = match decision {
        MatchDecision::Matched(h) => ("matched", Some(h)),
        MatchDecision::Editions(v) => ("editions", v.first()),
        MatchDecision::Ambiguous(_) => {
            return ImportPlan::empty("ambiguous", "多个候选接近或疑似同系列不同卷，需人工确认")
        }
        MatchDecision::Unmatched => {
            return ImportPlan::empty("unmatched", "未找到相似度达标的画廊，不猜")
        }
    };
    let Some(hit) = hit else {
        return ImportPlan::empty(status, "匹配结果为空");
    };

    let existing: Vec<String> = snapshot.tags.iter().map(|t| t.trim().to_string()).collect();
    let mut plan = ImportPlan {
        status: status.into(),
        gid: Some(hit.gid.clone()),
        title_jpn: Some(hit.title_jpn.clone()),
        score: Some(hit.score),
        tags: Vec::new(),
        fields: Vec::new(),
        skipped: Vec::new(),
    };

    // 1) 来源标记（置顶那一行的锚点）
    if existing.iter().any(|t| t == SOURCE_TAG) {
        plan.skipped.push(SkippedItem {
            what: SOURCE_TAG.into(),
            reason: "已存在来源标记".into(),
        });
    } else {
        plan.tags.push(PlannedTag {
            name: SOURCE_TAG.into(),
            namespace: "source".into(),
            raw: String::new(),
        });
    }

    // 2) 语义层标签：中文优先，缺译名退回原始值
    let mut push_tag = |ns: &str, raw: &str, zh: &str, plan: &mut ImportPlan| {
        let display = if zh.trim().is_empty() { raw.trim() } else { zh.trim() };
        if display.is_empty() {
            return;
        }
        let name = format!("{}:{}", namespace_prefix(ns), display);
        if existing.iter().any(|t| t == &name) {
            plan.skipped.push(SkippedItem { what: name, reason: "标签已存在".into() });
            return;
        }
        if plan.tags.iter().any(|t| t.name == name) {
            return; // 同一计划内去重
        }
        plan.tags.push(PlannedTag { name, namespace: ns.into(), raw: raw.into() });
    };

    // creators（artist / group）
    for c in &semantic.creators {
        push_tag(&c.role, &c.name, "", &mut plan);
    }
    // 原作 / 角色
    for s in &semantic.source_series {
        push_tag("parody", s, "", &mut plan);
    }
    for ch in &semantic.characters {
        push_tag("character", ch, "", &mut plan);
    }
    // 内容/资源标签（与中文译名下标对齐）
    for (i, tag) in semantic.resource_tags.iter().enumerate() {
        let (ns, raw) = match tag.split_once(':') {
            Some((a, b)) => (a, b),
            None => ("", tag.as_str()),
        };
        let zh = semantic.resource_tags_zh.get(i).map(String::as_str).unwrap_or("");
        push_tag(ns, raw, zh, &mut plan);
    }

    // 3) 只填空白字段
    let creators_joined = semantic
        .creators
        .iter()
        .filter(|c| c.role == "artist")
        .map(|c| c.name.trim())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("、");
    let creator_value = if creators_joined.is_empty() {
        semantic
            .creators
            .iter()
            .map(|c| c.name.trim())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("、")
    } else {
        creators_joined
    };

    if !creator_value.is_empty() {
        if snapshot.author.trim().is_empty() {
            plan.fields.push(PlannedField { field: "author".into(), value: creator_value });
        } else {
            plan.skipped.push(SkippedItem {
                what: "author".into(),
                reason: format!("已有作者「{}」，不覆盖", snapshot.author.trim()),
            });
        }
    }
    if let Some(series) = semantic.source_series.first().filter(|s| !s.trim().is_empty()) {
        if snapshot.series.trim().is_empty() {
            plan.fields.push(PlannedField { field: "series".into(), value: series.trim().into() });
        } else {
            plan.skipped.push(SkippedItem {
                what: "series".into(),
                reason: format!("已有系列「{}」，不覆盖", snapshot.series.trim()),
            });
        }
    }

    plan
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eh_match::MatchHit;

    fn hit(gid: &str, jpn: &str) -> MatchHit {
        MatchHit { gid: gid.into(), title: String::new(), title_jpn: jpn.into(), score: 0.92 }
    }

    fn semantic() -> EhSemantic {
        EhSemantic {
            work_title: "人生リサイクル".into(),
            creators: vec![
                crate::eh_subscription::EhCreator { role: "artist".into(), name: "朝凪".into() },
                crate::eh_subscription::EhCreator { role: "group".into(), name: "Fatalpulse".into() },
            ],
            source_series: vec!["original".into()],
            resource_tags: vec![
                "female:big breasts".into(),
                "male:bondage".into(),
                "other:multi-work series".into(),
            ],
            resource_tags_zh: vec!["巨乳".into(), "束缚".into(), String::new()],
            censorship: Some("uncensored".into()),
            ..Default::default()
        }
    }

    #[test]
    fn plans_prefixed_tags_with_chinese_names() {
        let plan = plan_import(&BookSnapshot::default(), &semantic(), &MatchDecision::Matched(hit("1", "T")));
        assert_eq!(plan.status, "matched");
        assert!(plan.has_changes());
        let names: Vec<&str> = plan.tags.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&SOURCE_TAG), "应置入来源标记");
        assert!(names.contains(&"作者:朝凪"), "artist → 作者 前缀");
        assert!(names.contains(&"社团:Fatalpulse"), "group → 社团 前缀");
        assert!(names.contains(&"原作:original"));
        assert!(names.contains(&"女性:巨乳"), "中文译名优先");
        assert!(names.contains(&"男性:束缚"));
        // 缺译名 → 退回原始值，不产出空标签
        assert!(names.contains(&"属性:multi-work series"));
    }

    #[test]
    fn never_overwrites_existing_meta_and_reports_reason() {
        let snap = BookSnapshot {
            author: "已有作者".into(),
            series: "已有系列".into(),
            ..Default::default()
        };
        let plan = plan_import(&snap, &semantic(), &MatchDecision::Matched(hit("1", "T")));
        assert!(plan.fields.is_empty(), "已有值不应被覆盖");
        assert!(plan.skipped.iter().any(|s| s.what == "author" && s.reason.contains("不覆盖")));
        assert!(plan.skipped.iter().any(|s| s.what == "series"));
    }

    #[test]
    fn fills_only_blank_fields() {
        let snap = BookSnapshot { author: "".into(), series: "".into(), ..Default::default() };
        let plan = plan_import(&snap, &semantic(), &MatchDecision::Matched(hit("1", "T")));
        let fields: Vec<(&str, &str)> =
            plan.fields.iter().map(|f| (f.field.as_str(), f.value.as_str())).collect();
        assert!(fields.contains(&("author", "朝凪")), "空白作者应被填补（取 artist）");
        assert!(fields.contains(&("series", "original")));
    }

    #[test]
    fn existing_tags_are_not_duplicated() {
        let snap = BookSnapshot {
            tags: vec![SOURCE_TAG.into(), "女性:巨乳".into()],
            ..Default::default()
        };
        let plan = plan_import(&snap, &semantic(), &MatchDecision::Matched(hit("1", "T")));
        assert!(!plan.tags.iter().any(|t| t.name == SOURCE_TAG));
        assert!(!plan.tags.iter().any(|t| t.name == "女性:巨乳"));
        assert!(plan.skipped.iter().any(|s| s.what == SOURCE_TAG));
    }

    #[test]
    fn ambiguous_and_unmatched_produce_no_writes() {
        for (decision, expect) in [
            (MatchDecision::Ambiguous(vec![hit("a", "x"), hit("b", "y")]), "ambiguous"),
            (MatchDecision::Unmatched, "unmatched"),
        ] {
            let plan = plan_import(&BookSnapshot::default(), &semantic(), &decision);
            assert_eq!(plan.status, expect);
            assert!(!plan.has_changes(), "不许猜：不产出任何写入项");
            assert!(!plan.skipped.is_empty(), "应给出原因");
        }
    }

    #[test]
    fn editions_plan_uses_highest_scoring() {
        let plan = plan_import(
            &BookSnapshot::default(),
            &semantic(),
            &MatchDecision::Editions(vec![hit("best", "T"), hit("other", "T2")]),
        );
        assert_eq!(plan.status, "editions");
        assert_eq!(plan.gid.as_deref(), Some("best"));
    }

    #[test]
    fn prefixes_cover_all_namespaces() {
        assert_eq!(namespace_prefix("female"), "女性");
        assert_eq!(namespace_prefix("male"), "男性");
        assert_eq!(namespace_prefix("other"), "属性");
        assert_eq!(namespace_prefix("language"), "语言");
        assert_eq!(namespace_prefix("artist"), "作者");
        assert_eq!(namespace_prefix("group"), "社团");
        assert_eq!(namespace_prefix("parody"), "原作");
        assert_eq!(namespace_prefix("character"), "角色");
        assert_eq!(namespace_prefix("unknown-ns"), "其他");
    }
}
