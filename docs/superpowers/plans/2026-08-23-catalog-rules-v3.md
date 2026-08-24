# Catalog Rules v3 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the narrow v2 filename parser with a position-sensitive, cross-ecosystem catalog grammar that remains local-only and produces explainable identity, publication, sequence and release projections.

**Architecture:** Keep the Rust parser as the single semantic owner. Extend its catalog input with persisted parent/sibling names, tokenize while preserving bracket position, classify structural/resource tokens before creator/title inference, reconcile candidates with ancestor/sibling context, and persist the richer projection through the existing FRB/database/UI path. The parser never receives `ByteSource`, Downloader, source sessions or sync handles.

**Tech Stack:** Rust (`serde`, unit tests), SQLite via `rusqlite`, Flutter/Dart FRB bindings, JSONL golden corpus.

**Spec:** `.trellis/tasks/08-23-m8-catalog-rules-v3-design/design.md`

## Global Constraints

- Scraping consumes only persisted SQLite catalog text: filename, ancestor names, parent metadata and persisted sibling names.
- RemoteOnly scraping performs zero book-source requests, Range reads, downloads, HEAD/stat/PROPFIND calls and cover/content reads.
- `circle`, `artist`, `release_group`, `publisher`, `publication_source`, `distribution_platform`, `source_series` and resource tags remain separate namespaces.
- Structural sequence tokens are consumed before title inference; unknown bracket groups are never silently assigned to author.
- `Complete` and `Incomplete` describe resource completeness, not canonical work completion.
- Automatic scraping creates working proposals only; it does not confirm canonical metadata or invoke sync transport.
- Preserve unrelated existing user changes in `app/lib/ui/library_page.dart`, `app/rust/src/reader.rs` and any pre-existing task artifacts.

## Implementation checkpoint

Tasks 1-5 are complete: the 33-line golden corpus, v3 parser, catalog-only
batch API, semantic persistence/FRB bindings, automatic coordinator and review
panel are implemented. Rust and Flutter verification pass. Task 6 is paused at
manual validation with real library samples; no provider enrichment or
canonical confirmation writes are included in this checkpoint.

---

### Task 1: Add v3 failing fixtures and semantic projections

**Files:**
- Modify: `app/rust/src/scraper.rs` tests
- Create: `.trellis/tasks/08-23-m8-catalog-rules-v3-design/corpus/catalog-rules-v3-golden.jsonl` (already seeded; keep it as the review corpus)

**Interfaces:**
- Test the existing `parse_catalog` API first so failures demonstrate the missing v3 behavior before production changes.
- The first red tests cover the supplied screenshots, `10续`, `(C100) [Circle (Artist)] Title (Source Series) [Chinese]`, `RJ01234567`, `01-08`, `c2c/noads`, `153b`, timestamp suffixes and resource-vs-work completeness.

- [ ] **Step 1: Add one focused test for the supplied screenshot**

```rust
#[test]
fn separates_circle_artist_title_part_and_resource_tags() {
    let snapshot = CatalogSnapshot {
        book_key: "fixture/jp-001".into(),
        filename: "[チサキックス (枡田ちさき)] 幼馴染ギャルに好きと言えない陰キャな俺 前編 [中国翻訳] [無修正] [DL版].zip".into(),
        ancestor_dirs: vec![],
    };

    let proposal = parse_catalog(&snapshot, 3, "catalog-rules-v3");

    assert_eq!(proposal.title.as_deref(), Some("幼馴染ギャルに好きと言えない陰キャな俺"));
    assert_eq!(proposal.chapter_title.as_deref(), Some("前編"));
    assert_eq!(proposal.resource_language.as_deref(), Some("zh"));
    assert_eq!(proposal.censorship.as_deref(), Some("uncensored"));
    assert_eq!(proposal.edition.as_deref(), Some("digital"));
    assert!(!proposal.authors.iter().any(|x| x == "中国翻訳"));
}
```

- [ ] **Step 2: Run the focused test and verify it fails for missing fields/behavior**

Run: `cargo test scraper::tests::separates_circle_artist_title_part_and_resource_tags --lib`

Expected: FAIL because the v2 proposal has no chapter-title/resource fields and treats CJK bracket labels as authors.

- [ ] **Step 3: Add focused red tests for continuation and cross-ecosystem structure**

