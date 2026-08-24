//! Catalog-only scraping primitives.
//!
//! The parser consumes persisted catalog text only. It does not accept a
//! ByteSource, downloader, source session or sync transport capability.

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CatalogSnapshot {
    pub book_key: String,
    pub filename: String,
    pub ancestor_dirs: Vec<String>,
    #[serde(default)]
    pub parent_siblings: Vec<String>,
}

impl CatalogSnapshot {
    pub fn new(
        book_key: impl Into<String>,
        filename: impl Into<String>,
        ancestors: Vec<String>,
    ) -> Self {
        Self {
            book_key: book_key.into(),
            filename: filename.into(),
            ancestor_dirs: ancestors,
            parent_siblings: Vec::new(),
        }
    }

    pub fn with_context(
        filename: impl Into<String>,
        ancestors: Vec<&str>,
        siblings: Vec<&str>,
    ) -> Self {
        Self {
            book_key: "fixture".into(),
            filename: filename.into(),
            ancestor_dirs: ancestors.into_iter().map(str::to_owned).collect(),
            parent_siblings: siblings.into_iter().map(str::to_owned).collect(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum ParseState {
    Ready,
    Partial,
    Ambiguous,
    Unmatched,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RoleEvidence {
    pub role: String,
    pub value: String,
    pub source: String,
    pub rule: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RoleConflict {
    pub roles: Vec<String>,
    pub value: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CreatorCandidate {
    pub name: String,
    pub role: String,
    pub alias_of: Option<String>,
}

/// A leading attribution token that the offline parser intentionally leaves
/// unresolved.  It is retained for review without projecting the token into
/// creator/provider fields.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AttributionCandidate {
    pub name: String,
    pub possible_roles: Vec<String>,
    pub source: String,
}

/// A numeric token that the offline grammar deliberately did not promote to a
/// chapter/issue/volume.  Keeping it structured prevents the title cleaner
/// from silently dropping meaningful labels such as `NO.41` or `(1)`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct NumericLabelCandidate {
    pub prefix: String,
    pub value: String,
    pub semantic_role: String,
    pub raw: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ExternalIdCandidate {
    pub namespace_hint: String,
    pub raw: String,
}

/// Structured ordering data for chapter/episode sequences.
///
/// `minor` is used for lettered subdivisions such as `153a`/`153b` and
/// `relation_rank` orders front/continuation/back parts without inventing a
/// fractional chapter number.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ChapterOrderKey {
    pub major: u32,
    pub minor: Option<u32>,
    pub minor_scale: Option<u8>,
    pub relation_rank: i16,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct NameRoleProposal {
    // Compatibility projections used by the current review UI. `title` is an
    // alias of `work_title`; `edition` is an alias of `resource_edition`.
    pub title: Option<String>,
    pub authors: Vec<String>,
    pub provider: Option<String>,
    pub volume: Option<String>,
    pub chapter: Option<String>,

    // v3 semantic projections.
    pub work_title: Option<String>,
    pub publication_title_raw: Option<String>,
    pub title_aliases: Vec<String>,
    pub creators: Vec<CreatorCandidate>,
    pub attribution_candidates: Vec<AttributionCandidate>,
    pub numeric_labels: Vec<NumericLabelCandidate>,
    pub source_series: Vec<String>,
    /// Trailing parenthetical text that is useful evidence but not strong
    /// enough for the offline parser to assert as a franchise/source series.
    pub source_context_candidates: Vec<String>,
    pub external_id_candidates: Vec<ExternalIdCandidate>,
    pub publisher: Option<String>,
    pub publication_source: Option<String>,
    pub release_event: Option<String>,
    pub publication_year: Option<String>,
    pub edition: Option<String>,
    pub resource_edition: Option<String>,
    pub distribution_platform: Option<String>,
    pub sequence_kind: Option<String>,
    pub issue: Option<String>,
    pub season: Option<String>,
    pub season_range: Option<String>,
    pub chapter_range: Option<String>,
    pub part: Option<i64>,
    pub range: Option<String>,
    pub sequence_members: Vec<String>,
    pub includes_special: bool,
    pub is_collection: bool,
    pub sequence_label: Option<String>,
    pub special_title: Option<String>,
    pub chapter_title: Option<String>,
    pub chapter_relation: Option<String>,
    pub sort_key: Option<ChapterOrderKey>,
    pub resource_language: Option<String>,
    pub translation_state: Option<String>,
    pub translation_method: Option<String>,
    pub source_medium: Option<String>,
    pub scan_completeness: Option<String>,
    pub resource_completeness: Option<String>,
    pub censorship: Option<String>,
    pub color_state: Option<String>,
    pub release_groups: Vec<String>,
    pub resource_tags: Vec<String>,
    pub evidence: Vec<RoleEvidence>,
    pub conflicts: Vec<RoleConflict>,
    pub warnings: Vec<String>,
    pub state: ParseState,
    pub rule_version: String,
}

impl NameRoleProposal {
    fn new(rule_version: &str) -> Self {
        Self {
            title: None,
            authors: Vec::new(),
            provider: None,
            volume: None,
            chapter: None,
            work_title: None,
            publication_title_raw: None,
            title_aliases: Vec::new(),
            creators: Vec::new(),
            attribution_candidates: Vec::new(),
            numeric_labels: Vec::new(),
            source_series: Vec::new(),
            source_context_candidates: Vec::new(),
            external_id_candidates: Vec::new(),
            publisher: None,
            publication_source: None,
            release_event: None,
            publication_year: None,
            edition: None,
            resource_edition: None,
            distribution_platform: None,
            sequence_kind: None,
            issue: None,
            season: None,
            season_range: None,
            chapter_range: None,
            part: None,
            range: None,
            sequence_members: Vec::new(),
            includes_special: false,
            is_collection: false,
            sequence_label: None,
            special_title: None,
            chapter_title: None,
            chapter_relation: None,
            sort_key: None,
            resource_language: None,
            translation_state: None,
            translation_method: None,
            source_medium: None,
            scan_completeness: None,
            resource_completeness: None,
            censorship: None,
            color_state: None,
            release_groups: Vec::new(),
            resource_tags: Vec::new(),
            evidence: Vec::new(),
            conflicts: Vec::new(),
            warnings: Vec::new(),
            state: ParseState::Unmatched,
            rule_version: rule_version.to_owned(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Delimiter {
    Square,
    Round,
    Curly,
    FullWidth,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BracketGroup {
    content: String,
    delimiter: Delimiter,
    start: usize,
    end: usize,
    leading: bool,
}

#[derive(Debug, Clone, Default)]
struct SequenceData {
    kind: Option<String>,
    volume: Option<String>,
    chapter: Option<String>,
    issue: Option<String>,
    season: Option<String>,
    season_range: Option<String>,
    chapter_range: Option<String>,
    part: Option<i64>,
    range: Option<String>,
    sequence_members: Vec<String>,
    includes_special: bool,
    is_collection: bool,
    sequence_label: Option<String>,
    chapter_title: Option<String>,
    relation: Option<String>,
    sort_key: Option<ChapterOrderKey>,
    weak_terminal_number: bool,
    weak_terminal_span: Option<(usize, usize)>,
    spans: Vec<(usize, usize)>,
}

#[derive(Debug, Clone)]
struct CompositeSequence {
    start: usize,
    end: usize,
    members: Vec<String>,
    range: Option<String>,
    includes_special: bool,
}

/// Parse a catalog snapshot without opening a file, source adapter or remote session.
pub fn parse_catalog(
    snapshot: &CatalogSnapshot,
    ancestor_depth: usize,
    rule_version: &str,
) -> NameRoleProposal {
    let mut proposal = NameRoleProposal::new(rule_version);
    let decoded_filename = decode_html_entities(&snapshot.filename);
    let decoded_filename = catalog_basename(&decoded_filename);
    let stem = strip_extension(&decoded_filename);
    let (groups, mut core) = extract_groups(&stem);

    for group in &groups {
        classify_group(group, &mut proposal);
    }
    if groups
        .iter()
        .any(|group| is_sequence_label(group.content.trim()))
    {
        core = core
            .replace(" + ", " ")
            .replace("+ ", " ")
            .replace(" +", " ");
    }

    strip_timestamp_suffix(&mut core, &mut proposal);
    classify_inline_resource_terms(&core, &mut proposal);
    detect_unresolved_numeric_labels(&core, &mut proposal);

    let publication_core = core.clone();
    let ambiguous_numeric_identifier = is_ambiguous_numeric_identifier(&publication_core, snapshot);
    if ambiguous_numeric_identifier {
        proposal.external_id_candidates.push(ExternalIdCandidate {
            namespace_hint: "unknown".into(),
            raw: publication_core.trim().into(),
        });
        push_evidence(
            &mut proposal,
            "external_id",
            publication_core.trim(),
            "filename",
            "ambiguous-numeric-identifier",
        );
    }
    let mut sequence = if ambiguous_numeric_identifier {
        SequenceData::default()
    } else {
        extract_sequence(&core, &mut proposal)
    };
    let depth = ancestor_depth.min(snapshot.ancestor_dirs.len());
    let ancestors = &snapshot.ancestor_dirs[..depth];
    let normalized_ancestors = ancestors
        .iter()
        .enumerate()
        .map(|(index, ancestor)| {
            normalize_ancestor_title_candidate(ancestor, index, &mut proposal)
        })
        .collect::<Vec<_>>();
    adjust_sequence_from_context(&mut sequence, &core, ancestors, snapshot, &mut proposal);
    apply_sequence(&sequence, &mut proposal);
    let sibling_context = sibling_context(snapshot);
    if !sibling_context.is_empty() && sequence.kind.is_some() {
        push_evidence(
            &mut proposal,
            "sequence_context",
            &sibling_context.join(" | "),
            "siblings",
            "exclude-current-file",
        );
    }
    core = remove_spans(&core, &sequence.spans);
    if !clean_title_text(&core).is_empty() {
        proposal.publication_title_raw = Some(normalize_publication_title_raw(&publication_core));
    }

    // A bounded tankobon range is a useful local completeness signal. It is
    // still resource-level metadata; it must never be projected to work
    // completion.
    if proposal.resource_completeness.is_none()
        && sequence.range.is_some()
        && contains_any(&core, &["单行本", "単行本", "tankobon"])
    {
        proposal.resource_completeness = Some("complete".into());
        push_tag(&mut proposal, "complete");
        push_evidence(
            &mut proposal,
            "resource_completeness",
            "bounded-tankobon-range",
            "filename",
            "bounded-tankobon-range",
        );
    }

    let raw_core = core.clone();
    let (title_source, explicit_file_author): (&str, Option<String>) =
        if proposal.creators.is_empty() {
            if let Some((left, right)) = split_author_title(&raw_core) {
                push_creator(
                    &mut proposal,
                    left,
                    "artist",
                    "filename",
                    "author-title-separator",
                );
                (right, Some(left.to_owned()))
            } else {
                (raw_core.as_str(), None)
            }
        } else {
            // Leading creator grammar has already established the attribution
            // namespace. Do not reinterpret a title's internal hyphen as another
            // artist/provider separator.
            (raw_core.as_str(), None)
        };

    let (core, bilingual_aliases) = split_bilingual_title(title_source, &mut proposal);
    if !bilingual_aliases.is_empty() {
        for alias in bilingual_aliases {
            if !proposal
                .title_aliases
                .iter()
                .any(|existing| normalize_for_compare(existing) == normalize_for_compare(&alias))
            {
                push_evidence(
                    &mut proposal,
                    "title_alias",
                    &alias,
                    "filename-residual",
                    "bilingual-title-separator",
                );
                proposal.title_aliases.push(alias);
            }
        }
    }

    let (ancestor_title, title_index) = choose_ancestor_title(&core, &normalized_ancestors);

    let special_sequence = proposal.sequence_kind.as_deref() == Some("special")
        && proposal.chapter_relation.as_deref() == Some("side_story");
    let special_with_ancestor = special_sequence && ancestor_title.is_some();
    if special_with_ancestor && !core.is_empty() && !is_structural_only(&core) {
        let special_title = ancestor_title
            .as_deref()
            .map(|ancestor| strip_special_work_prefix(&core, ancestor))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| core.clone());
        proposal.special_title = Some(special_title.clone());
        push_evidence(
            &mut proposal,
            "special_title",
            &special_title,
            "filename-residual",
            "special-residual-title",
        );
    }

    if !special_with_ancestor && !core.is_empty() && !is_structural_only(&core) {
        proposal.title = Some(core.clone());
        push_evidence(
            &mut proposal,
            "title",
            &core,
            "filename",
            "filename-residual",
        );
    }
    if let Some(ancestor) = ancestor_title {
        if special_with_ancestor {
            proposal.title = Some(ancestor.clone());
            push_evidence(
                &mut proposal,
                "title",
                &ancestor,
                &format!("ancestor({})", title_index.unwrap_or(0)),
                "special-ancestor-work-title",
            );
        }
        let core_agrees = !core.is_empty()
            && (normalize_for_compare(&core) == normalize_for_compare(&ancestor)
                || normalize_for_compare(&core).contains(&normalize_for_compare(&ancestor))
                || normalize_for_compare(&ancestor).contains(&normalize_for_compare(&core)));
        let alias_candidate = clean_title_text(&core);
        let prefer_ancestor_for_alias = !core.is_empty()
            && !core_agrees
            && !alias_candidate.is_empty()
            && !special_with_ancestor
            && looks_like_romanized_alias(&alias_candidate, &ancestor);
        if prefer_ancestor_for_alias
            && !proposal.title_aliases.iter().any(|alias| {
                normalize_for_compare(alias) == normalize_for_compare(&alias_candidate)
            })
        {
            proposal.title_aliases.push(alias_candidate.clone());
            push_evidence(
                &mut proposal,
                "title_alias",
                &alias_candidate,
                "filename-residual",
                "ancestor-title-reconciles-romanized-alias",
            );
        }
        if !special_with_ancestor
            && (proposal.title.is_none()
                || core.is_empty()
                || core_agrees
                || prefer_ancestor_for_alias)
        {
            proposal.title = Some(ancestor.clone());
            push_evidence(
                &mut proposal,
                "title",
                &ancestor,
                &format!("ancestor({})", title_index.unwrap_or(0)),
                if core.is_empty() {
                    "ancestor-context"
                } else {
                    "filename-ancestor-agreement"
                },
            );
        }
    }

    collect_ancestor_creators(
        &mut proposal,
        &normalized_ancestors,
        title_index,
        &snapshot.filename,
    );
    if let Some(explicit) = explicit_file_author {
        let normalized = normalize_for_compare(&explicit);
        if proposal
            .creators
            .iter()
            .any(|creator| normalize_for_compare(&creator.name) != normalized)
        {
            proposal.conflicts.push(RoleConflict {
                roles: vec!["creator".into()],
                value: explicit,
                reason: "filename and ancestor creator candidates disagree".into(),
            });
        }
    }

    if let Some(provider) = proposal.provider.clone() {
        let normalized_provider = normalize_for_compare(&provider);
        let overlapping: Vec<String> = proposal
            .creators
            .iter()
            .filter(|creator| normalize_for_compare(&creator.name) == normalized_provider)
            .map(|creator| creator.name.clone())
            .collect();
        for value in overlapping {
            proposal.conflicts.push(RoleConflict {
                roles: vec!["provider".into(), "creator".into()],
                value,
                reason: "provider/platform cannot also be a creator".into(),
            });
        }
        proposal
            .creators
            .retain(|creator| normalize_for_compare(&creator.name) != normalized_provider);
    }

    proposal.authors = proposal
        .creators
        .iter()
        .filter(|creator| matches!(creator.role.as_str(), "author" | "artist" | "writer"))
        .map(|creator| creator.name.clone())
        .collect();
    proposal.work_title = proposal.title.clone();
    proposal.edition = proposal.resource_edition.clone();

    let has_evidence = proposal.title.is_some()
        || !proposal.creators.is_empty()
        || proposal.provider.is_some()
        || proposal.chapter.is_some()
        || proposal.volume.is_some()
        || proposal.chapter_title.is_some()
        || proposal.sequence_kind.is_some()
        || proposal.part.is_some()
        || proposal.sequence_label.is_some()
        || proposal.special_title.is_some()
        || !proposal.sequence_members.is_empty()
        || !proposal.numeric_labels.is_empty()
        || !proposal.title_aliases.is_empty()
        || !proposal.source_series.is_empty()
        || !proposal.source_context_candidates.is_empty()
        || !proposal.external_id_candidates.is_empty()
        || !proposal.resource_tags.is_empty();
    proposal.state = if !proposal.conflicts.is_empty() || ambiguous_numeric_identifier {
        ParseState::Ambiguous
    } else if proposal.title.is_some() {
        ParseState::Ready
    } else if has_evidence {
        ParseState::Partial
    } else {
        ParseState::Unmatched
    };
    proposal
}

fn classify_group(group: &BracketGroup, proposal: &mut NameRoleProposal) {
    let value = group.content.trim();
    if value.is_empty() {
        return;
    }
    if classify_full_color_label(value, proposal) {
        return;
    }
    if classify_resource_label(value, proposal) {
        return;
    }
    if apply_sequence_label(value, proposal) {
        return;
    }
    if let Some(candidate) = parse_external_id(value) {
        proposal.external_id_candidates.push(candidate);
        push_evidence(
            proposal,
            "external_id",
            value,
            "filename-bracket",
            "external-id-namespace",
        );
        return;
    }
    if is_release_event(value) {
        set_once(
            &mut proposal.release_event,
            value,
            &mut proposal.evidence,
            "release_event",
            "filename-bracket",
            "release-event-code",
        );
        return;
    }
    if is_year(value) {
        set_once(
            &mut proposal.publication_year,
            value,
            &mut proposal.evidence,
            "publication_year",
            "filename-bracket",
            "publication-year",
        );
        return;
    }
    if let Some(page_count) = parse_page_count(value) {
        push_tag(proposal, "page_count");
        push_evidence(
            proposal,
            "page_count",
            &page_count,
            "filename-parenthesis",
            "page-count-parenthesis",
        );
        return;
    }
    if let Some(range) = parse_of_range(value) {
        set_once(
            &mut proposal.range,
            &range,
            &mut proposal.evidence,
            "range",
            "filename-bracket",
            "issue-total-range",
        );
        return;
    }
    if value.to_ascii_lowercase().contains("cover") {
        push_tag(proposal, "cover-variant");
        push_evidence(
            proposal,
            "resource",
            value,
            "filename-bracket",
            "cover-variant",
        );
        return;
    }
    if is_publication_source(value) {
        set_once(
            &mut proposal.publication_source,
            value,
            &mut proposal.evidence,
            "publication_source",
            "filename-bracket",
            "publication-source-shape",
        );
        return;
    }
    if let Some((part, relation)) = parenthetical_part(value) {
        proposal.sequence_kind.get_or_insert_with(|| "part".into());
        proposal.part = Some(part);
        proposal.sequence_label = Some(value.to_owned());
        proposal.chapter_relation = Some(relation.to_owned());
        push_evidence(
            proposal,
            "part",
            value,
            "filename-parenthesis",
            "parenthetical-part",
        );
        return;
    }
    if let Some(members) = parenthetical_sequence_members(value) {
        for member in members {
            if !proposal.sequence_members.iter().any(|item| item == &member) {
                proposal.sequence_members.push(member);
            }
        }
        push_evidence(
            proposal,
            "sequence_members",
            value,
            "filename-parenthesis",
            "composite-part-marker",
        );
        return;
    }
    // A nested leading creator expression is more specific than the generic
    // provider/platform lexicon. Parse it first so `[Circle (Artist)]` cannot
    // be downgraded to a release provider.
    if group.leading && group.delimiter == Delimiter::Square && parse_creator_group(value, proposal)
    {
        return;
    }
    if group.leading && group.delimiter == Delimiter::Square && is_platform_label(value) {
        add_attribution_candidate(proposal, value);
        push_evidence(
            proposal,
            "unknown_tag",
            value,
            "filename-bracket",
            "leading-platform-attribution",
        );
        return;
    }
    if is_numeric_range_token(value) {
        if let Some((from, to)) = parse_range(value) {
            let rendered = format!("{from}-{to}");
            set_once(
                &mut proposal.range,
                &rendered,
                &mut proposal.evidence,
                "range",
                "filename-bracket",
                "numeric-range",
            );
        }
        return;
    }
    if is_provider_label(value) || is_platform_label(value) {
        add_release_group(proposal, value);
        if is_platform_label(value) {
            set_once(
                &mut proposal.distribution_platform,
                value,
                &mut proposal.evidence,
                "distribution_platform",
                "filename-bracket",
                "platform-lexicon",
            );
        }
        return;
    }
    if group.leading && group.delimiter == Delimiter::Square {
        let lower = value.to_ascii_lowercase();
        if lower.ends_with("group") && !is_provider_label(value) {
            add_attribution_candidate(proposal, value);
            push_evidence(
                proposal,
                "unknown_tag",
                value,
                "filename-bracket",
                "unknown-leading-group",
            );
            return;
        }
        if is_title_alias_candidate(value) {
            let alias = clean_alias(value);
            if !alias.is_empty() {
                proposal.title_aliases.push(alias.clone());
                push_evidence(
                    proposal,
                    "title_alias",
                    &alias,
                    "filename-bracket",
                    "romanized-title-with-volume",
                );
            }
            return;
        }
        if is_release_group_marker(value) {
            add_release_group(proposal, value);
            return;
        }
        add_attribution_candidate(proposal, value);
        push_evidence(
            proposal,
            "unknown_tag",
            value,
            "filename-bracket",
            "unknown-leading-bracket",
        );
        return;
    }
    // An explicit volume/range marker is stronger evidence of a romanized
    // title alias than the generic hyphen/group release-label heuristic.
    if has_volume_marker(value) && is_title_alias_candidate(value) {
        let alias = clean_alias(value);
        if !alias.is_empty() {
            proposal.title_aliases.push(alias.clone());
            push_evidence(
                proposal,
                "title_alias",
                &alias,
                "filename-bracket",
                "romanized-title-with-volume",
            );
        }
        return;
    }
    if is_release_group_marker(value) {
        add_release_group(proposal, value);
        return;
    }
    if matches!(group.delimiter, Delimiter::Round | Delimiter::FullWidth) {
        if let Some(number) = pure_numeric_token(value) {
            add_numeric_label(
                proposal,
                "",
                &number,
                &number,
                "unresolved",
                "filename-parenthesis",
                "parenthetical-numeric-candidate",
            );
            return;
        }
    }
    if matches!(group.delimiter, Delimiter::Round | Delimiter::FullWidth) {
        if is_known_source_series(value) {
            proposal.source_series.push(value.to_owned());
            push_evidence(
                proposal,
                "source_series",
                value,
                "filename-parenthesis",
                "known-source-series",
            );
        } else {
            if !proposal
                .source_context_candidates
                .iter()
                .any(|candidate| candidate == value)
            {
                proposal.source_context_candidates.push(value.to_owned());
            }
            push_evidence(
                proposal,
                "source_context_candidate",
                value,
                "filename-parenthesis",
                "unresolved-parenthetical-context",
            );
            if !proposal
                .warnings
                .iter()
                .any(|warning| warning == "unresolved_parenthetical_context")
            {
                proposal
                    .warnings
                    .push("unresolved_parenthetical_context".into());
            }
        }
        return;
    }
    if is_title_alias_candidate(value) {
        let alias = clean_alias(value);
        if !alias.is_empty() {
            proposal.title_aliases.push(alias.clone());
            push_evidence(
                proposal,
                "title_alias",
                &alias,
                "filename-bracket",
                "title-alias-candidate",
            );
        }
    } else {
        push_tag(proposal, "unknown-tag");
        push_evidence(
            proposal,
            "unknown_tag",
            value,
            "filename-bracket",
            "unknown-bracket",
        );
    }
}

fn is_sequence_label(value: &str) -> bool {
    matches!(
        value.trim(),
        "前篇"
            | "前編"
            | "后篇"
            | "後篇"
            | "后编"
            | "後編"
            | "续篇"
            | "続編"
            | "番外篇"
            | "番外編"
            | "番外"
            | "外传"
            | "外伝"
            | "序章"
            | "终章"
            | "終章"
            | "幕间"
            | "幕間"
    )
}

fn apply_sequence_label(value: &str, proposal: &mut NameRoleProposal) -> bool {
    let (kind, relation, part) = match value.trim() {
        "前篇" | "前編" => ("part", "front_part", Some(1)),
        "后篇" | "後篇" | "后编" | "後編" => ("part", "back_part", Some(2)),
        "续篇" | "続編" => ("chapter", "continuation", None),
        "番外篇" | "番外編" | "番外" | "外传" | "外伝" => {
            ("special", "side_story", None)
        }
        "序章" => ("special", "prologue", None),
        "终章" | "終章" => ("special", "epilogue", None),
        "幕间" | "幕間" => ("special", "interlude", None),
        _ => return false,
    };
    proposal.sequence_kind = Some(kind.to_owned());
    if kind == "part" {
        let has_other_part = proposal
            .sequence_members
            .iter()
            .any(|member| member == "front_part" || member == "back_part");
        if !has_other_part {
            proposal.sequence_members.push(relation.to_owned());
            proposal.sequence_label = Some(value.trim().to_owned());
            proposal.chapter_relation = Some(relation.to_owned());
            proposal.part = part;
        } else if !proposal
            .sequence_members
            .iter()
            .any(|member| member == relation)
        {
            proposal.sequence_members.push(relation.to_owned());
            proposal.is_collection = true;
            proposal.part = None;
            proposal.sequence_label = None;
            proposal.chapter_relation = None;
        }
    } else {
        proposal.sequence_label = Some(value.trim().to_owned());
        proposal.chapter_relation = Some(relation.to_owned());
        proposal.part = part;
    }
    push_evidence(
        proposal,
        "sequence_label",
        value.trim(),
        "filename-bracket",
        "bracket-sequence-label",
    );
    true
}

fn parse_creator_group(value: &str, proposal: &mut NameRoleProposal) -> bool {
    if let Some(open) = value.find('(') {
        if value.ends_with(')') {
            let outer = value[..open].trim();
            let inner = value[open + 1..value.len() - 1].trim();
            if !outer.is_empty() && !inner.is_empty() {
                push_creator(
                    proposal,
                    outer,
                    "circle",
                    "filename-bracket",
                    "leading-circle",
                );
                push_creator(
                    proposal,
                    inner,
                    "artist",
                    "filename-bracket",
                    "nested-artist",
                );
                return true;
            }
        }
    }
    let lower = value.to_ascii_lowercase();
    if matches!(lower.as_str(), "artist" | "author" | "circle" | "writer") {
        let role = if lower == "circle" {
            "circle"
        } else {
            "artist"
        };
        push_creator(
            proposal,
            value,
            role,
            "filename-bracket",
            "explicit-creator-label",
        );
        return true;
    }
    false
}

fn classify_full_color_label(value: &str, proposal: &mut NameRoleProposal) -> bool {
    let lower = value.to_ascii_lowercase();
    if lower.contains("full color") || lower.contains("full-color") || value.contains("フルカラー")
    {
        proposal.color_state = Some("full_color".into());
        push_tag(proposal, "full_color");
        push_evidence(
            proposal,
            "color_state",
            value,
            "filename-bracket",
            "full-color-marker",
        );
        true
    } else {
        false
    }
}

fn classify_resource_label(value: &str, proposal: &mut NameRoleProposal) -> bool {
    if contains_collection_marker(value) {
        proposal.resource_completeness = Some("complete".into());
        proposal.is_collection = true;
        push_tag(proposal, "complete");
        push_tag(proposal, "collection");
        push_evidence(
            proposal,
            "resource_completeness",
            value,
            "filename-bracket",
            "collection-marker",
        );
    }
    let lower = value.to_ascii_lowercase();
    let mut matched = contains_collection_marker(value);
    let chinese_language_only = matches!(value.trim(), "中国語" | "中国语" | "中文");
    if chinese_language_only {
        set_once(
            &mut proposal.resource_language,
            "zh",
            &mut proposal.evidence,
            "language",
            "filename-bracket",
            "language-alias",
        );
        matched = true;
    } else if contains_any(
        value,
        &[
            "中国翻訳",
            "中国翻译",
            "汉化",
            "漢化",
            "简中",
            "繁中",
            "簡中",
            "繁體中文",
            "简体中文",
            "简中",
            "繁中",
            "中文",
        ],
    ) || matches!(lower.as_str(), "chinese" | "zh" | "zh-cn")
    {
        set_once(
            &mut proposal.resource_language,
            "zh",
            &mut proposal.evidence,
            "language",
            "filename-bracket",
            "translation-language",
        );
        set_once(
            &mut proposal.translation_state,
            "translated",
            &mut proposal.evidence,
            "translation_state",
            "filename-bracket",
            "translation-label",
        );
        push_tag(proposal, "translated");
        matched = true;
    } else if contains_any(value, &["英訳", "英文", "英语"]) || lower == "english" {
        set_once(
            &mut proposal.resource_language,
            "en",
            &mut proposal.evidence,
            "language",
            "filename-bracket",
            "translation-language",
        );
        set_once(
            &mut proposal.translation_state,
            "translated",
            &mut proposal.evidence,
            "translation_state",
            "filename-bracket",
            "translation-label",
        );
        push_tag(proposal, "translated");
        matched = true;
    } else if contains_any(value, &["韓国語", "한국어"]) || lower == "korean" {
        set_once(
            &mut proposal.resource_language,
            "ko",
            &mut proposal.evidence,
            "language",
            "filename-bracket",
            "translation-language",
        );
        set_once(
            &mut proposal.translation_state,
            "translated",
            &mut proposal.evidence,
            "translation_state",
            "filename-bracket",
            "translation-label",
        );
        matched = true;
    }
    if contains_any(value, &["無修正", "无修正", "無修", "无修"]) || lower == "decensored"
    {
        proposal.censorship = Some("uncensored".into());
        push_tag(proposal, "uncensored");
        matched = true;
    } else if contains_any(value, &["修正版"]) || lower == "censored" {
        proposal.censorship = Some("censored".into());
        push_tag(proposal, "censored");
        matched = true;
    }
    // `DL` in manga release names is a local quality marker (download/high
    // definition), not a publication-edition assertion.  Keep the useful
    // quality evidence, but never materialize the old misleading
    // `resource_edition=digital`/`digital` tag.  Explicit digital-version
    // wording is consumed as ignorable release noise.
    if lower == "dl"
        || lower == "dl版"
        || lower == "hd"
        || value == "DL版"
        || value == "高画質"
        || value == "高清"
    {
        push_tag(proposal, "high_quality");
        matched = true;
    } else if matches!(
        lower.as_str(),
        "digital" | "ebook" | "electronic"
    ) || matches!(value, "デジタル版" | "数字版" | "电子版" | "電子版")
    {
        matched = true;
    }
    if lower == "raw" || value == "生肉" {
        proposal.source_medium = Some("raw".into());
        push_tag(proposal, "raw");
        matched = true;
    }
    if lower == "lq" || lower == "low quality" || value == "低清" {
        push_tag(proposal, "low_quality");
        matched = true;
    }
    if lower == "webrip" || lower == "web rip" {
        proposal.source_medium = Some("web_rip".into());
        push_tag(proposal, "web_rip");
        matched = true;
    }
    if lower == "c2c" {
        proposal.scan_completeness = Some("cover_to_cover".into());
        push_tag(proposal, "c2c");
        matched = true;
    }
    if lower == "noads" || lower == "no ads" {
        proposal.scan_completeness = Some("no_ads".into());
        push_tag(proposal, "no_ads");
        matched = true;
    }
    if lower == "complete"
        || lower == "end"
        || value == "全"
        || value == "全集"
        || value == "全卷"
        || value == "完"
        || value == "完結"
    {
        proposal.resource_completeness = Some("complete".into());
        push_tag(proposal, "complete");
        matched = true;
    }
    if lower == "incomplete" || value == "不完整" {
        proposal.resource_completeness = Some("partial".into());
        push_tag(proposal, "incomplete");
        matched = true;
    }
    if lower == "colorized" || contains_any(value, &["カラー化", "彩色"]) {
        proposal.color_state = Some("colorized".into());
        push_tag(proposal, "colorized");
        matched = true;
    }
    if lower == "textless" || contains_any(value, &["無字", "无字"]) {
        push_tag(proposal, "textless");
        matched = true;
    }
    if lower == "sample" || value == "样本" || value == "例本" {
        proposal.resource_completeness = Some("sample".into());
        push_tag(proposal, "sample");
        matched = true;
    }
    if lower == "ai generated" || value == "AI生成" {
        push_tag(proposal, "ai_generated");
        matched = true;
    }
    if lower == "mtl"
        || matches!(value.trim(), "机翻" | "機翻" | "AI翻訳" | "AI翻译")
        || lower.contains("machine translation")
        || contains_any(value, &["机翻", "機翻", "AI翻訳", "AI翻译"])
    {
        proposal.translation_state = Some("translated".into());
        proposal.translation_method = Some("machine".into());
        matched = true;
    } else if lower.contains("human translation") || contains_any(value, &["人工翻译", "人工翻訳"])
    {
        proposal.translation_state = Some("translated".into());
        proposal.translation_method = Some("human".into());
        matched = true;
    }
    // A named scanlation group is release provenance, not a provider and not
    // a creator. Keep the explicit local token even when it also establishes
    // translation state above (for example `[無邪気漢化組]`).
    if is_explicit_scanlation_group_text(value) {
        add_release_group(proposal, value);
        matched = true;
    }
    matched
}

fn classify_inline_resource_terms(value: &str, proposal: &mut NameRoleProposal) {
    let lower = value.to_ascii_lowercase();
    if lower.contains("dl版")
        || lower.contains(" hd")
        || lower.starts_with("hd ")
        || lower.contains("高画質")
        || lower.contains("高清")
    {
        push_tag(proposal, "high_quality");
    }
    if lower.contains(" raw ") || lower.ends_with(" raw") || value.contains("生肉") {
        proposal.source_medium = Some("raw".into());
        push_tag(proposal, "raw");
    }
    if lower.contains("webrip") {
        proposal.source_medium = Some("web_rip".into());
        push_tag(proposal, "web_rip");
    }
    if value.contains("生肉") {
        proposal.translation_state = Some("untranslated".into());
        push_tag(proposal, "untranslated");
    } else if value.contains("熟肉") || value.contains("汉化") || value.contains("漢化") {
        proposal.translation_state = Some("translated".into());
        push_tag(proposal, "translated");
    }
    if value.contains("单行本") || value.contains("単行本") {
        proposal.resource_tags.push("tankoubon".into());
    }
    if value.contains("全集") || value.contains("完結") || value.contains("完结") {
        proposal.resource_completeness = Some("complete".into());
        push_tag(proposal, "complete");
    }
    if contains_collection_marker(value) {
        proposal.resource_completeness = Some("complete".into());
        proposal.is_collection = true;
        push_tag(proposal, "complete");
        push_tag(proposal, "collection");
    }
    if lower.contains("low quality") || lower.split_whitespace().any(|token| token == "lq") {
        push_tag(proposal, "low_quality");
    }
    if lower.contains("complete edition") || value.contains("完全版") || value.contains("完整版")
    {
        proposal.resource_edition = Some("complete_edition".into());
    }
}

fn parse_numeric_term(value: &str, start: usize) -> Option<(usize, String)> {
    let bytes = value.as_bytes();
    if !bytes.get(start).is_some_and(u8::is_ascii_digit) {
        return None;
    }
    let mut end = start;
    while bytes.get(end).is_some_and(u8::is_ascii_digit) {
        end += 1;
    }
    if bytes.get(end) == Some(&b'.') && bytes.get(end + 1).is_some_and(u8::is_ascii_digit) {
        end += 1;
        while bytes.get(end).is_some_and(u8::is_ascii_digit) {
            end += 1;
        }
    }
    Some((end, value[start..end].to_owned()))
}

fn parse_composite_atom(value: &str, start: usize) -> Option<(usize, String)> {
    if let Some(number) = parse_numeric_term(value, start) {
        return Some(number);
    }
    let ch = value[start..].chars().next()?;
    if is_roman_numeral(ch) {
        Some((start + ch.len_utf8(), ch.to_string()))
    } else {
        None
    }
}

fn is_roman_numeral(ch: char) -> bool {
    matches!(
        ch,
        'Ⅰ' | 'Ⅱ'
            | 'Ⅲ'
            | 'Ⅳ'
            | 'Ⅴ'
            | 'Ⅵ'
            | 'Ⅶ'
            | 'Ⅷ'
            | 'Ⅸ'
            | 'Ⅹ'
            | 'ⅰ'
            | 'ⅱ'
            | 'ⅲ'
            | 'ⅳ'
            | 'ⅴ'
            | 'ⅵ'
            | 'ⅶ'
            | 'ⅷ'
            | 'ⅸ'
            | 'ⅹ'
    )
}

fn composite_special_at(value: &str, start: usize) -> Option<(usize, String)> {
    for marker in ["番外篇", "番外編", "番外", "特典"] {
        if value[start..].starts_with(marker) {
            return Some((start + marker.len(), marker.to_owned()));
        }
    }
    None
}

fn extract_composite_sequences(value: &str, existing_range: bool) -> Vec<CompositeSequence> {
    let mut matches = Vec::new();
    for (digit_start, ch) in value.char_indices() {
        if !ch.is_ascii_digit() && !is_roman_numeral(ch) {
            continue;
        }
        let mut expression_start = digit_start;
        let mut numeric_start = digit_start;
        if digit_start > 0 {
            if let Some(previous) = value[..digit_start].chars().next_back() {
                if matches!(previous, 'm' | 'M') {
                    expression_start = digit_start - previous.len_utf8();
                    numeric_start = digit_start;
                }
            }
        }
        let Some((mut cursor, first)) = parse_composite_atom(value, numeric_start) else {
            continue;
        };
        let mut members = vec![first.clone()];
        let mut range = None;
        let mut includes_special = false;
        let mut connector_count = 0;
        loop {
            let connector_start = cursor;
            while value[cursor..]
                .chars()
                .next()
                .is_some_and(char::is_whitespace)
            {
                cursor += value[cursor..].chars().next().unwrap().len_utf8();
            }
            let Some(connector) = value[cursor..].chars().next() else {
                break;
            };
            if !matches!(connector, '+' | '-' | '~' | '～') {
                break;
            }
            cursor += connector.len_utf8();
            while value[cursor..]
                .chars()
                .next()
                .is_some_and(char::is_whitespace)
            {
                cursor += value[cursor..].chars().next().unwrap().len_utf8();
            }
            let member = if let Some((end, number)) = parse_composite_atom(value, cursor) {
                cursor = end;
                Some(number)
            } else if matches!(value[cursor..].chars().next(), Some('m' | 'M')) {
                let prefix_len = value[cursor..].chars().next().unwrap().len_utf8();
                parse_numeric_term(value, cursor + prefix_len).map(|(end, number)| {
                    cursor = end;
                    number
                })
            } else if connector == '+' {
                composite_special_at(value, cursor).map(|(end, marker)| {
                    cursor = end;
                    includes_special = true;
                    marker
                })
            } else {
                None
            };
            let Some(member) = member else {
                cursor = connector_start;
                break;
            };
            connector_count += 1;
            if range.is_none() && matches!(connector, '-' | '~' | '～') {
                range = Some(format!("{}{}{}", first, connector, member));
            }
            members.push(member);
        }
        if connector_count > 0 {
            matches.push(CompositeSequence {
                start: expression_start,
                end: cursor,
                members,
                range,
                includes_special,
            });
        }
    }

    if existing_range {
        let mut cursor = value.find('+').unwrap_or(value.len());
        if cursor < value.len() {
            let start = cursor;
            let mut members = Vec::new();
            while cursor < value.len() && value[cursor..].starts_with('+') {
                cursor += 1;
                while value[cursor..]
                    .chars()
                    .next()
                    .is_some_and(char::is_whitespace)
                {
                    cursor += value[cursor..].chars().next().unwrap().len_utf8();
                }
                let Some((end, member)) = parse_numeric_term(value, cursor) else {
                    break;
                };
                cursor = end;
                members.push(member);
            }
            if !members.is_empty() {
                matches.push(CompositeSequence {
                    start,
                    end: cursor,
                    members,
                    range: None,
                    includes_special: false,
                });
            }
        }
    }

    matches.sort_by_key(|item| (item.start, std::cmp::Reverse(item.end)));
    let mut unique = Vec::new();
    for item in matches {
        if unique.iter().any(|existing: &CompositeSequence| {
            item.start < existing.end && item.end > existing.start
        }) {
            continue;
        }
        unique.push(item);
    }
    unique
}

fn extract_sequence(value: &str, proposal: &mut NameRoleProposal) -> SequenceData {
    let mut sequence = SequenceData {
        kind: proposal.sequence_kind.clone(),
        part: proposal.part,
        range: proposal.range.clone(),
        sequence_members: proposal.sequence_members.clone(),
        includes_special: proposal.includes_special,
        is_collection: proposal.is_collection,
        sequence_label: proposal.sequence_label.clone(),
        relation: proposal.chapter_relation.clone(),
        season_range: proposal.season_range.clone(),
        chapter_range: proposal.chapter_range.clone(),
        ..SequenceData::default()
    };
    let mut spans = Vec::new();
    let mut has_part_marker = false;

    for composite in extract_composite_sequences(value, sequence.range.is_some()) {
        let composite_axis = composite
            .range
            .as_ref()
            .and_then(|_| sequence_axis(&value[composite.end..]));
        match composite_axis {
            Some("season") => {
                sequence.kind.get_or_insert_with(|| "season".into());
                sequence.season_range = composite.range.clone();
            }
            Some("chapter") => {
                sequence.kind.get_or_insert_with(|| "chapter".into());
                sequence.chapter_range = composite.range.clone();
            }
            _ => {
                sequence.kind.get_or_insert_with(|| "issue".into());
                if sequence.range.is_none() {
                    sequence.range = composite.range.clone();
                }
            }
        }
        for member in composite.members {
            if !sequence
                .sequence_members
                .iter()
                .any(|existing| existing == &member)
            {
                sequence.sequence_members.push(member);
            }
        }
        sequence.includes_special |= composite.includes_special;
        if let Some(suffix) = cjk_volume_suffix(&value[composite.end..]) {
            spans.push((composite.end, composite.end + suffix.len()));
        } else if let Some(suffix) = cjk_chapter_suffix(&value[composite.end..]) {
            spans.push((composite.end, composite.end + suffix.len()));
        } else if let Some(axis) = composite_axis {
            let suffix_len = value[composite.end..]
                .chars()
                .next()
                .map(char::len_utf8)
                .unwrap_or_default();
            spans.push((composite.end, composite.end + suffix_len));
            if (axis == "season" && sequence.chapter_range.is_some())
                || (axis == "chapter" && sequence.season_range.is_some())
            {
                sequence.kind = Some("collection".into());
                sequence.is_collection = true;
            }
        }
        if let Some(axis) = composite_axis {
            if (axis == "season" && sequence.chapter_range.is_some())
                || (axis == "chapter" && sequence.season_range.is_some())
            {
                sequence.kind = Some("collection".into());
                sequence.is_collection = true;
            }
        }
        spans.push((composite.start, composite.end));
        push_evidence(
            proposal,
            "composite_sequence",
            &value[composite.start..composite.end],
            "filename",
            "composite-sequence-expression",
        );
    }
    for (term, relation, _rank, part_number, is_part_marker) in [
        ("前篇", "front_part", -1, Some(1), true),
        ("前編", "front_part", -1, Some(1), true),
        ("后篇", "back_part", 2, Some(2), true),
        ("後篇", "back_part", 2, Some(2), true),
        ("后编", "back_part", 2, Some(2), true),
        ("後編", "back_part", 2, Some(2), true),
        ("续篇", "continuation", 1, None, true),
        ("続編", "continuation", 1, None, true),
        ("番外篇", "side_story", 0, None, false),
        ("番外編", "side_story", 0, None, false),
        ("番外", "side_story", 0, None, false),
        ("外传", "side_story", 0, None, false),
        ("外伝", "side_story", 0, None, false),
        ("序章", "prologue", -2, None, false),
        ("终章", "epilogue", 99, None, false),
        ("終章", "epilogue", 99, None, false),
        ("幕间", "interlude", 0, None, false),
        ("幕間", "interlude", 0, None, false),
    ] {
        for (start, end) in find_all(value, term) {
            let marker_start = if start > 0 && value[..start].ends_with('+') {
                start - 1
            } else {
                start
            };
            if is_part_marker {
                let has_other_part = sequence
                    .sequence_members
                    .iter()
                    .any(|member| member == "front_part" || member == "back_part");
                if !has_other_part {
                    sequence.sequence_members.push(relation.into());
                    sequence.sequence_label = Some(term.into());
                    sequence.relation = Some(relation.into());
                    sequence.part = part_number;
                } else if !sequence
                    .sequence_members
                    .iter()
                    .any(|member| member == relation)
                {
                    sequence.sequence_members.push(relation.into());
                    sequence.is_collection = true;
                    sequence.part = None;
                    sequence.sequence_label = None;
                    sequence.relation = None;
                }
                has_part_marker = true;
            } else if sequence.sequence_label.is_none() {
                sequence.sequence_label = Some(term.into());
                sequence.relation = Some(relation.into());
                sequence.kind = Some("special".into());
                push_evidence(
                    proposal,
                    "sequence_label",
                    term,
                    "filename",
                    "special-marker",
                );
            }
            spans.push((marker_start, end));
        }
    }

    for (start, end, raw) in digit_runs(value) {
        if spans
            .iter()
            .any(|(span_start, span_end)| start >= *span_start && start < *span_end)
        {
            continue;
        }
        let before = &value[..start];
        let after = &value[end..];
        let after_trimmed = after.trim_start();
        let before_trimmed = before.trim_end();
        if is_rating_context(before_trimmed) || is_unresolved_no_number_context(before) {
            continue;
        }
        let mut consumed_end = end;
        let first_number = first_number(&raw);
        let range = parse_range(&raw);
        let context = preceding_context(before_trimmed);
        let lower_context = context.to_ascii_lowercase();
        let mut kind = None;
        let mut relation = None;
        let mut rank = 0;
        let mut weak_terminal_number = false;
        let range_axis = range.as_ref().and_then(|_| sequence_axis(after_trimmed));

        if let Some((suffix, rel, suffix_rank)) = continuation_suffix(after_trimmed) {
            let delimiter_len = after_trimmed.strip_prefix('+').map(|_| 1).unwrap_or(0);
            consumed_end += delimiter_len + suffix.len();
            kind = Some("chapter");
            relation = Some(rel.into());
            rank = suffix_rank;
        } else if let Some(suffix) = cjk_chapter_suffix(after_trimmed) {
            consumed_end += suffix.len();
            kind = Some(if suffix == "화" || suffix == "회" {
                "episode"
            } else {
                "chapter"
            });
        } else if let Some(suffix) = cjk_volume_suffix(after_trimmed) {
            consumed_end += suffix.len();
            kind = Some("volume");
        } else if lower_context.ends_with("season")
            || lower_context.ends_with("시즌")
            || (lower_context.ends_with('s') && !lower_context.ends_with("series"))
        {
            kind = Some("season");
        } else if lower_context.ends_with("vol")
            || lower_context.ends_with("vol.")
            || lower_context.ends_with("volume")
            || lower_context.ends_with('v')
            || lower_context.ends_with("tome")
        {
            kind = Some("volume");
        } else if lower_context.ends_with("chapter")
            || lower_context.ends_with("ch")
            || lower_context.ends_with("ch.")
            || lower_context.ends_with("episode")
            || lower_context.ends_with("ep")
            || lower_context.ends_with("issue")
        {
            kind = Some(
                if lower_context.ends_with("episode") || lower_context.ends_with("ep") {
                    "episode"
                } else {
                    "chapter"
                },
            );
        } else if before_trimmed.ends_with('#') {
            kind = Some("issue");
        } else if lower_context.ends_with("annual") {
            kind = Some("special");
        } else if before_trimmed.contains("番外")
            || before_trimmed.contains("外传")
            || before_trimmed.contains("外伝")
        {
            kind = Some("special");
            relation = Some("side_story".into());
        } else if let Some(axis) = range_axis {
            kind = Some(axis);
        } else if range.is_some() {
            kind = Some(
                if lower_context.ends_with('v')
                    || lower_context.ends_with("vol")
                    || lower_context.ends_with("volume")
                {
                    "volume"
                } else {
                    "issue"
                },
            );
        } else if single_alpha_suffix(after_trimmed).is_some() {
            kind = Some("chapter");
        } else if raw.len() == 1 && lower_context.ends_with("part") {
            kind = Some("part");
        } else if raw.len() == 2 && before_trimmed.ends_with('S') {
            kind = Some("season");
        } else if raw.contains('.') {
            // Decimal chapter labels (57.2, 12.25, …) are one sequence
            // token even when the filename omits `Ch.`/`话`.
            kind = Some("chapter");
        } else if !first_number.is_empty()
            && (after_trimmed.trim().is_empty()
                || after_trimmed.starts_with(|c: char| c == '_' || c == '-'))
            && !lower_context.ends_with("201")
        {
            kind = Some("issue");
            weak_terminal_number = after_trimmed.trim().is_empty()
                && before
                    .as_bytes()
                    .last()
                    .is_some_and(|byte| !byte.is_ascii_whitespace());
        }

        if before_trimmed.contains("番外")
            || before_trimmed.contains("外传")
            || before_trimmed.contains("外伝")
            || before_trimmed.contains("외전")
        {
            kind = Some("special");
            relation = Some("side_story".into());
        }

        let Some(kind) = kind else {
            continue;
        };
        spans.push((start, consumed_end));
        if kind == "issue" && weak_terminal_number && sequence.issue.is_none() {
            sequence.weak_terminal_number = true;
            sequence.weak_terminal_span = Some((start, consumed_end));
        }
        let rendered = if let Some((from, to)) = range.clone() {
            let rendered_range = format!("{from}-{to}");
            match range_axis {
                Some("season") => {
                    sequence.season_range = Some(rendered_range.clone());
                    sequence.season = None;
                    if sequence.chapter_range.is_some() {
                        sequence.kind = Some("collection".into());
                        sequence.is_collection = true;
                    }
                }
                Some("chapter") => {
                    sequence.chapter_range = Some(rendered_range.clone());
                    sequence.chapter = None;
                    if sequence.season_range.is_some() {
                        sequence.kind = Some("collection".into());
                        sequence.is_collection = true;
                    }
                }
                _ => sequence.range = Some(rendered_range.clone()),
            }
            rendered_range
        } else {
            normalize_sequence_number(&first_number)
        };
        match kind {
            "volume" => {
                sequence.kind.get_or_insert_with(|| "volume".into());
                sequence.volume.get_or_insert(rendered.clone());
            }
            "chapter" | "episode" => {
                sequence.kind.get_or_insert_with(|| kind.into());
                sequence.chapter.get_or_insert(rendered.clone());
                if let Some(suffix) = single_alpha_suffix(after_trimmed) {
                    consumed_end += suffix.len();
                    spans.push((start, consumed_end));
                    sequence.chapter_title.get_or_insert(suffix.clone());
                }
            }
            "issue" => {
                sequence.kind.get_or_insert_with(|| "issue".into());
                sequence.issue.get_or_insert(rendered.clone());
            }
            "season" => {
                sequence
                    .season
                    .get_or_insert(normalize_sequence_number(&first_number));
            }
            "part" => {
                sequence
                    .part
                    .get_or_insert(first_number.parse().unwrap_or_default());
            }
            "special" => {
                sequence.kind.get_or_insert_with(|| "special".into());
                sequence
                    .part
                    .get_or_insert(first_number.parse().unwrap_or_default());
                if relation.as_deref() == Some("side_story") {
                    sequence.chapter.get_or_insert(first_number.clone());
                } else {
                    sequence
                        .issue
                        .get_or_insert(normalize_sequence_number(&first_number));
                }
            }
            _ => {}
        }
        if range_axis == Some("season") {
            sequence.season = None;
        } else if range_axis == Some("chapter") {
            sequence.chapter = None;
        }
        if relation.is_some() {
            sequence.relation = relation;
        }
        if sequence.chapter.is_some() {
            let relation_rank = sequence
                .relation
                .as_deref()
                .map(relation_rank)
                .unwrap_or(rank);
            sequence.sort_key = Some(chapter_order_key(
                sequence.chapter.as_deref().unwrap_or(&first_number),
                relation_rank,
                sequence.chapter_title.as_deref(),
            ));
        }
        push_evidence(
            proposal,
            kind,
            &rendered,
            "filename",
            "numeric-sequence-marker",
        );
    }

    if has_part_marker && sequence.chapter.is_none() && sequence.volume.is_none() {
        sequence.kind = Some("part".into());
    }

    sequence.spans = merge_spans(spans);
    sequence
}

fn apply_sequence(sequence: &SequenceData, proposal: &mut NameRoleProposal) {
    proposal.sequence_kind = sequence.kind.clone();
    proposal.volume = sequence.volume.clone();
    proposal.chapter = sequence.chapter.clone();
    proposal.issue = sequence.issue.clone();
    proposal.season = sequence.season.clone();
    proposal.season_range = sequence.season_range.clone();
    proposal.chapter_range = sequence.chapter_range.clone();
    proposal.part = sequence.part.clone();
    proposal.range = sequence.range.clone();
    proposal.sequence_members = sequence.sequence_members.clone();
    proposal.includes_special = sequence.includes_special;
    proposal.is_collection = sequence.is_collection;
    proposal.sequence_label = sequence.sequence_label.clone();
    proposal.chapter_title = sequence.chapter_title.clone();
    proposal.chapter_relation = sequence.relation.clone();
    proposal.sort_key = sequence.sort_key.clone();
}

fn sibling_context(snapshot: &CatalogSnapshot) -> Vec<String> {
    snapshot
        .parent_siblings
        .iter()
        .filter(|name| !name.trim().eq_ignore_ascii_case(snapshot.filename.trim()))
        .cloned()
        .collect()
}

fn is_ambiguous_numeric_identifier(value: &str, snapshot: &CatalogSnapshot) -> bool {
    let trimmed = value.trim();
    snapshot.ancestor_dirs.is_empty()
        && snapshot.parent_siblings.is_empty()
        && trimmed.len() >= 4
        && trimmed.chars().all(|ch| ch.is_ascii_digit())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AncestorSemantic {
    Format,
    Media,
    Publication,
    ExplicitCreator,
    WorkCandidate,
}

fn ancestor_semantic(value: &str) -> AncestorSemantic {
    let normalized = value.trim().to_ascii_lowercase();
    if matches!(
        normalized.as_str(),
        "epub" | "pdf" | "cbz" | "cbr" | "zip" | "rar" | "mobi" | "azw" | "azw3" | "7z"
    ) {
        return AncestorSemantic::Format;
    }
    if matches!(
        normalized.as_str(),
        "漫画"
            | "manga"
            | "comic"
            | "comics"
            | "小说"
            | "轻小说"
            | "杂志"
            | "图集"
            | "画集"
            | "日漫"
            | "国漫"
            | "韩漫"
            | "韓漫"
            | "欧美"
            | "分类"
            | "分类目录"
            | "书架"
            | "書架"
            | "downloads"
            | "download"
            | "全部"
            | "全部文件"
            | "根目录"
            | "root"
            | "夸克"
            | "夸克网盘"
            | "quark"
            | "云盘"
    ) {
        return AncestorSemantic::Media;
    }
    if matches!(
        normalized.as_str(),
        "单行本" | "连载" | "合集" | "全集" | "短篇" | "番外" | "tankoubon" | "serial"
    ) {
        return AncestorSemantic::Publication;
    }
    if is_explicit_author(value) || is_author_label_like(value) {
        return AncestorSemantic::ExplicitCreator;
    }
    AncestorSemantic::WorkCandidate
}

fn is_ancestor_bucket(value: &str) -> bool {
    matches!(
        ancestor_semantic(value),
        AncestorSemantic::Format | AncestorSemantic::Media | AncestorSemantic::Publication
    )
}

fn has_publication_context(ancestors: &[String], target: &str) -> bool {
    ancestors.iter().any(|ancestor| {
        let normalized = ancestor.trim().to_ascii_lowercase();
        normalized == target
            || (target == "tankoubon" && matches!(normalized.as_str(), "单行本" | "tankoubon"))
            || (target == "serial" && matches!(normalized.as_str(), "连载" | "serial"))
    })
}

fn pure_numeric_token(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if !trimmed.is_empty() && trimmed.chars().all(|ch| ch.is_ascii_digit()) {
        return Some(trimmed.to_owned());
    }
    None
}

fn add_numeric_label(
    proposal: &mut NameRoleProposal,
    prefix: &str,
    value: &str,
    raw: &str,
    semantic_role: &str,
    source: &str,
    rule: &str,
) {
    if !proposal.numeric_labels.iter().any(|label| {
        label.prefix == prefix
            && label.value == value
            && label.raw == raw
            && label.semantic_role == semantic_role
    }) {
        proposal.numeric_labels.push(NumericLabelCandidate {
            prefix: prefix.to_owned(),
            value: value.to_owned(),
            semantic_role: semantic_role.to_owned(),
            raw: raw.to_owned(),
        });
    }
    if !proposal
        .warnings
        .iter()
        .any(|warning| warning == "unresolved_numeric_label")
        && semantic_role == "unresolved"
    {
        proposal.warnings.push("unresolved_numeric_label".into());
    }
    push_evidence(proposal, "numeric_label", raw, source, rule);
}

fn detect_unresolved_numeric_labels(value: &str, proposal: &mut NameRoleProposal) {
    for token in value.split_whitespace() {
        let token = token.trim_matches(|ch: char| matches!(ch, '[' | ']' | '(' | ')' | '{' | '}'));
        let normalized = token.replace('．', ".");
        let upper = normalized.to_ascii_uppercase();
        let Some(rest) = upper.strip_prefix("NO.") else {
            continue;
        };
        if rest.is_empty() || !rest.chars().all(|ch| ch.is_ascii_digit()) {
            continue;
        }
        let number = &normalized[3..];
        add_numeric_label(
            proposal,
            "NO",
            number,
            &normalized,
            "unresolved",
            "filename",
            "unresolved-number-label",
        );
    }
}

fn is_unresolved_no_number_context(value: &str) -> bool {
    let trimmed = value.trim_end();
    trimmed
        .strip_suffix('.')
        .or(Some(trimmed))
        .is_some_and(|candidate| candidate.trim_end().to_ascii_lowercase().ends_with("no"))
}

fn is_rating_context(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    ["评分", "评价", "評分", "評価", "rating", "score"]
        .iter()
        .any(|term| lower.contains(term))
        || value.chars().any(|ch| matches!(ch, '★' | '☆' | '⭐'))
}

fn sibling_numeric_support(snapshot: &CatalogSnapshot, current: &str) -> bool {
    let Some(current_number) = current.parse::<i64>().ok() else {
        return false;
    };
    let mut neighboring_numbers = Vec::new();
    for sibling in sibling_context(snapshot) {
        let stem = strip_extension(&sibling);
        let Some((_, _, raw)) = digit_runs(&stem).into_iter().next() else {
            continue;
        };
        let raw = first_number(&raw);
        if let Ok(number) = raw.parse::<i64>() {
            neighboring_numbers.push(number);
        }
    }
    neighboring_numbers.len() >= 2
        && neighboring_numbers
            .iter()
            .any(|number| (number - current_number).abs() <= 1)
}

fn parenthetical_numeric_context(filename: &str) -> Option<(String, String)> {
    let decoded = decode_html_entities(filename);
    let stem = strip_extension(&decoded);
    let (groups, core) = extract_groups(&stem);
    let number = groups.iter().find_map(|group| {
        (matches!(group.delimiter, Delimiter::Round | Delimiter::FullWidth))
            .then(|| pure_numeric_token(&group.content))
            .flatten()
    })?;
    let title = clean_title_text(&core);
    Some((number, normalize_for_compare(&title)))
}

fn sibling_parenthetical_numeric_support(snapshot: &CatalogSnapshot) -> bool {
    let Some((current_number, current_title)) = parenthetical_numeric_context(&snapshot.filename)
    else {
        return false;
    };
    let mut sibling_numbers = std::collections::BTreeSet::new();
    for sibling in sibling_context(snapshot) {
        let Some((number, title)) = parenthetical_numeric_context(&sibling) else {
            continue;
        };
        if title == current_title && number != current_number {
            sibling_numbers.insert(number);
        }
    }
    sibling_numbers.len() >= 2
}

fn sibling_terminal_sequence_support(snapshot: &CatalogSnapshot, current: &str) -> bool {
    let current_stem = strip_extension(&decode_html_entities(current));
    let current_runs = digit_runs(&current_stem);
    let Some(&(current_start, current_end, ref current_raw)) = current_runs.last() else {
        return false;
    };
    if !current_stem[current_end..].trim().is_empty() {
        return false;
    }
    let current_number = first_number(&current_raw).parse::<i64>().ok();
    let current_base = normalize_for_compare(current_stem[..current_start].trim_end());
    if current_base.is_empty() {
        return false;
    }
    let mut sibling_numbers = std::collections::BTreeSet::new();
    for sibling in sibling_context(snapshot) {
        let stem = strip_extension(&decode_html_entities(&sibling));
        let runs = digit_runs(&stem);
        let Some(&(start, end, ref raw)) = runs.last() else {
            continue;
        };
        if !stem[end..].trim().is_empty()
            || normalize_for_compare(stem[..start].trim_end()) != current_base
        {
            continue;
        }
        if let Ok(number) = first_number(&raw).parse::<i64>() {
            sibling_numbers.insert(number);
        }
    }
    sibling_numbers.len() >= 2
        && current_number.is_some_and(|number| {
            sibling_numbers
                .iter()
                .any(|sibling| (*sibling - number).abs() <= 1)
        })
}

fn bilingual_numeric_spans(value: &str, number: &str) -> Vec<(usize, usize)> {
    let mut boundaries = vec![0];
    for (index, ch) in value.char_indices() {
        if matches!(ch, '|' | '｜' | '丨' | '│') {
            boundaries.push(index);
            boundaries.push(index + ch.len_utf8());
        }
    }
    boundaries.push(value.len());

    boundaries
        .windows(2)
        .filter_map(|window| {
            let start = window[0];
            let end = window[1];
            let segment = &value[start..end];
            let runs = digit_runs(segment);
            let &(digit_start, digit_end, ref raw) = runs.last()?;
            if digit_end != segment.trim_end().len() || first_number(raw) != number {
                return None;
            }
            Some((start + digit_start, start + digit_end))
        })
        .collect()
}

fn bilingual_numeric_support(value: &str, number: &str) -> bool {
    bilingual_numeric_spans(value, number).len() >= 2
}

fn sibling_has_continuation_marker(snapshot: &CatalogSnapshot) -> bool {
    sibling_context(snapshot).iter().any(|sibling| {
        let stem = strip_extension(&decode_html_entities(sibling));
        let compact = stem.replace([' ', '_', '-'], "");
        compact.contains('续')
            || compact.contains('續')
            || compact.contains('続')
            || compact.contains("后篇")
            || compact.contains("後編")
            || compact.contains("前篇")
            || compact.contains("前編")
    })
}

fn adjust_sequence_from_context(
    sequence: &mut SequenceData,
    core: &str,
    ancestors: &[String],
    snapshot: &CatalogSnapshot,
    proposal: &mut NameRoleProposal,
) {
    if let Some(issue) = sequence.issue.clone() {
        for span in bilingual_numeric_spans(core, &issue) {
            if !sequence.spans.contains(&span) {
                sequence.spans.push(span);
            }
        }
    }

    if sequence.issue.is_none() && sibling_parenthetical_numeric_support(snapshot) {
        if let Some(label_index) = proposal
            .numeric_labels
            .iter()
            .position(|label| label.semantic_role == "unresolved" && label.prefix.is_empty())
        {
            let number = proposal.numeric_labels[label_index].value.clone();
            proposal.numeric_labels[label_index].semantic_role = "issue".into();
            sequence.kind = Some("issue".into());
            sequence.issue = Some(number.clone());
            sequence.sort_key = None;
            proposal
                .warnings
                .retain(|warning| warning != "unresolved_numeric_label");
            push_evidence(
                proposal,
                "issue",
                &number,
                "catalog-context",
                "parenthetical-sibling-sequence",
            );
        }
    }

    // Plain numeric siblings are ambiguous in isolation, but a persisted
    // sibling such as `10续`/`06+续` establishes a chapter stream for the
    // whole directory. Promote the plain members to chapter rather than
    // leaving one directory split between `issue` and `chapter` kinds.
    if sequence.issue.is_some()
        && sequence.chapter.is_none()
        && pure_numeric_token(core).is_some()
        && sibling_has_continuation_marker(snapshot)
    {
        let number = sequence.issue.take().unwrap_or_default();
        sequence.kind = Some("chapter".into());
        sequence.chapter = Some(number.clone());
        sequence.sort_key = Some(chapter_order_key(&number, 0, None));
        proposal.evidence.retain(|evidence| {
            !(evidence.role == "issue"
                && evidence.value == number
                && evidence.rule == "numeric-sequence-marker")
        });
        push_evidence(
            proposal,
            "chapter",
            &number,
            "catalog-context",
            "sibling-continuation-stream",
        );
    }

    if sequence.weak_terminal_number {
        let Some(issue) = sequence.issue.clone() else {
            return;
        };
        let bilingual_supported = bilingual_numeric_support(core, &issue);
        if bilingual_supported {
            for span in bilingual_numeric_spans(core, &issue) {
                if !sequence.spans.contains(&span) {
                    sequence.spans.push(span);
                }
            }
        }
        let supported = has_publication_context(ancestors, "serial")
            || sibling_terminal_sequence_support(snapshot, core)
            || bilingual_supported;
        if !supported {
            if let Some(weak_span) = sequence.weak_terminal_span {
                sequence.spans.retain(|span| *span != weak_span);
            }
            sequence.issue = None;
            if sequence.kind.as_deref() == Some("issue") {
                sequence.kind = None;
            }
            sequence.sort_key = None;
            proposal.evidence.retain(|evidence| {
                !(evidence.role == "issue"
                    && evidence.value == issue
                    && evidence.rule == "numeric-sequence-marker")
            });
            if !proposal
                .warnings
                .iter()
                .any(|warning| warning == "unresolved_terminal_number")
            {
                proposal.warnings.push("unresolved_terminal_number".into());
            }
        }
    }

    let Some(issue) = sequence.issue.clone() else {
        return;
    };
    let Some(numeric) = pure_numeric_token(core) else {
        return;
    };
    if numeric != issue {
        return;
    }
    if has_publication_context(ancestors, "tankoubon") {
        sequence.kind = Some("volume".into());
        sequence.volume = Some(issue.clone());
        sequence.issue = None;
        push_evidence(
            proposal,
            "volume",
            &issue,
            "ancestor-context",
            "tankoubon-numeric-filename",
        );
    } else if has_publication_context(ancestors, "serial")
        || sibling_numeric_support(snapshot, &issue)
    {
        sequence.kind = Some("chapter".into());
        sequence.chapter = Some(issue.clone());
        sequence.issue = None;
        sequence.sort_key = Some(chapter_order_key(&issue, 0, None));
        push_evidence(
            proposal,
            "chapter",
            &issue,
            "catalog-context",
            "serial-or-sibling-numeric-context",
        );
    }
}

/// Apply the same leading-bracket lexical pass to an ancestor that is being
/// considered as a work-title candidate.  A persisted directory name such as
/// `[Vchan]Work` must not bypass the filename grammar and become one opaque
/// title string.  Only local text is inspected here.
fn normalize_ancestor_title_candidate(
    value: &str,
    index: usize,
    proposal: &mut NameRoleProposal,
) -> String {
    let decoded = decode_html_entities(value);
    let (groups, core) = extract_groups(&decoded);
    if groups
        .iter()
        .any(|group| group.leading && group.delimiter == Delimiter::Square)
    {
        if let Some(group) = groups
            .iter()
            .find(|group| group.leading && group.delimiter == Delimiter::Square)
        {
            let evidence_start = proposal.evidence.len();
            let attribution_start = proposal.attribution_candidates.len();
            classify_group(group, proposal);
            let source = format!("ancestor({index})");
            for candidate in proposal
                .attribution_candidates
                .iter_mut()
                .skip(attribution_start)
            {
                candidate.source = source.clone();
            }
            for evidence in proposal.evidence.iter_mut().skip(evidence_start) {
                if evidence.source == "filename-bracket" {
                    evidence.source = source.clone();
                }
            }
        }
        return clean_title_text(&core);
    }
    decoded.trim().to_owned()
}

fn choose_ancestor_title(core: &str, ancestors: &[String]) -> (Option<String>, Option<usize>) {
    let normalized_core = normalize_for_compare(core);
    let mut best: Option<(i32, usize, String)> = None;
    for (index, ancestor) in ancestors.iter().enumerate() {
        let candidate = ancestor.trim();
        if candidate.is_empty()
            || is_noise_label(candidate)
            || is_provider_label(candidate)
            || is_ancestor_bucket(candidate)
            || is_structural_only(candidate)
            || is_explicit_author(candidate)
            || is_author_label_like(candidate)
        {
            continue;
        }
        let normalized = normalize_for_compare(candidate);
        let mut score = if !normalized_core.is_empty()
            && (normalized == normalized_core
                || normalized_core.contains(&normalized)
                || normalized.contains(&normalized_core))
        {
            200
        } else {
            0
        };
        if normalized_core.is_empty() {
            // With no filename title left (e.g. `9.epub`), the nearest
            // meaningful work candidate is normally the last non-structural
            // ancestor in the persisted path.
            score += 50 + index as i32;
        }
        if looks_like_name(candidate) {
            score -= 30;
        }
        if index == 0 {
            score += 5;
        }
        if score > best.as_ref().map(|entry| entry.0).unwrap_or(i32::MIN) {
            best = Some((score, index, candidate.to_owned()));
        }
    }
    best.map(|(_, index, value)| (Some(value), Some(index)))
        .unwrap_or((None, None))
}

fn collect_ancestor_creators(
    proposal: &mut NameRoleProposal,
    ancestors: &[String],
    title_index: Option<usize>,
    filename: &str,
) {
    for (index, ancestor) in ancestors.iter().enumerate() {
        if Some(index) == title_index {
            continue;
        }
        let candidate = ancestor.trim();
        if candidate.is_empty()
            || is_noise_label(candidate)
            || is_provider_label(candidate)
            || is_ancestor_bucket(candidate)
        {
            continue;
        }
        if let Some(name) = value_after_marker(
            candidate,
            &[
                "作者:",
                "作者：",
                "原作:",
                "原作：",
                "作画:",
                "作画：",
                "著者:",
                "著者：",
                "author:",
                "by ",
            ],
        ) {
            push_creator(
                proposal,
                name,
                "artist",
                &format!("ancestor({index})"),
                "explicit-creator-marker",
            );
        } else if is_author_label_like(candidate)
            || (looks_like_name(candidate)
                && normalize_for_compare(filename).contains(&normalize_for_compare(candidate)))
        {
            push_creator(
                proposal,
                candidate,
                "artist",
                &format!("ancestor({index})"),
                "title-adjacent-person",
            );
        }
    }
}

fn push_creator(
    proposal: &mut NameRoleProposal,
    value: &str,
    role: &str,
    source: &str,
    rule: &str,
) {
    let value = value.trim();
    if value.is_empty()
        || proposal
            .creators
            .iter()
            .any(|creator| normalize_for_compare(&creator.name) == normalize_for_compare(value))
    {
        return;
    }
    proposal.creators.push(CreatorCandidate {
        name: value.to_owned(),
        role: role.to_owned(),
        alias_of: None,
    });
    if matches!(role, "author" | "artist" | "writer") {
        proposal.authors.push(value.to_owned());
    }
    push_evidence(proposal, "creator", value, source, rule);
}

fn add_release_group(proposal: &mut NameRoleProposal, value: &str) {
    if !proposal.release_groups.iter().any(|item| item == value) {
        proposal.release_groups.push(value.to_owned());
    }
    push_evidence(
        proposal,
        "release_group",
        value,
        "filename-bracket",
        "release-group-marker",
    );
}

fn add_attribution_candidate(proposal: &mut NameRoleProposal, value: &str) {
    let value = value.trim();
    if value.is_empty() {
        return;
    }
    if !proposal
        .attribution_candidates
        .iter()
        .any(|candidate| candidate.name == value)
    {
        proposal.attribution_candidates.push(AttributionCandidate {
            name: value.to_owned(),
            possible_roles: vec!["creator".into(), "provider".into(), "release_group".into()],
            source: "filename-bracket".into(),
        });
    }
    if !proposal
        .warnings
        .iter()
        .any(|warning| warning == "unresolved_leading_attribution")
    {
        proposal
            .warnings
            .push("unresolved_leading_attribution".into());
    }
}

fn push_tag(proposal: &mut NameRoleProposal, value: &str) {
    if !proposal.resource_tags.iter().any(|tag| tag == value) {
        proposal.resource_tags.push(value.to_owned());
    }
}

fn set_once(
    slot: &mut Option<String>,
    value: &str,
    evidence: &mut Vec<RoleEvidence>,
    role: &str,
    source: &str,
    rule: &str,
) {
    if slot.is_none() {
        *slot = Some(value.to_owned());
        evidence.push(RoleEvidence {
            role: role.to_owned(),
            value: value.to_owned(),
            source: source.to_owned(),
            rule: rule.to_owned(),
        });
    }
}

fn push_evidence(
    proposal: &mut NameRoleProposal,
    role: &str,
    value: &str,
    source: &str,
    rule: &str,
) {
    proposal.evidence.push(RoleEvidence {
        role: role.to_owned(),
        value: value.to_owned(),
        source: source.to_owned(),
        rule: rule.to_owned(),
    });
}

fn strip_extension(value: &str) -> String {
    let trimmed = value.trim();
    match trimmed.rfind('.') {
        Some(index) if index > 0 => trimmed[..index].to_owned(),
        _ => trimmed.to_owned(),
    }
}

fn catalog_basename(value: &str) -> String {
    value
        .trim()
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .trim()
        .to_owned()
}

/// Decode the small HTML entity vocabulary that commonly appears in catalog
/// filenames before any structural tokenization takes place. Numeric entities
/// are intentionally decoded here so `&#124;` cannot be mistaken for issue
/// number `124`.
fn decode_html_entities(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut cursor = 0;
    while cursor < value.len() {
        let Some(relative_start) = value[cursor..].find('&') else {
            output.push_str(&value[cursor..]);
            break;
        };
        let start = cursor + relative_start;
        output.push_str(&value[cursor..start]);
        let Some(relative_end) = value[start..].find(';') else {
            output.push_str(&value[start..]);
            break;
        };
        let end = start + relative_end + 1;
        let entity = &value[start + 1..end - 1];
        if let Some(decoded) = decode_html_entity(entity) {
            output.push(decoded);
        } else {
            output.push_str(&value[start..end]);
        }
        cursor = end;
    }
    output
}

fn decode_html_entity(entity: &str) -> Option<char> {
    if let Some(hexadecimal) = entity
        .strip_prefix("#x")
        .or_else(|| entity.strip_prefix("#X"))
    {
        return u32::from_str_radix(hexadecimal, 16)
            .ok()
            .and_then(char::from_u32);
    }
    if let Some(decimal) = entity.strip_prefix('#') {
        return decimal.parse::<u32>().ok().and_then(char::from_u32);
    }
    Some(match entity.to_ascii_lowercase().as_str() {
        "nbsp" => ' ',
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "vert" => '|',
        "brvbar" => '¦',
        "laquo" => '«',
        "raquo" => '»',
        _ => return None,
    })
}

fn extract_groups(value: &str) -> (Vec<BracketGroup>, String) {
    let mut stack: Vec<(usize, char, Delimiter)> = Vec::new();
    let mut groups = Vec::new();
    for (index, ch) in value.char_indices() {
        if let Some(delimiter) = opening_delimiter(ch) {
            stack.push((index, ch, delimiter));
            continue;
        }
        if let Some(expected) = closing_delimiter(ch) {
            let Some((start, _open, delimiter)) = stack.pop() else {
                continue;
            };
            if expected != delimiter {
                continue;
            }
            if stack.is_empty() {
                let content_start = start + opening_char(delimiter).len_utf8();
                let content_end = index;
                groups.push(BracketGroup {
                    content: value[content_start..content_end].to_owned(),
                    delimiter,
                    start,
                    end: index + ch.len_utf8(),
                    leading: is_leading_group_position(value, start),
                });
            }
        }
    }
    groups.sort_by_key(|group| group.start);
    let mut core = value.to_owned();
    for group in groups.iter().rev() {
        core.replace_range(group.start..group.end, " ");
    }
    (groups, core)
}

fn opening_delimiter(ch: char) -> Option<Delimiter> {
    Some(match ch {
        '[' => Delimiter::Square,
        '(' => Delimiter::Round,
        '{' => Delimiter::Curly,
        '【' | '（' | '［' | '｛' => Delimiter::FullWidth,
        _ => return None,
    })
}

fn is_leading_group_position(value: &str, start: usize) -> bool {
    let prefix = value[..start].trim();
    if prefix.is_empty() {
        return true;
    }
    if prefix.starts_with('[') && prefix.ends_with(']') {
        let previous = prefix[1..prefix.len() - 1].trim();
        if parse_external_id(previous).is_some()
            || is_provider_label(previous)
            || is_release_event(previous)
        {
            return true;
        }
    }
    let upper = prefix.to_ascii_uppercase();
    (prefix.starts_with('(')
        && prefix.ends_with(')')
        && is_release_event(prefix.trim_matches(['(', ')'])))
        || (prefix.starts_with('(') && prefix.ends_with(')'))
        || upper.starts_with("(C")
        || upper.starts_with("(COMITIA")
        || upper.starts_with("(COMIC1")
}

fn closing_delimiter(ch: char) -> Option<Delimiter> {
    Some(match ch {
        ']' => Delimiter::Square,
        ')' => Delimiter::Round,
        '}' => Delimiter::Curly,
        '】' | '）' | '］' | '｝' => Delimiter::FullWidth,
        _ => return None,
    })
}

fn opening_char(delimiter: Delimiter) -> char {
    match delimiter {
        Delimiter::Square => '[',
        Delimiter::Round => '(',
        Delimiter::Curly => '{',
        Delimiter::FullWidth => '【',
    }
}

fn strip_timestamp_suffix(value: &mut String, proposal: &mut NameRoleProposal) {
    let trimmed_end = value.trim_end().len();
    let bytes = value.as_bytes();
    let mut start = trimmed_end;
    while start > 0 && bytes[start - 1].is_ascii_digit() {
        start -= 1;
    }
    let length = trimmed_end.saturating_sub(start);
    if !matches!(length, 10 | 13 | 14) || start == 0 {
        return;
    }
    let separator = value[..start].chars().next_back().unwrap_or_default();
    if !matches!(separator, '_' | '-' | ' ' | '.') {
        return;
    }
    let timestamp = value[start..trimmed_end].to_owned();
    value.replace_range(start - separator.len_utf8()..trimmed_end, "");
    push_tag(proposal, "technical_timestamp_suffix");
    push_evidence(
        proposal,
        "resource",
        &timestamp,
        "filename",
        "technical-timestamp-suffix",
    );
}

fn remove_spans(value: &str, spans: &[(usize, usize)]) -> String {
    let mut out = value.to_owned();
    for (start, end) in spans.iter().rev() {
        if *start < *end && *end <= out.len() {
            out.replace_range(*start..*end, " ");
        }
    }
    out
}

fn merge_spans(mut spans: Vec<(usize, usize)>) -> Vec<(usize, usize)> {
    spans.sort_by_key(|span| span.0);
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (start, end) in spans {
        if let Some(last) = merged.last_mut() {
            if start <= last.1 {
                last.1 = last.1.max(end);
                continue;
            }
        }
        merged.push((start, end));
    }
    merged
}

fn digit_runs(value: &str) -> Vec<(usize, usize, String)> {
    let mut runs = Vec::new();
    let mut chars = value.char_indices().peekable();
    while let Some((start, ch)) = chars.next() {
        if !ch.is_ascii_digit() {
            continue;
        }
        let mut end = start + ch.len_utf8();
        while let Some(&(index, next)) = chars.peek() {
            if next.is_ascii_digit() {
                chars.next();
                end = index + next.len_utf8();
            } else {
                break;
            }
        }
        // Decimal sequence numbers are one token.  This check must happen
        // before range handling and before the next digit run is emitted, so
        // `57.2` cannot degrade into the unrelated integer sequence `2`.
        if value[end..].starts_with('.')
            && value[end + 1..]
                .chars()
                .next()
                .is_some_and(|next| next.is_ascii_digit())
        {
            chars.next();
            end += 1;
            while let Some(&(index, next)) = chars.peek() {
                if next.is_ascii_digit() {
                    chars.next();
                    end = index + next.len_utf8();
                } else {
                    break;
                }
            }
        }
        let mut range_end = end;
        if let Some((dash_index, dash)) = value[range_end..].char_indices().next() {
            if matches!(dash, '-' | '~' | '～') {
                let after_dash = range_end + dash_index + dash.len_utf8();
                if value[after_dash..]
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_digit())
                {
                    let mut cursor = after_dash;
                    while let Some(next) = value[cursor..].chars().next() {
                        if next.is_ascii_digit() {
                            cursor += next.len_utf8();
                        } else {
                            break;
                        }
                    }
                    range_end = cursor;
                }
            }
        }
        runs.push((start, range_end, value[start..range_end].to_owned()));
        while let Some(&(index, next)) = chars.peek() {
            if index < range_end {
                chars.next();
            } else if next.is_ascii_digit() {
                chars.next();
            } else {
                break;
            }
        }
    }
    runs
}

fn first_number(raw: &str) -> String {
    raw.split(['-', '~', '～']).next().unwrap_or(raw).to_owned()
}

fn normalize_sequence_number(value: &str) -> String {
    if let Some((whole, fraction)) = value.split_once('.') {
        if whole.chars().all(|ch| ch.is_ascii_digit())
            && fraction.chars().all(|ch| ch.is_ascii_digit())
        {
            let normalized_whole = whole.parse::<u64>().map(|number| number.to_string());
            if let Ok(normalized_whole) = normalized_whole {
                return format!("{normalized_whole}.{fraction}");
            }
        }
        return value.to_owned();
    }
    if value.chars().all(|ch| ch.is_ascii_digit()) {
        return value
            .parse::<u64>()
            .map(|number| number.to_string())
            .unwrap_or_else(|_| value.to_owned());
    }
    value.to_owned()
}

fn parse_range(raw: &str) -> Option<(String, String)> {
    raw.split_once(['-', '~', '～'])
        .map(|(from, to)| (from.to_owned(), to.to_owned()))
}

fn sequence_axis(after: &str) -> Option<&'static str> {
    let trimmed = after.trim_start();
    let lower = trimmed.to_ascii_lowercase();
    if trimmed.starts_with('季') || lower.starts_with("season") {
        return Some("season");
    }
    if trimmed.starts_with('话')
        || trimmed.starts_with('話')
        || trimmed.starts_with('回')
        || lower.starts_with("chapter")
        || lower.starts_with("episode")
    {
        return Some("chapter");
    }
    None
}

fn continuation_suffix(value: &str) -> Option<(&'static str, &'static str, i32)> {
    let value = value.strip_prefix('+').unwrap_or(value).trim_start();
    for (suffix, relation, rank) in [
        ("续", "continuation", 1),
        ("續", "continuation", 1),
        ("続", "continuation", 1),
        ("后篇", "back_part", 2),
        ("後篇", "back_part", 2),
        ("后编", "back_part", 2),
        ("後編", "back_part", 2),
        ("前篇", "front_part", -1),
        ("前編", "front_part", -1),
    ] {
        if value.starts_with(suffix) {
            return Some((suffix, relation, rank));
        }
    }
    None
}

fn relation_rank(relation: &str) -> i32 {
    match relation {
        "front_part" => -1,
        "continuation" => 1,
        "back_part" | "following_part" => 2,
        "prologue" => -2,
        "epilogue" => 99,
        _ => 0,
    }
}

fn chapter_order_key(
    major: &str,
    relation_rank: i32,
    chapter_title: Option<&str>,
) -> ChapterOrderKey {
    let decimal_minor = major.split_once('.').and_then(|(whole, fraction)| {
        whole.parse::<u32>().ok().and_then(|major| {
            fraction
                .parse::<u32>()
                .ok()
                .map(|minor| (major, minor, fraction.len() as u8))
        })
    });
    let (major_value, decimal_minor_value, decimal_scale) =
        decimal_minor.unwrap_or_else(|| (major.parse().unwrap_or_default(), 0, 0));
    let minor = if decimal_minor.is_some() {
        Some(decimal_minor_value)
    } else {
        chapter_title
            .and_then(|suffix| suffix.trim().chars().next())
            .filter(|ch| ch.is_ascii_alphabetic())
            .map(|ch| ch.to_ascii_lowercase() as u32 - 'a' as u32 + 1)
    };
    ChapterOrderKey {
        major: major_value,
        minor,
        minor_scale: if decimal_minor.is_some() {
            Some(decimal_scale)
        } else {
            minor.map(|_| 26)
        },
        relation_rank: relation_rank as i16,
    }
}

fn cjk_chapter_suffix(value: &str) -> Option<&'static str> {
    for suffix in ["话", "話", "章", "回", "화", "회"] {
        if value.starts_with(suffix) {
            return Some(suffix);
        }
    }
    None
}

fn cjk_volume_suffix(value: &str) -> Option<&'static str> {
    for suffix in ["卷", "巻", "册", "冊"] {
        if value.starts_with(suffix) {
            return Some(suffix);
        }
    }
    None
}

fn single_alpha_suffix(value: &str) -> Option<String> {
    let mut chars = value.trim_start().chars();
    let first = chars.next()?;
    if first.is_ascii_alphabetic() && chars.next().is_none() {
        Some(first.to_string())
    } else {
        None
    }
}

fn preceding_context(value: &str) -> String {
    let trimmed =
        value.trim_end_matches(|c: char| c.is_whitespace() || matches!(c, '.' | ':' | '_'));
    trimmed
        .rsplit(|c: char| c.is_whitespace() || matches!(c, '|' | '/' | '[' | ']' | '(' | ')'))
        .next()
        .unwrap_or(trimmed)
        .to_owned()
}

fn find_all(value: &str, needle: &str) -> Vec<(usize, usize)> {
    value
        .match_indices(needle)
        .map(|(start, _)| (start, start + needle.len()))
        .collect()
}

fn strip_extensionless_token(value: &str) -> String {
    value
        .trim_matches(|c: char| c.is_whitespace() || matches!(c, '_' | '-' | '.' | ':' | '|'))
        .to_owned()
}

fn clean_title_text(value: &str) -> String {
    let mut normalized = value
        .replace('_', " ")
        .replace('.', " ")
        .replace('|', " ")
        .replace('｜', " ")
        .replace('丨', " ")
        .replace('│', " ")
        .replace(':', " ")
        .replace('/', " ");
    normalized = normalized
        .replace('－', " ")
        .replace('–', " ")
        .replace('—', " ");
    normalized = replace_ascii_phrase(&normalized, "one shot");
    normalized = normalized.replace('#', " ");
    normalized
        .split_whitespace()
        .map(strip_extensionless_token)
        .filter(|token| !token.is_empty())
        .filter(|token| !is_noise_label(token))
        .filter(|token| !is_structural_word(token))
        .collect::<Vec<_>>()
        .join(" ")
}

fn split_bilingual_title(value: &str, proposal: &mut NameRoleProposal) -> (String, Vec<String>) {
    let mut parts = value.split(|ch| matches!(ch, '|' | '｜' | '丨' | '│'));
    let primary = clean_title_text(parts.next().unwrap_or_default());
    let aliases = parts
        .filter_map(|part| {
            let cleaned = clean_title_text(part);
            if cleaned.is_empty() {
                return None;
            }
            if is_bilingual_metadata_text(&cleaned) {
                if is_explicit_scanlation_group_text(&cleaned) {
                    add_release_group(proposal, &cleaned);
                    push_evidence(
                        proposal,
                        "release_group",
                        &cleaned,
                        "filename-residual",
                        "bilingual-release-group",
                    );
                }
                return None;
            }
            if is_title_shaped_for_alias(&primary) && is_title_shaped_for_alias(&cleaned) {
                Some(cleaned)
            } else {
                None
            }
        })
        .collect();
    (primary, aliases)
}

fn is_title_shaped_for_alias(value: &str) -> bool {
    let cleaned = clean_title_text(value);
    !cleaned.is_empty()
        && cleaned.chars().count() >= 2
        && cleaned.chars().any(|ch| ch.is_alphabetic())
        && !is_bilingual_metadata_text(&cleaned)
}

fn is_bilingual_metadata_text(value: &str) -> bool {
    let lower = value.trim().to_ascii_lowercase();
    is_explicit_scanlation_group_text(value)
        || is_provider_label(value)
        || is_platform_label(value)
        || is_release_event(value)
        || parse_external_id(value).is_some()
        || is_sequence_label(value)
        || is_numeric_range_token(value)
        || matches!(
            lower.as_str(),
            "raw"
                | "digital"
                | "ebook"
                | "electronic"
                | "sample"
                | "textless"
                | "colorized"
                | "complete"
                | "incomplete"
                | "chinese"
                | "english"
                | "korean"
                | "mtl"
                | "machine translation"
                | "human translation"
        )
        || matches!(
            value.trim(),
            "DL版"
                | "デジタル版"
                | "数字版"
                | "电子版"
                | "電子版"
                | "高画質"
                | "高清"
        )
        || contains_any(
            value,
            &[
                "中国翻訳",
                "中国翻译",
                "無修正",
                "无修正",
                "無修",
                "无修",
                "DL版",
                "デジタル版",
                "机翻",
                "機翻",
                "AI翻訳",
                "AI翻译",
            ],
        )
}

fn strip_special_work_prefix(value: &str, ancestor: &str) -> String {
    let cleaned = clean_title_text(value);
    let ancestor_parts = ancestor
        .split(['-', '－', '–', '—', '_'])
        .map(clean_title_text)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    for prefix in ancestor_parts.iter().rev() {
        let prefix_normalized = normalize_for_compare(prefix);
        let cleaned_normalized = normalize_for_compare(&cleaned);
        if cleaned_normalized == prefix_normalized {
            return String::new();
        }
        if let Some(rest) = cleaned_normalized.strip_prefix(&(prefix_normalized + " ")) {
            let prefix_len = prefix.len();
            if !rest.is_empty()
                && cleaned.is_char_boundary(prefix_len)
                && cleaned[prefix_len..]
                    .chars()
                    .next()
                    .is_some_and(char::is_whitespace)
            {
                return cleaned[prefix_len..].trim().to_owned();
            }
        }
    }
    cleaned
}

fn normalize_publication_title_raw(value: &str) -> String {
    value
        .replace('_', " ")
        .replace('.', " ")
        .replace('|', " ")
        .replace(':', " ")
        .replace('/', " ")
        .replace('－', " ")
        .replace('–', " ")
        .replace('—', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn clean_alias(value: &str) -> String {
    let tokens: Vec<&str> = value.split_whitespace().collect();
    let end = tokens
        .iter()
        .position(|token| is_volume_marker_token(token))
        .unwrap_or(tokens.len());
    clean_title_text(&tokens[..end].join(" "))
}

fn replace_ascii_phrase(value: &str, phrase: &str) -> String {
    let lower = value.to_ascii_lowercase();
    let phrase_lower = phrase.to_ascii_lowercase();
    let mut out = value.to_owned();
    for (start, _) in lower
        .match_indices(&phrase_lower)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
    {
        out.replace_range(start..start + phrase.len(), " ");
    }
    out
}

fn normalize_for_compare(value: &str) -> String {
    clean_title_text(value).to_lowercase()
}

fn looks_like_romanized_alias(candidate: &str, ancestor: &str) -> bool {
    let has_ascii_letters = candidate.chars().any(|ch| ch.is_ascii_alphabetic());
    let ancestor_has_non_ascii = ancestor.chars().any(|ch| !ch.is_ascii());
    let has_multiple_tokens = candidate.split_whitespace().count() >= 2;
    has_ascii_letters && ancestor_has_non_ascii && has_multiple_tokens
}

fn is_structural_only(value: &str) -> bool {
    let cleaned = clean_title_text(value);
    cleaned.is_empty() || cleaned.chars().all(|c| c.is_ascii_digit())
}

fn is_noise_label(value: &str) -> bool {
    let lower = value.trim().to_ascii_lowercase();
    if lower.is_empty() {
        return true;
    }
    [
        "manga",
        "comic",
        "comics",
        "collection",
        "library",
        "downloads",
        "complete",
        "incomplete",
        "digital",
        "ebook",
        "electronic",
        "数字版",
        "raw",
        "scan",
        "webrip",
        "c2c",
        "noads",
        "textless",
        "colorized",
        "sample",
        "lq",
        "low",
        "quality",
        "生肉",
        "熟肉",
        "收藏",
        "分类",
        "分類",
        "书架",
        "書架",
        "完全版",
        "完整版",
        "全集",
        "全卷",
        "单行本",
        "単行本",
    ]
    .iter()
    .any(|term| lower == *term)
        || is_structural_word(value)
        || lower.contains("收藏")
        || lower.contains("书架")
        || lower.contains("書架")
        || lower.contains("漫画收藏")
        || contains_any(
            value,
            &["中国翻訳", "中国翻译", "中国翻譯", "无修正", "無修正"],
        )
}

fn is_structural_word(value: &str) -> bool {
    let lower = value.trim().to_ascii_lowercase();
    [
        "ch",
        "ch.",
        "chapter",
        "episode",
        "ep",
        "vol",
        "vol.",
        "volume",
        "v",
        "tome",
        "part",
        "season",
        "annual",
        "special",
        "extra",
        "tpb",
        "omnibus",
        "compendium",
        "sp",
    ]
    .iter()
    .any(|term| lower == *term)
        || [
            "前篇",
            "前編",
            "后篇",
            "後篇",
            "后编",
            "後編",
            "番外",
            "番外編",
            "外传",
            "外伝",
            "序章",
            "终章",
            "終章",
            "幕间",
            "幕間",
            "第",
            "시즌",
            "외전",
            "외전編",
        ]
        .contains(&value.trim())
}

fn is_provider_label(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    [
        "汉化组",
        "漢化組",
        "翻译组",
        "翻訳組",
        "扫描组",
        "掃描組",
        "出版",
        "scangroup",
        "scanlation",
        "scan group",
        "minutemen",
        "the last kryptonian",
    ]
    .iter()
    .any(|term| lower.contains(&term.to_ascii_lowercase()))
}

fn is_explicit_scanlation_group_text(value: &str) -> bool {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return false;
    }
    let exact_translation_labels = [
        "中国翻訳",
        "中国翻译",
        "中国翻譯",
        "汉化",
        "漢化",
        "翻译",
        "翻訳",
    ];
    if exact_translation_labels.contains(&trimmed) {
        return false;
    }
    let group_suffixes = [
        "汉化组",
        "漢化組",
        "汉化組",
        "漢化组",
        "翻译组",
        "翻訳組",
        "漢化",
        "汉化",
    ];
    if group_suffixes
        .iter()
        .any(|suffix| trimmed.ends_with(suffix) && trimmed.len() > suffix.len())
    {
        return true;
    }
    for marker in ["汉化", "漢化", "翻译", "翻訳"] {
        let Some(index) = trimmed.find(marker) else {
            continue;
        };
        let before = trimmed[..index].trim();
        let after = trimmed[index + marker.len()..].trim();
        if (!before.is_empty() && before != "中国") || !after.is_empty() {
            return true;
        }
    }
    false
}

fn is_platform_label(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    [
        "webtoon",
        "kakao",
        "jmcomic",
        "dlsite",
        "pixiv",
        "booth",
        "腾讯动漫",
        "快看漫画",
        "哔哩哔哩漫画",
        "漫客栈",
        "拷贝漫画",
    ]
    .iter()
    .any(|term| lower.contains(&term.to_ascii_lowercase()))
}

fn is_release_group_marker(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    !is_numeric_range_token(value)
        && (lower == "translator"
            || lower == "release group"
            || lower.contains("scan")
            || lower.contains("group")
            || lower.contains("minutemen")
            || (value.contains('-') && value.split('-').all(|part| !part.trim().is_empty())))
}

fn is_known_source_series(value: &str) -> bool {
    let normalized = normalize_for_compare(value);
    [
        "blue archive",
        "ブルーアーカイブ",
        "五等分の花嫁",
        "アークナイツ",
        "推しの子",
        "アズールレーン",
        "艦隊これくしょん -艦これ-",
        "ダンダダン",
        "その着せ替え人形は恋をする",
        "fate／stay night",
        "fate/stay night",
        "to loveる -とらぶる-",
        "ぼっち・ざ・ろっく!",
        "彼女フェイス",
        "アイドルマスター シャイニーカラーズ",
        "新世紀エヴァンゲリオン",
        "touhou project",
    ]
    .iter()
    .any(|candidate| normalize_for_compare(candidate) == normalized)
}

fn is_numeric_range_token(value: &str) -> bool {
    let trimmed = value.trim();
    parse_range(trimmed).is_some()
        && trimmed
            .chars()
            .all(|ch| ch.is_ascii_digit() || matches!(ch, '-' | '~' | '～' | '.'))
}

fn is_publication_source(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    lower.contains("comic")
        || lower.contains("megastore")
        || lower.contains("magazine")
        || lower.contains("weekly")
        || value.contains("快楽天")
        || lower.contains("tankoubon")
        || lower.contains("単行本")
}

fn is_release_event(value: &str) -> bool {
    let upper = value.to_ascii_uppercase();
    (upper.starts_with("FF") && upper[2..].chars().all(|c| c.is_ascii_digit()))
        || (upper.starts_with('C')
            || upper.starts_with("SC")
            || upper.starts_with("COMITIA")
            || upper.starts_with("COMIC1"))
            && upper.chars().skip(1).all(|c| {
                c.is_ascii_digit()
                    || c == 'I'
                    || c == 'T'
                    || c == 'A'
                    || c == 'O'
                    || c == 'M'
                    || c == 'C'
            })
        || value.contains("例大祭")
}

fn parse_page_count(value: &str) -> Option<String> {
    let trimmed = value.trim();
    let lower = trimmed.to_ascii_lowercase();
    let suffix = if lower.ends_with('p') {
        1
    } else if lower.ends_with("pages") {
        5
    } else {
        return None;
    };
    let number = &trimmed[..trimmed.len().saturating_sub(suffix)];
    if !number.is_empty() && number.chars().all(|ch| ch.is_ascii_digit()) {
        Some(trimmed.to_owned())
    } else {
        None
    }
}

fn parenthetical_part(value: &str) -> Option<(i64, &'static str)> {
    match value.trim() {
        "上" => Some((1, "front_part")),
        "中" => Some((2, "middle_part")),
        "下" => Some((3, "back_part")),
        _ => None,
    }
}

fn contains_collection_marker(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    value.contains("全集")
        || value.contains("全卷")
        || value.contains("全巻")
        || lower.contains("complete collection")
}

fn parenthetical_sequence_members(value: &str) -> Option<Vec<String>> {
    let normalized = value
        .trim()
        .replace('＋', "+")
        .replace('／', "/")
        .replace('、', "+");
    let parts = normalized
        .split(['+', '/'])
        .map(str::trim)
        .collect::<Vec<_>>();
    if parts.len() < 2 || parts.iter().any(|part| part.is_empty()) {
        return None;
    }
    let mut members = Vec::new();
    for part in parts {
        let member = match part {
            "上" => "upper_part",
            "下" => "lower_part",
            _ => return None,
        };
        if !members.iter().any(|item| item == member) {
            members.push(member.to_owned());
        }
    }
    Some(members)
}

fn is_year(value: &str) -> bool {
    value.len() == 4
        && value.chars().all(|c| c.is_ascii_digit())
        && (1900..=2100).contains(&value.parse::<i32>().unwrap_or_default())
}

fn parse_of_range(value: &str) -> Option<String> {
    let lower = value.to_ascii_lowercase();
    let (_, total) = lower.split_once("of")?;
    let total = total
        .trim()
        .trim_start_matches(|c: char| !c.is_ascii_digit());
    let current = lower
        .split_whitespace()
        .find(|token| token.chars().any(|c| c.is_ascii_digit()))?
        .chars()
        .filter(|c| c.is_ascii_digit())
        .collect::<String>();
    let total = total
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>();
    if current.is_empty() || total.is_empty() {
        None
    } else {
        Some(format!("{current}-{total}"))
    }
}

fn parse_external_id(value: &str) -> Option<ExternalIdCandidate> {
    let upper = value.to_ascii_uppercase();
    for prefix in ["RJ", "BJ", "VJ"] {
        if upper.starts_with(prefix) && upper[prefix.len()..].chars().all(|c| c.is_ascii_digit()) {
            return Some(ExternalIdCandidate {
                namespace_hint: "dlsite".into(),
                raw: value.to_owned(),
            });
        }
    }
    None
}

fn is_title_alias_candidate(value: &str) -> bool {
    has_volume_marker(value) || value.split_whitespace().count() >= 3
}

fn has_volume_marker(value: &str) -> bool {
    value.split_whitespace().any(is_volume_marker_token)
}

fn is_volume_marker_token(token: &str) -> bool {
    let lower = token
        .trim_matches(|ch: char| matches!(ch, '[' | ']' | '(' | ')' | ':' | ','))
        .to_ascii_lowercase();
    if matches!(lower.as_str(), "vol" | "vol." | "volume" | "v") {
        return true;
    }
    lower.strip_prefix('v').is_some_and(|suffix| {
        !suffix.is_empty()
            && suffix
                .chars()
                .all(|ch| ch.is_ascii_digit() || matches!(ch, '-' | '~'))
    })
}

fn split_author_title(value: &str) -> Option<(&str, &str)> {
    for separator in [" - ", " – ", " — ", "/", "／"] {
        let Some((left, right)) = value.split_once(separator) else {
            continue;
        };
        let left = left.trim();
        let right = right.trim();
        if !left.is_empty()
            && !right.is_empty()
            && (looks_like_name(left) || looks_like_ascii_name(left))
        {
            return Some((left, right));
        }
    }
    None
}

fn value_after_marker<'a>(value: &'a str, markers: &[&str]) -> Option<&'a str> {
    let lower = value.to_ascii_lowercase();
    markers.iter().find_map(|marker| {
        lower.find(&marker.to_ascii_lowercase()).and_then(|index| {
            let start = index + marker.len();
            let out = value[start..]
                .trim_matches(|c: char| c.is_whitespace() || matches!(c, ':' | '：' | '-' | '–'));
            (!out.is_empty() && start > 0).then_some(out)
        })
    })
}

fn is_explicit_author(value: &str) -> bool {
    let lower = value.trim().to_ascii_lowercase();
    [
        "作者:",
        "作者：",
        "原作:",
        "原作：",
        "作画:",
        "作画：",
        "著者:",
        "著者：",
        "author:",
        "by ",
    ]
    .iter()
    .any(|marker| lower.starts_with(&marker.to_ascii_lowercase()))
}

fn is_author_label_like(value: &str) -> bool {
    let trimmed = value.trim();
    trimmed.contains("作者")
        || trimmed.contains("原作")
        || trimmed.contains("作画")
        || trimmed.contains("画师")
        || trimmed.to_ascii_lowercase().contains("artist")
        || trimmed.to_ascii_lowercase().contains("circle")
        || trimmed.to_ascii_lowercase().contains("author")
}

fn looks_like_name(value: &str) -> bool {
    if value.trim().is_empty() || value.chars().count() > 40 {
        return false;
    }
    let compact: String = value.chars().filter(|c| !c.is_whitespace()).collect();
    let cjk = compact
        .chars()
        .filter(|c| ('\u{3400}'..='\u{9fff}').contains(c))
        .count();
    let kana = compact
        .chars()
        .filter(|c| ('\u{3040}'..='\u{30ff}').contains(c))
        .count();
    if (2..=5).contains(&cjk) && compact.chars().all(|c| !c.is_ascii_digit()) {
        return true;
    }
    if kana >= 2 && compact.chars().all(|c| !c.is_ascii_digit()) {
        return true;
    }
    let words: Vec<&str> = value.split_whitespace().collect();
    words.len() == 2
        && words
            .iter()
            .all(|word| word.chars().all(|c| c.is_ascii_alphabetic()))
}

fn looks_like_ascii_name(value: &str) -> bool {
    let value = value.trim();
    value.len() >= 2
        && value.len() <= 40
        && value.chars().all(|c| c.is_ascii_alphabetic() || c == ' ')
}

fn contains_any(value: &str, terms: &[&str]) -> bool {
    terms.iter().any(|term| value.contains(term))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use serde_json::Value;

    use super::{parse_catalog, CatalogSnapshot, ParseState};

    #[test]
    fn separates_title_author_provider_and_chapter_from_ancestors() {
        let snapshot = CatalogSnapshot::new(
            "webdav|library|manga/One Piece/尾田荣一郎/[漫客栈] One Piece 第1095话.cbz",
            "[漫客栈] One Piece 第1095话.cbz",
            vec!["One Piece".into(), "尾田荣一郎".into(), "漫画收藏".into()],
        );

        let proposal = parse_catalog(&snapshot, 3, "rules-v1");

        assert_eq!(proposal.state, ParseState::Ready);
        assert_eq!(proposal.title.as_deref(), Some("One Piece"));
        assert!(proposal.authors.is_empty());
        assert_eq!(proposal.provider, None);
        assert_eq!(proposal.attribution_candidates[0].name, "漫客栈");
        assert_eq!(proposal.chapter.as_deref(), Some("1095"));
    }

    #[test]
    fn parses_ascii_author_title_provider_and_chapter() {
        let snapshot = CatalogSnapshot::new(
            "local|library|Slam Dunk/Takehiko Inoue/[ScanGroup] Takehiko Inoue - Slam Dunk Ch. 10.cbz",
            "[ScanGroup] Takehiko Inoue - Slam Dunk Ch. 10.cbz",
            vec!["Slam Dunk".into(), "Takehiko Inoue".into(), "Manga".into()],
        );

        let proposal = parse_catalog(&snapshot, 3, "rules-v1");

        assert_eq!(proposal.state, ParseState::Ready);
        assert_eq!(proposal.title.as_deref(), Some("Slam Dunk"));
        assert_eq!(proposal.authors, vec!["Takehiko Inoue"]);
        assert_eq!(proposal.provider, None);
        assert_eq!(proposal.release_groups, vec!["ScanGroup"]);
        assert_eq!(proposal.chapter.as_deref(), Some("10"));
    }

    #[test]
    fn accepts_title_without_author_and_extracts_volume() {
        let snapshot = CatalogSnapshot::new(
            "local|library/Yotsuba/Yotsuba Vol. 1.cbz",
            "Yotsuba Vol. 1.cbz",
            vec!["Yotsuba".into(), "Books".into()],
        );

        let proposal = parse_catalog(&snapshot, 2, "rules-v1");

        assert_eq!(proposal.state, ParseState::Ready);
        assert_eq!(proposal.title.as_deref(), Some("Yotsuba"));
        assert!(proposal.authors.is_empty());
        assert_eq!(proposal.volume.as_deref(), Some("1"));
    }

    #[test]
    fn ancestor_depth_prevents_using_remote_or_unrelated_parent_names() {
        let snapshot = CatalogSnapshot::new(
            "remote|library/Title/Author/Issue.cbz",
            "Issue.cbz",
            vec!["Title".into(), "Author".into()],
        );

        let proposal = parse_catalog(&snapshot, 1, "rules-v1");

        assert_eq!(proposal.title.as_deref(), Some("Issue"));
        assert!(proposal.authors.is_empty());
    }

    #[test]
    fn conflicting_author_candidates_are_marked_ambiguous() {
        let snapshot = CatalogSnapshot::new(
            "local|library/Other Title/Other Author/Some Author - Title.cbz",
            "Some Author - Title.cbz",
            vec!["Other Title".into(), "Other Author".into()],
        );

        let proposal = parse_catalog(&snapshot, 2, "rules-v1");

        assert_eq!(proposal.state, ParseState::Ambiguous);
        assert!(!proposal.conflicts.is_empty());
    }

    #[test]
    fn resource_labels_are_not_authors() {
        let snapshot = CatalogSnapshot::new(
            "fixture/jp-001",
            "[チサキックス (枡田ちさき)] 幼馴染ギャルに好きと言えない陰キャな俺 前編 [中国翻訳] [無修正] [DL版].zip",
            vec![],
        );

        let proposal = parse_catalog(&snapshot, 3, "catalog-rules-v3");

        assert!(!proposal.authors.iter().any(|value| value == "中国翻訳"));
        assert!(!proposal.authors.iter().any(|value| value == "無修正"));
        assert!(!proposal.authors.iter().any(|value| value == "DL版"));
        assert_eq!(
            proposal.title.as_deref(),
            Some("幼馴染ギャルに好きと言えない陰キャな俺")
        );
        assert_eq!(proposal.chapter_title, None);
    }

    #[test]
    fn complete_filename_without_context_is_ready_with_role_separated_authors() {
        let snapshot = CatalogSnapshot::new(
            "fixture/jp-001",
            "[チサキックス (枡田ちさき)] 幼馴染ギャルに好きと言えない陰キャな俺 前編 [中国翻訳] [無修正] [DL版].zip",
            vec![],
        );
        let proposal = parse_catalog(&snapshot, 3, "catalog-rules-v3");
        let json = serde_json::to_value(&proposal).unwrap();

        assert_eq!(proposal.state, ParseState::Ready);
        assert_eq!(proposal.authors, vec!["枡田ちさき"]);
        assert_eq!(json["work_title"], "幼馴染ギャルに好きと言えない陰キャな俺");
        assert_eq!(
            json["publication_title_raw"],
            "幼馴染ギャルに好きと言えない陰キャな俺 前編"
        );
        assert_eq!(json["sequence_kind"], "part");
        assert_eq!(json["part"], 1);
        assert_eq!(json["sequence_label"], "前編");
        assert!(json["chapter_title"].is_null());
        assert_eq!(json["chapter_relation"], "front_part");
        assert_eq!(json["resource_language"], "zh");
        assert_eq!(json["translation_state"], "translated");
        assert!(json["translation_method"].is_null());
        assert!(json["resource_edition"].is_null());
        assert!(json["edition"].is_null());
        assert_eq!(json["censorship"], "uncensored");
        assert!(json["resource_tags"]
            .as_array()
            .is_some_and(|tags| tags.iter().any(|tag| tag == "high_quality")));
        assert!(json["resource_tags"]
            .as_array()
            .is_some_and(|tags| tags.iter().all(|tag| tag != "digital")));
    }

    #[test]
    fn dl_is_high_quality_and_digital_version_markers_are_ignored() {
        let dl = parse_catalog(
            &CatalogSnapshot::new("fixture/dl", "作品名 [DL版].zip", vec![]),
            3,
            "catalog-rules-v3",
        );
        assert_eq!(dl.resource_edition, None);
        assert!(dl.resource_tags.iter().any(|tag| tag == "high_quality"));
        assert!(!dl.resource_tags.iter().any(|tag| tag == "digital"));

        let digital = parse_catalog(
            &CatalogSnapshot::new("fixture/digital", "作品名 [数字版].zip", vec![]),
            3,
            "catalog-rules-v3",
        );
        assert_eq!(digital.resource_edition, None);
        assert!(!digital.resource_tags.iter().any(|tag| tag == "digital"));
        assert!(!digital.resource_tags.iter().any(|tag| tag == "unknown-tag"));
    }

    #[test]
    fn back_part_is_a_part_not_a_special_or_chapter_title() {
        let proposal = parse_catalog(
            &CatalogSnapshot::new("fixture/back", "作品名 后篇.zip", vec![]),
            3,
            "catalog-rules-v3",
        );

        assert_eq!(proposal.sequence_kind.as_deref(), Some("part"));
        assert_eq!(proposal.part, Some(2));
        assert_eq!(proposal.chapter, None);
        assert_eq!(proposal.chapter_title, None);
        assert_eq!(proposal.chapter_relation.as_deref(), Some("back_part"));
        assert!(proposal.sort_key.is_none());
    }

    #[test]
    fn chapter_part_marker_keeps_chapter_and_part_relation() {
        let proposal = parse_catalog(
            &CatalogSnapshot::new("fixture/chapter-part", "作品名 第10話 前編.zip", vec![]),
            3,
            "catalog-rules-v3",
        );

        assert_eq!(proposal.sequence_kind.as_deref(), Some("chapter"));
        assert_eq!(proposal.chapter.as_deref(), Some("10"));
        assert_eq!(proposal.part, Some(1));
        assert_eq!(proposal.chapter_title, None);
        assert_eq!(proposal.chapter_relation.as_deref(), Some("front_part"));
    }

    #[test]
    fn chapter_back_part_keeps_back_relation_and_structured_rank() {
        let proposal = parse_catalog(
            &CatalogSnapshot::new(
                "fixture/chapter-back-part",
                "作品名 第10話 後編.zip",
                vec![],
            ),
            3,
            "catalog-rules-v3",
        );

        assert_eq!(proposal.sequence_kind.as_deref(), Some("chapter"));
        assert_eq!(proposal.chapter.as_deref(), Some("10"));
        assert_eq!(proposal.part, Some(2));
        assert_eq!(proposal.chapter_title, None);
        assert_eq!(proposal.chapter_relation.as_deref(), Some("back_part"));
        assert_eq!(
            proposal.sort_key.as_ref().map(|key| key.relation_rank),
            Some(2)
        );
    }

    #[test]
    fn numeric_continuation_without_title_context_is_partial() {
        let proposal = parse_catalog(
            &CatalogSnapshot::new("fixture/continuation", "10续.zip", vec![]),
            3,
            "catalog-rules-v3",
        );

        assert_eq!(proposal.title, None);
        assert_eq!(proposal.chapter.as_deref(), Some("10"));
        assert_eq!(proposal.chapter_relation.as_deref(), Some("continuation"));
        assert_eq!(proposal.state, ParseState::Partial);
        assert!(proposal.publication_title_raw.is_none());
    }

    #[test]
    fn current_file_is_not_its_own_sibling_evidence() {
        let proposal = parse_catalog(
            &CatalogSnapshot::with_context(
                "10续.zip",
                vec!["作品名"],
                vec!["9.zip", "10.zip", "10续.zip", "11.zip"],
            ),
            3,
            "catalog-rules-v3",
        );
        let evidence = proposal
            .evidence
            .iter()
            .find(|item| item.role == "sequence_context")
            .expect("sibling context evidence should be recorded");

        assert_eq!(evidence.source, "siblings");
        assert!(!evidence.value.contains("10续.zip"));
        assert!(evidence.value.contains("9.zip"));
        assert!(evidence.value.contains("10.zip"));
        assert!(evidence.value.contains("11.zip"));
    }

    #[test]
    fn lettered_chapter_uses_structured_order_without_fractional_numbers() {
        let proposal = parse_catalog(
            &CatalogSnapshot::with_context(
                "Series 153b.cbz",
                vec![],
                vec!["Series 153.cbz", "Series 153a.cbz", "Series 153b.cbz"],
            ),
            3,
            "catalog-rules-v3",
        );

        assert_eq!(proposal.chapter.as_deref(), Some("153"));
        assert_eq!(proposal.chapter_title.as_deref(), Some("b"));
        assert_eq!(
            proposal.sort_key,
            Some(super::ChapterOrderKey {
                major: 153,
                minor: Some(2),
                minor_scale: Some(26),
                relation_rank: 0,
            })
        );
    }

    #[test]
    fn remote_only_catalog_snapshot_never_needs_content_or_source_capabilities() {
        // The parser accepts only persisted catalog text. There is deliberately
        // no ByteSource, downloader, source adapter, or network handle to call.
        let proposal = parse_catalog(
            &CatalogSnapshot::with_context(
                "10续.zip",
                vec!["作品名"],
                vec!["9.zip", "10.zip", "10续.zip", "11.zip"],
            ),
            3,
            "catalog-rules-v3",
        );

        assert_eq!(proposal.state, ParseState::Ready);
        assert_eq!(proposal.work_title.as_deref(), Some("作品名"));
        assert_eq!(proposal.chapter.as_deref(), Some("10"));
        assert_eq!(proposal.chapter_relation.as_deref(), Some("continuation"));
    }

    #[test]
    fn translation_label_does_not_invent_translation_method() {
        let proposal = parse_catalog(
            &CatalogSnapshot::new("fixture/translation", "作品名 [中国翻译].zip", vec![]),
            3,
            "catalog-rules-v3",
        );
        let json = serde_json::to_value(&proposal).unwrap();

        assert_eq!(json["resource_language"], "zh");
        assert_eq!(json["translation_state"], "translated");
        assert!(json["translation_method"].is_null());
    }

    #[test]
    fn explicit_machine_translation_sets_method() {
        let proposal = parse_catalog(
            &CatalogSnapshot::new(
                "fixture/machine-translation",
                "作品名 [中国翻译] [机翻].zip",
                vec![],
            ),
            3,
            "catalog-rules-v3",
        );
        assert_eq!(proposal.translation_method.as_deref(), Some("machine"));
        assert_eq!(proposal.translation_state.as_deref(), Some("translated"));
    }

    #[test]
    fn explicit_human_translation_sets_method() {
        let proposal = parse_catalog(
            &CatalogSnapshot::new(
                "fixture/human-translation",
                "作品名 [中国翻译] [人工翻译].zip",
                vec![],
            ),
            3,
            "catalog-rules-v3",
        );
        assert_eq!(proposal.translation_method.as_deref(), Some("human"));
        assert_eq!(proposal.translation_state.as_deref(), Some("translated"));
    }

    #[test]
    fn unknown_bracket_is_not_default_creator() {
        let proposal = parse_catalog(
            &CatalogSnapshot::new("fixture/unknown-bracket", "[Unknown] Work 01.zip", vec![]),
            3,
            "catalog-rules-v3",
        );
        assert!(proposal.creators.is_empty());
        assert!(proposal
            .evidence
            .iter()
            .any(|item| item.role == "unknown_tag"));
    }

    #[test]
    fn unknown_leading_attribution_does_not_make_core_identity_ambiguous() {
        let proposal = parse_catalog(
            &CatalogSnapshot::new(
                "fixture/unknown-attribution",
                "[Been] 恋人に知られちゃいけないこと 2 [中国语] [無修正].cbz",
                vec![],
            ),
            3,
            "catalog-rules-v3",
        );

        assert_eq!(proposal.state, ParseState::Ready);
        assert_eq!(
            proposal.work_title.as_deref(),
            Some("恋人に知られちゃいけないこと")
        );
        assert!(proposal.creators.is_empty());
        assert!(proposal.conflicts.is_empty());
        assert!(proposal
            .evidence
            .iter()
            .any(|item| item.role == "unknown_tag" && item.value == "Been"));
        assert_eq!(proposal.attribution_candidates[0].name, "Been");
        assert!(proposal
            .warnings
            .iter()
            .any(|warning| warning == "unresolved_leading_attribution"));
    }

    #[test]
    fn ancestor_semantics_skip_format_media_and_publication_buckets() {
        let proposal = parse_catalog(
            &CatalogSnapshot::with_context(
                "9.epub",
                vec!["EPUB", "单行本", "漫画", "W-舞冰的祈愿-金牌得主"],
                vec!["8.epub", "9.epub", "10.epub"],
            ),
            4,
            "catalog-rules-v3",
        );

        assert_eq!(
            proposal.work_title.as_deref(),
            Some("W-舞冰的祈愿-金牌得主")
        );
        assert_eq!(proposal.volume.as_deref(), Some("9"));
        assert_eq!(proposal.sequence_kind.as_deref(), Some("volume"));
        assert!(proposal.authors.is_empty());
    }

    #[test]
    fn decimal_chapter_is_atomic_and_structured() {
        let proposal = parse_catalog(
            &CatalogSnapshot::new(
                "fixture/decimal",
                "57.2 第57.2话.pdf",
                vec!["连载".into(), "漫画".into(), "作品名".into()],
            ),
            3,
            "catalog-rules-v3",
        );

        assert_eq!(proposal.chapter.as_deref(), Some("57.2"));
        assert_eq!(proposal.sequence_kind.as_deref(), Some("chapter"));
        assert_eq!(proposal.work_title.as_deref(), Some("作品名"));
        assert_eq!(
            proposal.sort_key,
            Some(super::ChapterOrderKey {
                major: 57,
                minor: Some(2),
                minor_scale: Some(1),
                relation_rank: 0,
            })
        );
    }

    #[test]
    fn decimal_filename_with_serial_context_is_a_chapter() {
        let proposal = parse_catalog(
            &CatalogSnapshot::with_context(
                "57.2.pdf",
                vec!["连载", "漫画", "作品名"],
                vec!["56.pdf", "57.pdf", "57.2.pdf", "58.pdf", "58.2.pdf"],
            ),
            4,
            "catalog-rules-v3",
        );

        assert_eq!(proposal.work_title.as_deref(), Some("作品名"));
        assert_eq!(proposal.chapter.as_deref(), Some("57.2"));
        assert_eq!(proposal.sequence_kind.as_deref(), Some("chapter"));
    }

    #[test]
    fn decimal_precision_is_preserved_in_sort_key() {
        let proposal = parse_catalog(
            &CatalogSnapshot::new("fixture/decimal-precision", "12.25.pdf", vec![]),
            3,
            "catalog-rules-v3",
        );

        assert_eq!(proposal.chapter.as_deref(), Some("12.25"));
        assert_eq!(
            proposal.sort_key,
            Some(super::ChapterOrderKey {
                major: 12,
                minor: Some(25),
                minor_scale: Some(2),
                relation_rank: 0,
            })
        );
    }

    #[test]
    fn plain_numeric_filename_uses_single_volume_context() {
        let proposal = parse_catalog(
            &CatalogSnapshot::with_context(
                "9.epub",
                vec!["EPUB", "单行本", "漫画", "作品名"],
                vec!["8.epub", "9.epub", "10.epub"],
            ),
            4,
            "catalog-rules-v3",
        );

        assert_eq!(proposal.volume.as_deref(), Some("9"));
        assert_eq!(proposal.sequence_kind.as_deref(), Some("volume"));
        assert_eq!(proposal.issue, None);
    }

    #[test]
    fn language_aliases_do_not_invent_translation_state() {
        let proposal = parse_catalog(
            &CatalogSnapshot::new("fixture/language", "作品名 [中国語] [中文].zip", vec![]),
            3,
            "catalog-rules-v3",
        );

        assert_eq!(proposal.resource_language.as_deref(), Some("zh"));
        assert_eq!(proposal.translation_state, None);
        assert_eq!(proposal.translation_method, None);
    }

    #[test]
    fn translated_chinese_aliases_set_translation_state() {
        let proposal = parse_catalog(
            &CatalogSnapshot::new("fixture/translated-language", "作品名 [简中].zip", vec![]),
            3,
            "catalog-rules-v3",
        );

        assert_eq!(proposal.resource_language.as_deref(), Some("zh"));
        assert_eq!(proposal.translation_state.as_deref(), Some("translated"));
        assert_eq!(proposal.translation_method, None);
    }

    #[test]
    fn nested_creator_grammar_precedes_provider_and_title_split() {
        let proposal = parse_catalog(
            &CatalogSnapshot::new(
                "fixture/nested-creator",
                "[きょくちょ局 (きょくちょ)] エルフ教育。 -亡国のミスト- 丨 精灵教育 - 亡国的蜜斯特 [中国語].cbz",
                vec![],
            ),
            3,
            "catalog-rules-v3",
        );

        assert_eq!(proposal.provider, None);
        assert_eq!(
            proposal
                .creators
                .iter()
                .map(|creator| creator.name.as_str())
                .collect::<Vec<_>>(),
            vec!["きょくちょ局", "きょくちょ"]
        );
        assert_eq!(proposal.authors, vec!["きょくちょ"]);
        assert_eq!(
            proposal.work_title.as_deref(),
            Some("エルフ教育。 亡国のミスト")
        );
        assert_eq!(proposal.title_aliases, vec!["精灵教育 亡国的蜜斯特"]);
    }

    #[test]
    fn numeric_range_is_not_a_provider() {
        let proposal = parse_catalog(
            &CatalogSnapshot::new("fixture/range", "作品名 1-4.zip", vec![]),
            3,
            "catalog-rules-v3",
        );

        assert_eq!(proposal.provider, None);
        assert!(proposal.release_groups.is_empty());
    }

    #[test]
    fn chapter_only_filename_uses_parent_title() {
        let snapshot = CatalogSnapshot::with_context(
            "10续.zip",
            vec!["作品名"],
            vec!["9.zip", "10.zip", "10续.zip", "11.zip"],
        );

        let proposal = parse_catalog(&snapshot, 3, "catalog-rules-v3");

        assert_eq!(proposal.title.as_deref(), Some("作品名"));
        assert_eq!(proposal.chapter.as_deref(), Some("10"));
        assert_eq!(proposal.chapter_relation.as_deref(), Some("continuation"));
        assert!(proposal.publication_title_raw.is_none());
        assert_eq!(
            proposal.sort_key,
            Some(super::ChapterOrderKey {
                major: 10,
                minor: None,
                minor_scale: None,
                relation_rank: 1,
            })
        );
    }

    #[test]
    fn technical_timestamp_suffix_does_not_become_title() {
        let snapshot = CatalogSnapshot::with_context(
            "19话_20190923103738.zip",
            vec!["作品名"],
            vec!["18话_20190923103700.zip", "19话_20190923103738.zip"],
        );

        let proposal = parse_catalog(&snapshot, 3, "catalog-rules-v3");

        assert_eq!(proposal.title.as_deref(), Some("作品名"));
        assert_eq!(proposal.chapter.as_deref(), Some("19"));
        assert!(proposal
            .resource_tags
            .iter()
            .any(|tag| tag == "technical_timestamp_suffix"));
    }

    #[test]
    fn separates_event_source_series_year_and_release_group() {
        let snapshot = CatalogSnapshot::new(
            "fixture/eh-001",
            "(C100) [ABC (XYZ)] Some Title (Blue Archive) (2016) (Group).zip",
            vec![],
        );
        let proposal = parse_catalog(&snapshot, 3, "catalog-rules-v3");

        assert_eq!(proposal.title.as_deref(), Some("Some Title"));
        assert_eq!(proposal.release_event.as_deref(), Some("C100"));
        assert_eq!(proposal.source_series, vec!["Blue Archive"]);
        assert_eq!(proposal.publication_year.as_deref(), Some("2016"));
        assert_eq!(proposal.release_groups, vec!["Group"]);
    }

    #[test]
    fn external_id_is_not_a_chapter_number() {
        let snapshot = CatalogSnapshot::new(
            "fixture/dlsite-001",
            "[RJ01234567][Circle (Artist)] Title.zip",
            vec![],
        );
        let proposal = parse_catalog(&snapshot, 3, "catalog-rules-v3");

        assert_eq!(proposal.title.as_deref(), Some("Title"));
        assert!(proposal.chapter.is_none());
        assert_eq!(proposal.external_id_candidates[0].namespace_hint, "dlsite");
    }

    #[test]
    fn complete_is_resource_completeness_not_work_status() {
        let snapshot =
            CatalogSnapshot::new("fixture/complete", "[Complete] Work Vol 1-4.cbz", vec![]);
        let proposal = parse_catalog(&snapshot, 3, "catalog-rules-v3");

        assert_eq!(proposal.resource_completeness.as_deref(), Some("complete"));
        assert_eq!(proposal.title.as_deref(), Some("Work"));
        assert!(!proposal.resource_tags.is_empty());
    }

    #[test]
    fn html_entities_are_decoded_before_sequence_tokenization() {
        let proposal = parse_catalog(
            &CatalogSnapshot::new(
                "fixture/html-entity",
                "お隣さんは陰キャっぽいのに隠れビッチ &#124; 中文标题.zip",
                vec![],
            ),
            3,
            "catalog-rules-v3",
        );

        assert_ne!(proposal.issue.as_deref(), Some("124"));
        assert_eq!(
            proposal.title.as_deref(),
            Some("お隣さんは陰キャっぽいのに隠れビッチ")
        );
        assert_eq!(proposal.title_aliases, vec!["中文标题"]);

        let named = parse_catalog(
            &CatalogSnapshot::new("fixture/html-named", "作品&nbsp;&amp;副标题.zip", vec![]),
            3,
            "catalog-rules-v3",
        );
        assert_eq!(named.title.as_deref(), Some("作品 &副标题"));
    }

    #[test]
    fn special_filename_prefers_ancestor_work_and_keeps_special_title() {
        let proposal = parse_catalog(
            &CatalogSnapshot::with_context(
                "番外篇 冰焰之兽.pdf",
                vec!["番外", "漫画", "W-舞冰的祈愿-金牌得主"],
                vec![],
            ),
            3,
            "catalog-rules-v3",
        );

        assert_eq!(
            proposal.work_title.as_deref(),
            Some("W-舞冰的祈愿-金牌得主")
        );
        assert_eq!(proposal.sequence_kind.as_deref(), Some("special"));
        assert_eq!(proposal.chapter_relation.as_deref(), Some("side_story"));
        let json = serde_json::to_value(&proposal).unwrap();
        assert_eq!(json["special_title"], "冰焰之兽");

        let prefixed = parse_catalog(
            &CatalogSnapshot::with_context(
                "金牌得主_番外篇 冰焰之兽.pdf",
                vec!["连载", "漫画", "W-舞冰的祈愿-金牌得主"],
                vec![],
            ),
            3,
            "catalog-rules-v3",
        );
        assert_eq!(
            prefixed.work_title.as_deref(),
            Some("W-舞冰的祈愿-金牌得主")
        );
        assert_eq!(prefixed.special_title.as_deref(), Some("冰焰之兽"));
    }

    #[test]
    fn leading_platform_name_remains_unresolved_attribution() {
        let proposal = parse_catalog(
            &CatalogSnapshot::new(
                "fixture/kakao",
                "[kakao] エンジェリック・カズン.zip",
                vec![],
            ),
            3,
            "catalog-rules-v3",
        );

        assert_eq!(proposal.provider, None);
        assert_eq!(proposal.distribution_platform, None);
        assert!(proposal.release_groups.is_empty());
        assert_eq!(proposal.attribution_candidates[0].name, "kakao");
    }

    #[test]
    fn explicit_scanlation_group_is_retained_without_becoming_provider() {
        let proposal = parse_catalog(
            &CatalogSnapshot::new(
                "fixture/scanlation-group",
                "[無邪気漢化組] 作品名 [無修正].cbz",
                vec![],
            ),
            3,
            "catalog-rules-v3",
        );

        assert_eq!(proposal.provider, None);
        assert_eq!(proposal.release_groups, vec!["無邪気漢化組"]);
        assert_eq!(proposal.translation_state.as_deref(), Some("translated"));
    }

    #[test]
    fn bilingual_release_signature_is_not_a_title_alias() {
        let proposal = parse_catalog(
            &CatalogSnapshot::new(
                "fixture/bilingual-release-signature",
                "姉トモ レイナさん｜漢化組漢化組x我尻故我在＃45.cbz",
                vec![],
            ),
            3,
            "catalog-rules-v3",
        );

        assert!(proposal.title_aliases.is_empty());
        assert!(proposal
            .release_groups
            .iter()
            .any(|group| group == "漢化組漢化組x我尻故我在＃45"));
        assert_eq!(proposal.provider, None);
    }

    #[test]
    fn special_ancestor_residual_is_not_an_alias() {
        let proposal = parse_catalog(
            &CatalogSnapshot::with_context(
                "金牌得主_番外篇 Novice-A男子组.pdf",
                vec!["番外", "漫画", "W-舞冰的祈愿-金牌得主"],
                vec![],
            ),
            3,
            "catalog-rules-v3",
        );

        assert_eq!(
            proposal.work_title.as_deref(),
            Some("W-舞冰的祈愿-金牌得主")
        );
        assert_eq!(proposal.special_title.as_deref(), Some("Novice-A男子组"));
        assert!(proposal.title_aliases.is_empty());
    }

    #[test]
    fn unknown_parenthetical_is_a_context_candidate_not_source_series() {
        let proposal = parse_catalog(
            &CatalogSnapshot::new(
                "fixture/unknown-parenthetical",
                "作品名 (しぐれうい).zip",
                vec![],
            ),
            3,
            "catalog-rules-v3",
        );

        assert!(proposal.source_series.is_empty());
        let json = serde_json::to_value(&proposal).unwrap();
        assert_eq!(
            json["source_context_candidates"],
            serde_json::json!(["しぐれうい"])
        );
        assert!(proposal
            .warnings
            .iter()
            .any(|warning| warning == "unresolved_parenthetical_context"));
    }

    #[test]
    fn parenthesis_roles_use_resource_event_and_publication_precedence() {
        let page = parse_catalog(
            &CatalogSnapshot::new("fixture/page-count", "作品名 (106p).zip", vec![]),
            3,
            "catalog-rules-v3",
        );
        assert!(page.source_series.is_empty());
        assert!(page.resource_tags.iter().any(|tag| tag == "page_count"));

        let censorship = parse_catalog(
            &CatalogSnapshot::new("fixture/uncensored-short", "作品名 (無修).zip", vec![]),
            3,
            "catalog-rules-v3",
        );
        assert_eq!(censorship.censorship.as_deref(), Some("uncensored"));
        assert!(censorship.source_series.is_empty());

        let part = parse_catalog(
            &CatalogSnapshot::new("fixture/down-part", "作品名 (下).zip", vec![]),
            3,
            "catalog-rules-v3",
        );
        assert_eq!(part.sequence_kind.as_deref(), Some("part"));
        assert_eq!(part.part, Some(3));
        assert!(part.source_series.is_empty());

        let event = parse_catalog(
            &CatalogSnapshot::new("fixture/ff-event", "作品名 (FF45).zip", vec![]),
            3,
            "catalog-rules-v3",
        );
        assert_eq!(event.release_event.as_deref(), Some("FF45"));
        assert!(event.source_series.is_empty());

        let publication = parse_catalog(
            &CatalogSnapshot::new(
                "fixture/weekly-source",
                "作品名 (WEEKLY快楽天 2025 No.16).zip",
                vec![],
            ),
            3,
            "catalog-rules-v3",
        );
        assert_eq!(
            publication.publication_source.as_deref(),
            Some("WEEKLY快楽天 2025 No.16")
        );
        assert!(publication.source_series.is_empty());
    }

    #[test]
    fn composite_sequence_is_consumed_before_single_issue_scan() {
        for (filename, expected_title, expected_range) in [
            ("ゲーミング彼女 1+2.zip", "ゲーミング彼女", false),
            (
                "TSあきら君の性生活総集編 (1-6)+7+8+9.zip",
                "TSあきら君の性生活総集編",
                true,
            ),
            ("作品名 m1-m50.zip", "作品名", true),
            ("作品名 01-4.5.zip", "作品名", true),
            ("作品名 1-9+番外.zip", "作品名", true),
            ("作品名 1-3+特典.zip", "作品名", true),
            ("作品名 Ⅰ-Ⅵ+特典.zip", "作品名", true),
        ] {
            let proposal = parse_catalog(
                &CatalogSnapshot::new("fixture/composite", filename, vec![]),
                3,
                "catalog-rules-v3",
            );
            assert_eq!(
                proposal.title.as_deref(),
                Some(expected_title),
                "title for {filename}"
            );
            assert_eq!(proposal.issue, None, "single issue stolen from {filename}");
            assert_eq!(
                proposal.range.is_some(),
                expected_range,
                "range for {filename}"
            );
            assert!(
                !proposal.sequence_members.is_empty(),
                "members missing for {filename}"
            );
        }
    }

    #[test]
    fn bilingual_separator_creates_title_alias() {
        let proposal = parse_catalog(
            &CatalogSnapshot::new(
                "fixture/bilingual",
                "ヒミツの睡眠学習｜祕密的睡眠學習.zip",
                vec![],
            ),
            3,
            "catalog-rules-v3",
        );

        assert_eq!(proposal.work_title.as_deref(), Some("ヒミツの睡眠学習"));
        assert_eq!(proposal.title_aliases, vec!["祕密的睡眠學習"]);
    }

    #[test]
    fn unresolved_no_number_is_not_silently_consumed() {
        let proposal = parse_catalog(
            &CatalogSnapshot::new(
                "fixture/no-number",
                "[MANA] NO.41 短标题 [中国翻译].cbz",
                vec![],
            ),
            3,
            "catalog-rules-v3",
        );

        assert!(proposal.issue.is_none());
        assert!(proposal
            .work_title
            .as_deref()
            .is_some_and(|title| title.contains("NO") && title.contains("41")));
        assert!(proposal.numeric_labels.iter().any(|label| {
            label.prefix == "NO"
                && label.value == "41"
                && label.semantic_role == "unresolved"
                && label.raw == "NO.41"
        }));
        assert!(proposal
            .warnings
            .iter()
            .any(|warning| warning == "unresolved_numeric_label"));
    }

    #[test]
    fn parenthetical_numeric_is_unresolved_without_sibling_support() {
        let proposal = parse_catalog(
            &CatalogSnapshot::new(
                "fixture/parenthetical-number",
                "The illusion of lies (1).cbz",
                vec![],
            ),
            3,
            "catalog-rules-v3",
        );

        assert_eq!(proposal.issue, None);
        assert_eq!(proposal.title.as_deref(), Some("The illusion of lies"));
        assert!(proposal.numeric_labels.iter().any(|label| {
            label.value == "1" && label.semantic_role == "unresolved" && label.raw == "1"
        }));
        assert!(proposal
            .warnings
            .iter()
            .any(|warning| warning == "unresolved_numeric_label"));

        let full_width = parse_catalog(
            &CatalogSnapshot::new(
                "fixture/full-width-parenthetical-number",
                "The illusion of lies（1）.cbz",
                vec![],
            ),
            3,
            "catalog-rules-v3",
        );
        assert!(full_width.numeric_labels.iter().any(|label| {
            label.value == "1" && label.semantic_role == "unresolved" && label.raw == "1"
        }));
    }

    #[test]
    fn parenthetical_numeric_with_matching_siblings_promotes_to_issue() {
        let proposal = parse_catalog(
            &CatalogSnapshot::with_context(
                "The illusion of lies (2).cbz",
                vec![],
                vec![
                    "The illusion of lies (1).cbz",
                    "The illusion of lies (2).cbz",
                    "The illusion of lies (3).cbz",
                ],
            ),
            3,
            "catalog-rules-v3",
        );

        assert_eq!(proposal.issue.as_deref(), Some("2"));
        assert_eq!(proposal.sequence_kind.as_deref(), Some("issue"));
        assert_eq!(proposal.title.as_deref(), Some("The illusion of lies"));
        assert!(!proposal
            .warnings
            .iter()
            .any(|warning| warning == "unresolved_numeric_label"));
    }

    #[test]
    fn front_and_back_parts_are_retained_as_a_composite() {
        let proposal = parse_catalog(
            &CatalogSnapshot::new(
                "fixture/front-back",
                "故事名【前編】【後編】 (Source).cbz",
                vec![],
            ),
            3,
            "catalog-rules-v3",
        );

        assert_eq!(proposal.sequence_kind.as_deref(), Some("part"));
        assert_eq!(proposal.part, None);
        assert!(proposal.is_collection);
        assert_eq!(
            proposal.sequence_members,
            vec!["front_part".to_owned(), "back_part".to_owned()]
        );
    }

    #[test]
    fn attached_terminal_number_needs_context_before_becoming_issue() {
        let proposal = parse_catalog(
            &CatalogSnapshot::new("fixture/attached-number", "桜春女学院の男剣2.cbz", vec![]),
            3,
            "catalog-rules-v3",
        );

        assert_eq!(proposal.issue, None);
        assert!(proposal
            .work_title
            .as_deref()
            .is_some_and(|title| title.ends_with('2')));
    }

    #[test]
    fn attached_terminal_number_with_siblings_is_an_issue() {
        let proposal = parse_catalog(
            &CatalogSnapshot::with_context(
                "美容師さんは指名で？2.cbz",
                vec![],
                vec![
                    "美容師さんは指名で？1.cbz",
                    "美容師さんは指名で？2.cbz",
                    "美容師さんは指名で？3.cbz",
                ],
            ),
            3,
            "catalog-rules-v3",
        );

        assert_eq!(proposal.issue.as_deref(), Some("2"));
        assert_eq!(proposal.title.as_deref(), Some("美容師さんは指名で？"));
    }

    #[test]
    fn plus_continuation_is_a_chapter_and_never_a_title() {
        let proposal = parse_catalog(
            &CatalogSnapshot::with_context(
                "06+续.zip",
                vec!["作品名"],
                vec!["05.zip", "06+续.zip", "07.zip", "10续.zip"],
            ),
            4,
            "catalog-rules-v3",
        );
        assert_eq!(proposal.work_title.as_deref(), Some("作品名"));
        assert_eq!(proposal.chapter.as_deref(), Some("6"));
        assert_eq!(proposal.sequence_kind.as_deref(), Some("chapter"));
        assert_eq!(proposal.chapter_relation.as_deref(), Some("continuation"));
        assert_eq!(proposal.state, ParseState::Ready);

        let plain = parse_catalog(
            &CatalogSnapshot::with_context(
                "06.zip",
                vec!["作品名"],
                vec!["05.zip", "06.zip", "06+续.zip", "07.zip"],
            ),
            4,
            "catalog-rules-v3",
        );
        assert_eq!(plain.chapter.as_deref(), Some("6"));
        assert_eq!(plain.sequence_kind.as_deref(), Some("chapter"));

        let unresolved = parse_catalog(
            &CatalogSnapshot::new("fixture/continuation-only", "06+续.zip", vec![]),
            4,
            "catalog-rules-v3",
        );
        assert_eq!(unresolved.work_title, None);
        assert_eq!(unresolved.state, ParseState::Partial);
    }

    #[test]
    fn ancestor_title_reuses_leading_attribution_grammar() {
        let proposal = parse_catalog(
            &CatalogSnapshot::new(
                "fixture/ancestor-attribution",
                "01.zip",
                vec!["[Vchan]我的合租女室友是不是过于淫荡了？".into()],
            ),
            4,
            "catalog-rules-v3",
        );

        assert_eq!(
            proposal.work_title.as_deref(),
            Some("我的合租女室友是不是过于淫荡了？")
        );
        assert!(proposal
            .attribution_candidates
            .iter()
            .any(|candidate| candidate.name == "Vchan"));
        assert_eq!(
            proposal
                .attribution_candidates
                .iter()
                .find(|candidate| candidate.name == "Vchan")
                .map(|candidate| candidate.source.as_str()),
            Some("ancestor(0)")
        );
        assert!(!proposal
            .work_title
            .as_deref()
            .unwrap_or_default()
            .contains("[Vchan]"));
    }

    #[test]
    fn upper_lower_part_marker_is_sequence_evidence() {
        let proposal = parse_catalog(
            &CatalogSnapshot::with_context(
                "05（上+下）.zip",
                vec!["作品名"],
                vec!["04（上+下）.zip", "05（上+下）.zip", "06+续.zip"],
            ),
            4,
            "catalog-rules-v3",
        );

        assert_eq!(proposal.chapter.as_deref(), Some("5"));
        assert_eq!(proposal.sequence_kind.as_deref(), Some("chapter"));
        assert!(proposal
            .sequence_members
            .iter()
            .any(|member| member == "upper_part"));
        assert!(proposal
            .sequence_members
            .iter()
            .any(|member| member == "lower_part"));
        assert!(!proposal
            .source_context_candidates
            .iter()
            .any(|candidate| candidate == "上+下"));
    }

    #[test]
    fn complete_collection_markers_are_distinguished_from_end_hint() {
        let collection = parse_catalog(
            &CatalogSnapshot::new(
                "fixture/complete-collection",
                "富家女姐姐 1-137 全集.zip",
                vec![],
            ),
            4,
            "catalog-rules-v3",
        );
        assert_eq!(
            collection.resource_completeness.as_deref(),
            Some("complete")
        );
        assert!(collection.is_collection);

        let composite = parse_catalog(
            &CatalogSnapshot::new(
                "fixture/complete-composite",
                "洞玄尘心道归真01-07 [全集无修正].zip",
                vec![],
            ),
            4,
            "catalog-rules-v3",
        );
        assert_eq!(composite.resource_completeness.as_deref(), Some("complete"));
        assert_eq!(composite.censorship.as_deref(), Some("uncensored"));
        assert!(composite.is_collection);

        let end_hint = parse_catalog(
            &CatalogSnapshot::new("fixture/end-hint", "What Happened 1-10 [End].zip", vec![]),
            4,
            "catalog-rules-v3",
        );
        assert_eq!(end_hint.resource_completeness.as_deref(), Some("complete"));
        assert!(!end_hint.is_collection);
    }

    #[test]
    fn full_color_marker_sets_full_color_state() {
        let proposal = parse_catalog(
            &CatalogSnapshot::new("fixture/full-color", "色恋桜【フルカラー版】.zip", vec![]),
            4,
            "catalog-rules-v3",
        );

        assert_eq!(proposal.color_state.as_deref(), Some("full_color"));
        assert!(!proposal
            .source_context_candidates
            .iter()
            .any(|candidate| candidate == "フルカラー版"));
    }

    #[test]
    fn full_catalog_path_is_not_retained_as_work_title() {
        let proposal = parse_catalog(
            &CatalogSnapshot::new(
                "fixture/115",
                "115:/日漫/[Alice Crazy] What Happened.zip",
                vec!["日漫".into()],
            ),
            4,
            "catalog-rules-v3",
        );
        assert!(!proposal
            .work_title
            .as_deref()
            .unwrap_or_default()
            .contains(".zip"));
        assert!(!proposal
            .work_title
            .as_deref()
            .unwrap_or_default()
            .contains("115:"));
    }

    #[test]
    fn context_prefix_does_not_block_nested_creator_grammar() {
        let proposal = parse_catalog(
            &CatalogSnapshot::new(
                "fixture/event-prefix",
                "(アズレン夢想) [CAT GARDEN (ねこてゐ)] 碧藍射爆 (アズールレーン).zip",
                vec![],
            ),
            4,
            "catalog-rules-v3",
        );
        assert!(proposal
            .creators
            .iter()
            .any(|creator| creator.name == "CAT GARDEN" && creator.role == "circle"));
        assert!(proposal
            .creators
            .iter()
            .any(|creator| creator.name == "ねこてゐ" && creator.role == "artist"));
        assert_eq!(proposal.work_title.as_deref(), Some("碧藍射爆"));
    }

    #[test]
    fn season_and_chapter_ranges_are_typed_separately() {
        let proposal = parse_catalog(
            &CatalogSnapshot::new(
                "fixture/multi-axis",
                "[Alice Crazy] 一個變態的日常生活 第1-4季 第1-144話[完結].zip",
                vec![],
            ),
            4,
            "catalog-rules-v3",
        );
        assert_eq!(proposal.season_range.as_deref(), Some("1-4"));
        assert_eq!(proposal.chapter_range.as_deref(), Some("1-144"));
        assert!(proposal.is_collection);
        assert_eq!(proposal.resource_completeness.as_deref(), Some("complete"));
        assert_eq!(proposal.chapter, None);
        assert_eq!(proposal.season, None);
    }

    #[test]
    fn rating_number_is_not_an_issue() {
        for filename in ["作品名 评分5.zip", "作品名 評価5.zip"] {
            let proposal = parse_catalog(
                &CatalogSnapshot::new("fixture/rating", filename, vec![]),
                3,
                "catalog-rules-v3",
            );
            assert_eq!(proposal.issue, None, "rating parsed as issue: {filename}");
            assert!(proposal
                .work_title
                .as_deref()
                .is_some_and(|title| title.ends_with('5')));
        }
    }

    #[test]
    fn matching_bilingual_terminal_numbers_support_issue_and_strip_both_sides() {
        let proposal = parse_catalog(
            &CatalogSnapshot::new("fixture/bilingual-number", "作品名2｜作品名2.cbz", vec![]),
            3,
            "catalog-rules-v3",
        );

        assert_eq!(proposal.issue.as_deref(), Some("2"));
        assert_eq!(proposal.work_title.as_deref(), Some("作品名"));
        assert_eq!(proposal.title_aliases, vec!["作品名"]);
    }

    #[test]
    fn golden_corpus_seed_matches_semantic_projections() {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let candidates = [
            manifest_dir.join(
                "../../.trellis/tasks/08-23-m8-catalog-rules-v3-design/corpus/catalog-rules-v3-golden.jsonl",
            ),
            manifest_dir.join(
                "../../.trellis/tasks/archive/2026-08/08-23-m8-catalog-rules-v3-design/corpus/catalog-rules-v3-golden.jsonl",
            ),
        ];
        let path = candidates
            .iter()
            .find(|candidate| candidate.is_file())
            .expect("golden corpus must be readable");
        let content = std::fs::read_to_string(path).expect("golden corpus must be readable");
        let mut failures = Vec::new();
        let mut count = 0;
        for (line_number, line) in content.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            count += 1;
            let record: Value = serde_json::from_str(line).expect("golden line must be JSON");
            let filename = record["filename"].as_str().unwrap_or_default().to_owned();
            let ancestors = record["ancestors"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect();
            let siblings = record["siblings"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect();
            let snapshot = CatalogSnapshot {
                book_key: record["id"].as_str().unwrap_or("fixture").into(),
                filename,
                ancestor_dirs: ancestors,
                parent_siblings: siblings,
            };
            let proposal = parse_catalog(&snapshot, 4, "catalog-rules-v3");
            let expected = &record["expected"];
            if let Some(expected_state) = record["state"].as_str() {
                let actual_state = match proposal.state {
                    ParseState::Ready => "Ready",
                    ParseState::Partial => "Partial",
                    ParseState::Ambiguous => "Ambiguous",
                    ParseState::Unmatched => "Unmatched",
                };
                if actual_state != expected_state {
                    failures.push(format!(
                        "line {line_number}: expected state {expected_state}, got {actual_state}"
                    ));
                }
            }
            if let Some(title) = expected["identity"]["work_title"].as_str() {
                if proposal.work_title.as_deref() != Some(title) {
                    failures.push(format!(
                        "line {line_number}: expected title {title:?}, got {:?}",
                        proposal.title
                    ));
                }
            }
            for alias in expected["identity"]["title_aliases"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                if !proposal.title_aliases.iter().any(|item| item == alias) {
                    failures.push(format!("line {line_number}: missing title alias {alias}"));
                }
            }
            for label in expected["identity"]["numeric_labels"]
                .as_array()
                .into_iter()
                .flatten()
            {
                let prefix = label["prefix"].as_str().unwrap_or_default();
                let value = label["value"].as_str().unwrap_or_default();
                let semantic_role = label["semantic_role"].as_str().unwrap_or_default();
                let raw = label["raw"].as_str().unwrap_or_default();
                if !proposal.numeric_labels.iter().any(|candidate| {
                    candidate.prefix == prefix
                        && candidate.value == value
                        && candidate.semantic_role == semantic_role
                        && candidate.raw == raw
                }) {
                    failures.push(format!(
                        "line {line_number}: missing numeric label {prefix}{raw}"
                    ));
                }
            }
            if let Some(special_title) = expected["identity"]["special_title"].as_str() {
                if proposal.special_title.as_deref() != Some(special_title) {
                    failures.push(format!(
                        "line {line_number}: special title mismatch, got {:?}",
                        proposal.special_title
                    ));
                }
            }
            for series in expected["identity"]["source_series"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                if !proposal.source_series.iter().any(|item| item == series) {
                    failures.push(format!(
                        "line {line_number}: missing source series {series}"
                    ));
                }
            }
            for candidate in expected["identity"]["source_context_candidates"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                if !proposal
                    .source_context_candidates
                    .iter()
                    .any(|item| item == candidate)
                {
                    failures.push(format!(
                        "line {line_number}: missing source context candidate {candidate}"
                    ));
                }
            }
            for creator in expected["identity"]["creators"]
                .as_array()
                .into_iter()
                .flatten()
            {
                let name = creator["name"].as_str().unwrap_or_default();
                let role = creator["role"].as_str().unwrap_or_default();
                if !proposal
                    .creators
                    .iter()
                    .any(|candidate| candidate.name == name && candidate.role == role)
                {
                    failures.push(format!("line {line_number}: missing creator {role}:{name}"));
                }
            }
            if let Some(value) = expected["publication"]["release_event"].as_str() {
                if proposal.release_event.as_deref() != Some(value) {
                    failures.push(format!("line {line_number}: release event mismatch"));
                }
            }
            if let Some(value) = expected["publication"]["publication_year"].as_i64() {
                if proposal.publication_year.as_deref() != Some(value.to_string().as_str()) {
                    failures.push(format!("line {line_number}: publication year mismatch"));
                }
            }
            if let Some(expected_raw) = expected["publication"].get("publication_title_raw") {
                if let Some(value) = expected_raw.as_str() {
                    if proposal.publication_title_raw.as_deref() != Some(value) {
                        failures.push(format!(
                            "line {line_number}: publication title raw mismatch"
                        ));
                    }
                } else if expected_raw.is_null() && proposal.publication_title_raw.is_some() {
                    failures.push(format!(
                        "line {line_number}: publication title raw should be null"
                    ));
                }
            }
            if let Some(value) = expected["publication"]["publication_source"].as_str() {
                if proposal.publication_source.as_deref() != Some(value) {
                    failures.push(format!("line {line_number}: publication source mismatch"));
                }
            }
            let expected_resource_edition = expected["publication"]["resource_edition"]
                .as_str()
                .or_else(|| expected["release"]["resource_edition"].as_str());
            if let Some(value) = expected_resource_edition {
                if proposal.resource_edition.as_deref() != Some(value) {
                    failures.push(format!("line {line_number}: resource edition mismatch"));
                }
            }
            if let Some(value) = expected["sequence"]["chapter_number"]["major"].as_i64() {
                if proposal.chapter.as_deref() != Some(value.to_string().as_str()) {
                    failures.push(format!("line {line_number}: chapter mismatch"));
                }
            }
            if let Some(value) = expected["sequence"]["volume"]["major"].as_i64() {
                if proposal.volume.as_deref() != Some(value.to_string().as_str()) {
                    failures.push(format!("line {line_number}: volume mismatch"));
                }
            }
            if let Some(value) = expected["sequence"]["issue_number"]["major"].as_i64() {
                if proposal.issue.as_deref() != Some(value.to_string().as_str()) {
                    failures.push(format!(
                        "line {line_number}: issue mismatch expected {value}, got {:?} (chapter={:?}, kind={:?})",
                        proposal.issue,
                        proposal.chapter,
                        proposal.sequence_kind
                    ));
                }
            }
            if let Some(value) = expected["sequence"]["chapter_relation"].as_str() {
                if proposal.chapter_relation.as_deref() != Some(value) {
                    failures.push(format!("line {line_number}: chapter relation mismatch"));
                }
            }
            if let Some(value) = expected["sequence"]["sequence_kind"].as_str() {
                if proposal.sequence_kind.as_deref() != Some(value) {
                    failures.push(format!("line {line_number}: sequence kind mismatch"));
                }
            }
            if let Some(value) = expected["sequence"]["part_number"].as_i64() {
                if proposal.part != Some(value) {
                    failures.push(format!("line {line_number}: part number mismatch"));
                }
            }
            if let Some(value) = expected["sequence"]["sequence_label"].as_str() {
                if proposal.sequence_label.as_deref() != Some(value) {
                    failures.push(format!("line {line_number}: sequence label mismatch"));
                }
            }
            if let Some(value) = expected["sequence"]["range"].as_str() {
                if proposal.range.as_deref() != Some(value) {
                    failures.push(format!("line {line_number}: range mismatch"));
                }
            }
            for member in expected["sequence"]["sequence_members"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                if !proposal.sequence_members.iter().any(|item| item == member) {
                    failures.push(format!(
                        "line {line_number}: missing sequence member {member}"
                    ));
                }
            }
            if expected["sequence"]["is_collection"].as_bool() == Some(true)
                && !proposal.is_collection
            {
                failures.push(format!("line {line_number}: expected composite collection"));
            }
            if expected["sequence"].get("sort_key").is_some() {
                let actual = serde_json::to_value(&proposal.sort_key).unwrap_or(Value::Null);
                if actual != expected["sequence"]["sort_key"] {
                    failures.push(format!("line {line_number}: sort key mismatch"));
                }
            }
            if let Some(value) = expected["sequence"]["chapter_title"].as_str() {
                if proposal.chapter_title.as_deref() != Some(value) {
                    failures.push(format!("line {line_number}: chapter title mismatch"));
                }
            }
            if let Some(value) = expected["release"]["language"].as_str() {
                if proposal.resource_language.as_deref() != Some(value) {
                    failures.push(format!("line {line_number}: language mismatch"));
                }
            }
            if let Some(value) = expected["release"]["translation_state"].as_str() {
                if proposal.translation_state.as_deref() != Some(value) {
                    failures.push(format!("line {line_number}: translation state mismatch"));
                }
            }
            if expected["release"]["translation_method"].is_null()
                && expected["release"].get("translation_method").is_some()
                && proposal.translation_method.is_some()
            {
                failures.push(format!(
                    "line {line_number}: translation method should be null"
                ));
            } else if let Some(value) = expected["release"]["translation_method"].as_str() {
                if proposal.translation_method.as_deref() != Some(value) {
                    failures.push(format!("line {line_number}: translation method mismatch"));
                }
            }
            if let Some(value) = expected["release"]["resource_completeness"].as_str() {
                if proposal.resource_completeness.as_deref() != Some(value) {
                    failures.push(format!("line {line_number}: completeness mismatch"));
                }
            }
            if let Some(value) = expected["release"]["censorship"].as_str() {
                if proposal.censorship.as_deref() != Some(value) {
                    failures.push(format!("line {line_number}: censorship mismatch"));
                }
            }
            for tag in expected["release"]["resource_tags"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                if !proposal.resource_tags.iter().any(|item| item == tag) {
                    failures.push(format!("line {line_number}: missing resource tag {tag}"));
                }
            }
            for group in expected["release"]["release_groups"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                if !proposal.release_groups.iter().any(|item| {
                    item == group
                        || item
                            .split(['&', ',', '，', '、'])
                            .any(|part| part.trim() == group)
                }) {
                    failures.push(format!("line {line_number}: missing release group {group}"));
                }
            }
            for forbidden in record["must_not"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                if let Some((role, value)) = forbidden.split_once(':') {
                    let hit = match role {
                        "author" => proposal.authors.iter().any(|item| item == value),
                        "provider" => proposal.provider.as_deref() == Some(value),
                        "work_title" => proposal.title.as_deref() == Some(value),
                        "chapter_number" => proposal.chapter.as_deref() == Some(value),
                        "title_alias" => proposal.title_aliases.iter().any(|item| item == value),
                        "source_series" => proposal.source_series.iter().any(|item| item == value),
                        "release_group" => proposal.release_groups.iter().any(|item| item == value),
                        _ => false,
                    };
                    if hit {
                        failures.push(format!(
                            "line {line_number}: forbidden assignment {forbidden}"
                        ));
                    }
                }
            }
        }
        assert_eq!(count, 52, "seed corpus size changed unexpectedly");
        assert!(
            failures.is_empty(),
            "golden corpus failures:\n{}",
            failures.join("\n")
        );
    }
}