```rust
#[test]
fn chapter_only_continuation_uses_parent_and_sorts_after_main_chapter() {
    let snapshot = CatalogSnapshot::with_context(
        "10续.zip",
        vec!["作品名"],
        vec!["9.zip", "10.zip", "10续.zip", "11.zip"],
    );
    let proposal = parse_catalog(&snapshot, 3, "catalog-rules-v3");
    assert_eq!(proposal.title.as_deref(), Some("作品名"));
    assert_eq!(proposal.sequence.chapter_major, Some(10));
    assert_eq!(proposal.sequence.relation, ChapterRelation::Continuation);
    assert_eq!(proposal.sequence.sort_key, Some((10, 1)));
}

#[test]
fn parenthesis_position_separates_event_source_series_year_and_release_group() {
    let proposal = parse_fixture("(C100) [ABC (XYZ)] Some Title (Blue Archive) (2016) (Group).zip");
    assert_eq!(proposal.publication.release_event.as_deref(), Some("C100"));
    assert_eq!(proposal.identity.source_series, vec!["Blue Archive"]);
    assert_eq!(proposal.publication.publication_year, Some(2016));
    assert_eq!(proposal.release.release_groups, vec!["Group"]);
}

#[test]
fn numeric_external_id_is_not_a_chapter_without_namespace_context() {
    let proposal = parse_fixture("[RJ01234567] Title.zip");
    assert_eq!(proposal.identity.external_ids[0].namespace_hint, "dlsite");
    assert!(proposal.sequence.chapter_major.is_none());
}
```

- [ ] **Step 4: Run the new tests and record the expected red failures**

Run: `cargo test scraper::tests --lib`

Expected: the new v3 cases fail while the existing v2 tests remain green.

---

### Task 2: Replace the flat proposal with the v3 semantic model

**Files:**
- Modify: `app/rust/src/scraper.rs`
- Test: `app/rust/src/scraper.rs` unit tests

**Interfaces:**
- Preserve the existing FRB-facing fields until the API migration in Task 5.
- Add serializable projections for creators with roles, aliases, source series, external IDs, publication fields, sequence fields, release fields and typed evidence.

- [ ] **Step 1: Add the v3 Rust types and compatibility fields**

Define `CreatorRole`, `CreatorCandidate`, `SequenceKind`, `ChapterNumber`, `NumberRange`, `ResourceTag`, `EditionCandidate`, `ExternalIdCandidate` and the grouped `NameRoleProposalV3` projection. Keep `title`, `authors`, `provider`, `volume` and `chapter` as compatibility projections derived from the richer model.

- [ ] **Step 2: Run the focused red tests to confirm the new types do not yet change parsing**

Run: `cargo test scraper::tests::separates_circle_artist_title_part_and_resource_tags --lib`

Expected: FAIL on semantic values, not compilation errors.

- [ ] **Step 3: Add deterministic normalization/token types**

Implement `Token`, `BracketGroup`, raw-span preservation, Unicode whitespace normalization, separator normalization and position labels (`leading`, `internal`, `trailing`). Do not discard the display token.

- [ ] **Step 4: Run the tokenizer tests**

Run: `cargo test scraper::tests::tokenizer_preserves_bracket_position --lib`

Expected: PASS with exact delimiter/position assertions.

---

### Task 3: Implement lexicon precedence and position-sensitive classification

**Files:**
- Modify: `app/rust/src/scraper.rs`
- Test: `app/rust/src/scraper.rs` unit tests

**Interfaces:**
- Add versioned `LexiconEntry` data and category helpers for chapter, volume, part, season, special, edition, translation, scan/source, processing, completeness, event, external id, provider/platform and negative terms.
- Classification order is hard negative/resource -> structural grammar -> leading creator -> trailing release/publication -> context -> residual candidate.

- [ ] **Step 1: Add failing tests for hard negative/resource precedence**

```rust
#[test]
fn resource_labels_never_become_authors_or_providers() {
    let proposal = parse_fixture("Title [中国翻訳] [無修正] [DL版].zip");
    assert_eq!(proposal.release.language.as_deref(), Some("zh"));
    assert_eq!(proposal.release.censorship, Censorship::Uncensored);
    assert_eq!(proposal.publication.edition, Edition::Digital);
    assert!(!proposal.identity.creators.iter().any(|c| c.name == "中国翻訳"));
}

#[test]
fn raw_is_resource_origin_not_language() {
    let proposal = parse_fixture("作品名 raw 第1巻.zip");
    assert_eq!(proposal.release.source_medium, SourceMedium::Raw);
    assert!(proposal.release.language.is_none());
}

#[test]
fn complete_is_resource_completeness_not_work_status() {
    let proposal = parse_fixture("[Complete] Work Vol 1-4.cbz");
    assert_eq!(proposal.release.resource_completeness, ResourceCompleteness::Complete);
    assert_eq!(proposal.identity.work_status, WorkStatus::Unknown);
}
```

- [ ] **Step 2: Run the tests and verify v2 misclassifies them**

Run: `cargo test scraper::tests::{resource_labels_never_become_authors_or_providers,raw_is_resource_origin_not_language,complete_is_resource_completeness_not_work_status} --lib`

Expected: FAIL with v2 role assignments or missing fields.

- [ ] **Step 3: Implement lexicon entries and precedence**

Use explicit normalized categories and negative weights. Provider/platform labels may lower creator confidence but cannot erase evidence. Unknown groups become `unknown_tag`, `title_alias_candidate` or `sequence_candidate`.

- [ ] **Step 4: Implement leading `[Circle (Artist)]` and trailing release groups**

Keep circle and artist as separate creator candidates. Only a later explicit alias rule can relate them. `[Translator]` becomes release-group/translation evidence, not artist.

- [ ] **Step 5: Run the focused suite and then all Rust scraper tests**

Run: `cargo test scraper::tests --lib`

Expected: all v3 and existing compatible tests PASS.

---

### Task 4: Implement sequence grammar and catalog context reconciliation

**Files:**
- Modify: `app/rust/src/scraper.rs`
- Modify: `app/rust/src/db/mod.rs` for persisted sibling-name queries
- Test: Rust scraper and database tests

**Interfaces:**
- Extend `CatalogSnapshot` with persisted `parent_siblings` and optional directory depth metadata. Keep missing sibling context valid.
- Produce `sequence_kind`, chapter/issue/volume/episode/season/part, ranges, relation and stable `sort_key`.

- [ ] **Step 1: Add failing sequence tests**

```rust
#[test]
fn parses_10_continuation_as_10_plus_one_not_chapter_11() {
    let proposal = parse_fixture("10续.zip");
    assert_eq!(proposal.sequence.chapter_major, Some(10));
    assert_eq!(proposal.sequence.relation, ChapterRelation::Continuation);
    assert_eq!(proposal.sequence.sort_key, Some((10, 1)));
}

#[test]
fn parses_half_suffix_without_floating_point_loss() {
    let proposal = parse_fixture("Series 153b.cbz");
    assert_eq!(proposal.sequence.chapter_major, Some(153));
    assert_eq!(proposal.sequence.chapter_suffix.as_deref(), Some("b"));
}

#[test]
fn parses_ranges_and_total_issue_counts() {
    let volume = parse_fixture("Series v01-08.rar");
    assert_eq!(volume.sequence.volume_range, Some((1, 8)));
    let issue = parse_fixture("Series 01 (of 06).cbz");
    assert_eq!(issue.sequence.issue_range, Some((1, 6)));
}

#[test]
fn strips_technical_timestamp_suffix_without_losing_raw_evidence() {
    let proposal = parse_fixture("19话_20190923103738.zip");
    assert_eq!(proposal.sequence.chapter_major, Some(19));
    assert!(proposal.evidence.iter().any(|e| e.rule_id == "technical-timestamp-suffix"));
}
```

- [ ] **Step 2: Run the tests and verify the v2 parser fails**

Run: `cargo test scraper::tests::{parses_10_continuation_as_10_plus_one_not_chapter_11,parses_half_suffix_without_floating_point_loss,parses_ranges_and_total_issue_counts,strips_technical_timestamp_suffix_without_losing_raw_evidence} --lib`

Expected: FAIL on continuation, suffix, range or timestamp assertions.

- [ ] **Step 3: Implement structural token consumption before title selection**

Consume chapter/issue/volume/season/part/special/range/year/id/timestamp tokens first. A pure chapter filename cannot become a title.

- [ ] **Step 4: Implement parent/sibling reconciliation**

Use exact ancestor agreement, normalized agreement, repeated sibling parent evidence and person/provider negative rules. If a chapter-only file has no title context, return `Partial`.

- [ ] **Step 5: Add SQLite snapshot helpers and tests**

Load persisted sibling names by parent key without opening a source. Test that the query returns only already-indexed rows and that RemoteOnly scraping performs no source calls.

- [ ] **Step 6: Run Rust scraper/database tests**

Run: `cargo test scraper:: db:: --lib`

Expected: PASS with zero source/session dependencies.

---

### Task 5: Thread the v3 projection through persistence, FRB and review UI

**Files:**
- Modify: `app/rust/src/db/mod.rs`
- Modify: `app/rust/src/api/scraper.rs`
- Regenerate: `app/lib/src/rust/api/scraper.dart`, FRB generated files
- Modify: `app/lib/ui/scrape_panel.dart`
- Test: Rust API/DB tests and Flutter analyze/tests

**Interfaces:**
- Store the serialized v3 projection and rule version in `scrape_proposals`; retain compatibility columns for existing UI until the panel consumes the grouped model.
- Expose a typed `ScrapeProposalDto` with identity/publication/sequence/release/evidence projections.

- [ ] **Step 1: Add failing persistence/API tests for the grouped projection**

Assert that a proposal round-trips creator roles, source series, event, external id, sequence relation, release tags and state without dropping raw evidence.

- [ ] **Step 2: Run the API/DB tests and verify missing columns/serialization fail**

Run: `cargo test api::scraper db:: --lib`

Expected: FAIL on the new v3 fields.

- [ ] **Step 3: Add schema columns or a versioned JSON projection**

Use the existing SQLite conventions and upsert semantics. Keep migration ordered and write the rule version with every proposal.

- [ ] **Step 4: Update FRB API and regenerate bindings**

Run from `app`:

```powershell
flutter_rust_bridge_codegen generate
```

- [ ] **Step 5: Update the review panel**

Display creator roles, aliases, work/source series, event, edition, sequence kind/relation/sort key, release groups and resource completeness. Show unresolved/unknown candidates and evidence instead of silently hiding them.

- [ ] **Step 6: Run Rust and Flutter verification**

Run:

```powershell
cargo test --lib
flutter analyze
flutter test --no-pub
```

Expected: all pass; the panel remains catalog-only and no canonical writes occur.

---

### Task 6: Run the golden corpus and stop at manual validation

**Files:**
- Create/modify: a Rust corpus runner test near `app/rust/src/scraper.rs`
- Read: `.trellis/tasks/08-23-m8-catalog-rules-v3-design/corpus/catalog-rules-v3-golden.jsonl`
- Modify: `.trellis/tasks/08-19-m8-a0-automation-integration/implement.md` with the validation checkpoint

**Interfaces:**
- The corpus runner compares only stable semantic projections and `must_not` assertions.
- It must not open fixture paths or contact any source.

- [ ] **Step 1: Add a corpus loader test**

Parse all 33 seed lines, verify unique IDs and compare expected projections.

- [ ] **Step 2: Run the corpus test and fix parser behavior, not fixture expectations**

Run: `cargo test scraper::golden --lib`

Expected: PASS for the seed corpus before manual UI validation.

- [ ] **Step 3: Run the full verification gate**

Run `cargo test --lib`, `flutter analyze`, `flutter test --no-pub` and `git diff --check`.

- [ ] **Step 4: Launch the Windows app and perform manual sample validation**

Use Settings -> 智能刮削（Catalog-only） -> 立即刮削 with the two supplied examples and chapter-only directory fixtures. Verify title, creator roles, chapter relation, resource tags, state and evidence.

- [ ] **Step 5: Stop and request user feedback**

Do not implement Provider enrichment, canonical confirmation or further automatic learning until the user reports real filename/ancestor expected-vs-actual cases.

---

## Self-review

- Spec coverage: identity, publication, sequence, release, bracket position,
  parent/sibling context, resource/work state, external ids, local-first I/O,
  persistence, UI and corpus validation are covered by Tasks 1-6.
- No production implementation is included in this plan; every code task has
  a failing test before the implementation step.
- Compatibility fields remain until the FRB/UI migration, so existing automatic
  jobs can continue to persist proposals during the transition.
