# Catalog Rules v3 Design

## 1. Design boundary

`catalog-rules-v3` is a deterministic grammar over a local catalog snapshot.
It is not a file parser, archive reader, Provider client or sync operation.

```text
SQLite catalog snapshot
  filename + ancestor names + parent metadata + persisted sibling names
        |
        v
tokenizer -> structural grammar -> position-sensitive role classifier
        -> parent/sibling reconciliation -> explainable semantic AST
```

The parser accepts no `ByteSource`, Downloader, source session, remote URL or
sync transport capability. Parent and sibling context must already exist in
SQLite; missing context is represented as missing evidence.

## 2. Input contract

The v3 design extends the current snapshot without requiring content access:

```text
CatalogSnapshot {
  book_key: String,
  filename: String,
  ancestor_dirs: [String], // persisted local catalog names
  parent_siblings: [String], // persisted local catalog names only
}
```

`parent_siblings` is optional. A source adapter may populate it during its
normal catalog browsing operation, but the scraper never asks the source to
refresh it. `ancestor_depth` and `rule_version` are part of the parse request
and therefore part of the deduplication key.

## 3. Semantic output

The output is an AST projection. Empty fields are valid and are not replaced by
guesses.

```text
Identity
  work_title: Candidate<String>
  publication_title_raw: Candidate<String>
  title_aliases: [Candidate<String>]
  special_title: Candidate<String>
  authors: [String] // compatibility projection: artist/author/writer only
  creators: [CreatorCandidate]
  attribution_candidates: [AttributionCandidate] // unresolved leading tags
  numeric_labels: [NumericLabelCandidate] // consumed but semantically unresolved
  source_series: [Candidate<String>]
  source_context_candidates: [Candidate<String>] // unresolved trailing context
  external_id_candidates: [ExternalIdCandidate]

Publication
  publisher: Candidate<String>
  publication_source: Candidate<String>
  release_event: Candidate<String>
  resource_edition: EditionCandidate
  edition: EditionCandidate // compatibility alias of resource_edition
  publication_year: Optional<i32>
  distribution_platform: Candidate<String>

Sequence
  sequence_kind: chapter | issue | volume | episode | season | part | special
  volume: Optional<NumberOrRange>
  issue_number: Optional<NumberOrRange>
  chapter_number: Optional<ChapterNumber>
  chapter_title: Candidate<String>
  chapter_relation: main | front_part | back_part | continuation |
                    side_story | prologue | epilogue | interlude | unknown
  season_number: Optional<i32>
  part_number: Optional<i32>
  sequence_label: Candidate<String>
  sequence_members: [String]
  includes_special: Optional<bool>
  is_collection: bool // e.g. 前編 + 後編 in one resource
  range: Optional<NumberRange>
  sort_key: Optional<ChapterOrderKey>

Release
  release_groups: [Candidate<String>]
  language: Candidate<String>
  translation_state: Optional<translated | untranslated>
  translation_method: Optional<machine | human> // only explicit evidence
  source_medium: original | raw | paper_scan | web_rip | digital | unknown
  scan_completeness: cover_to_cover | no_ads | incomplete | unknown
  resource_completeness: complete | partial | sample | unknown
  censorship: uncensored | censored | decensored | unknown
  color_state: colorized | monochrome | mixed | unknown
  resource_tags: [ResourceTag]

Evidence
  token: String
  normalized_value: String
  source: filename | ancestor(depth) | sibling(book_key) | user_metadata
  span: Optional<ByteOrCharacterSpan>
  rule_id: String
  confidence: f32
  contextual_support: [String]

State
  Ready | Partial | Ambiguous | Unmatched

Warnings
  unresolved_leading_attribution
  unresolved_numeric_label
  unresolved_terminal_number
  unresolved_parenthetical_context
```

The current Rust projection intentionally stores compact evidence
(`role/value/source/rule`); byte spans and confidence scores remain enrichment
fields for a later ranking layer.

The tokenizer pipeline is fixed as `raw filename -> HTML entity decode ->
Unicode/separator normalization -> structural tokenization`. This prevents
entities such as `&#124;` from becoming a numeric issue token. Bilingual
separators (`|`, `｜`, `丨`, `│`) split the filename residual into a primary
`work_title` and non-canonical `title_aliases` only when both sides are
title-shaped. A right-hand side matching release-group, resource or sequence
grammar is retained as release metadata instead of an alias; no spelling or
canonical-name correction is performed.

`前編`/`前篇`/`後編`/`后篇` are publication parts, not `special` and not
chapter titles. A bare part has `sequence_kind=part`, `part_number` and
`sequence_label`; it does not fabricate `chapter` or `sort_key`. When a chapter
number is present, the same part relation is attached to that chapter.

When both part labels occur in one filename, the parser records
`sequence_members=["front_part", "back_part"]`, sets `is_collection=true` and
leaves `part_number`/`sequence_label` unset rather than overwriting the front
part with the back part. A numeric token in `NO.41` or a standalone
parenthetical `(1)`/`（1）` is retained as a `NumericLabelCandidate` with
`semantic_role=unresolved` until local sibling evidence promotes it. No numeric
token removed from the title is allowed to disappear without structured
evidence.

```text
ChapterOrderKey {
  major: u32,
  minor: Optional<u32>,
  minor_scale: Optional<u8>,
  relation_rank: i16,
}
```

Continuation ordering is represented structurally (`10`, `10续`, `10后編`) by
`major=10` and relation ranks `0`, `1`, `2`; no floating-point chapter number
is ever created. Letter suffixes such as `153b` use `minor=2` and
`minor_scale=26`.

`CreatorCandidate.role` is one of `circle`, `artist`, `writer`, `translator`,
or `unknown_creator`. A circle is never copied into the compatibility
`authors` list; only person creator roles (`artist`, `author` or `writer`) are
projected there. A translator or release group is not silently promoted to the
work creator. `ExternalIdCandidate` contains `namespace_hint` and `raw`.

```text
AttributionCandidate {
  name: String,
  possible_roles: [creator | provider | release_group],
  source: filename-bracket,
}

NumericLabelCandidate {
  prefix: String,
  value: String,
  semantic_role: unresolved | issue,
  raw: String,
}
```

Unknown leading brackets populate this candidate and the warning
`unresolved_leading_attribution`; they do not create a role conflict by
themselves.

Platform/provider dictionaries do not override a leading square-bracket
attribution slot. A leading `[kakao]` is retained as an
`attribution_candidate`; provider/platform fields are filled only by trailing
release grammar or persisted ancestor platform context.

## 4. Token and bracket model

The tokenizer preserves raw text and location before normalization:

```text
Token {
  raw,
  normalized,
  source,
  start,
  end,
  delimiter: bare | square | round | curly | fullwidth,
  container_index,
  adjacency: leading | internal | trailing,
}
```

Normalization is limited to separator/case/Unicode-space comparison. Display
text and original spans are retained for evidence and user review.

Bracket groups are classified using this order:

1. delimiter balance and position;
2. hard negative/resource lexicon;
3. explicit structural grammar;
4. leading nested creator grammar;
5. trailing release/publication grammar;
6. ancestor and sibling agreement;
7. residual candidate classification.

An unknown group is never forced into `author` merely because it is bracketed.
Numeric ranges such as `1-4` are structural range evidence and are never
classified as providers.

## 5. Position-sensitive grammar

### 5.1 Leading creator groups

Before a title, `[Circle (Artist)]` or `[Artist]` is a creator candidate. The
outer value is a `circle` candidate when it has a nested person/group shape;
the nested value is an `artist` candidate. They remain separate until later
evidence proves an alias relation.

Explicit markers such as `作者`, `原作`, `作画`, `著者`, `by`, `artist`,
`circle` and `サークル` override weak shape heuristics.

### 5.2 Trailing release groups

After the title, groups matching `Chinese`, `中国翻訳`, `汉化`, `無修正`,
`DL版`, `Digital`, `Raw`, `Sample`, `Textless`, `c2c`, `noads` or similar
resource lexicon entries become release attributes. They cannot become creators.

Known scanlation names, explicit `汉化组`/`漢化組` identities, `ScanGroup`,
`Scanlation` and platform names become `release_groups` or
`distribution_platform`, not `artist` or `provider`. A translation label such
as `[中国翻译]` or `[汉化]` alone sets translation state but is not itself a
release-group identity.

### 5.3 Parentheses

Parentheses are interpreted by location and shape:

| Pattern | Primary role |
|---|---|
| leading `(C100)`, `(COMITIA143)`, `(SC40)` | `release_event` |
| trailing `(Touhou Project)`, `(Blue Archive)` | `source_series` / parody |
| `(COMIC Megastore 2010-02)` | `publication_source` |
| standalone `(2016)` | `publication_year` |
| `(2 covers)` | cover/variant metadata |
| `(of 06)` | total issue/range metadata |
| trailing `(Minutemen-Slayer)` | `release_group` candidate |

The parser keeps competing candidates when the same shape is genuinely
ambiguous and lets ancestor/sibling context resolve it.

The trailing fallback to `source_series` is deliberately last and requires a
known local franchise/work-shape entry (for example `Blue Archive`,
`ブルーアーカイブ`, `五等分の花嫁` or `アークナイツ`). Resource/page tokens
such as `(106p)` and `(無修)`, part markers such as `(下)`, release events such
as `(FF45)`, and publication sources such as `(WEEKLY快楽天 2025 No.16)` are
classified before source-series inference. An unrecognized parenthetical such
as `(しぐれうい)`, `(シスター・クレア)`, `(古月娜／古月)` or `(exodus626)` is
stored as `source_context_candidates` with an
`unresolved_parenthetical_context` warning; it is never asserted as a source
series solely because it is in trailing parentheses.

Standalone parenthetical numbers are not issues by default. They become an
`issue` only when persisted siblings with the same normalized title provide a
sequence (for example `(1)`, `(2)`, `(3)`). A terminal number attached directly
to a title is similarly retained with `unresolved_terminal_number` unless a
serial ancestor, sibling set or matching bilingual title sides corroborate it.
Explicit markers (`#57`, `Ch.10`, `第10话`, `Vol.2`) remain strong sequence
evidence. Rating contexts such as `评分5`, `评价5`, `評分5`, `評価5`, `rating5`,
`score5` and star glyphs are negative evidence and never create an issue.

### 5.4 Unknown bracket groups

An unknown group can produce one or more candidates:

```text
unknown_tag
title_alias_candidate
sequence_candidate
```

For example, `[Kimi wa Chuu no Subete vol 01-08]` is more likely a romanized
title plus volume range than an author because it contains a title-like phrase
and a structural volume marker.

## 6. Structural sequence grammar

Structural tokens are consumed before title extraction.

```text
chapter: 第10话, 第10章, 10话, Ch.10, Chapter 10, Ep.10, Episode 10
volume:  卷, 巻, 册, Vol.1, Volume 01, v01, Tome 2, Tankoubon
part:    前篇, 前編, 后篇, 後編, Part 1, Part 2, 1部
season:  Season 2, S02, 시즌 2
special: 番外, 番外編, 外传, 外伝, Extra, Special, One Shot, Annual, SP01
```

Continuation is represented independently from the numeric chapter:

```text
10       -> chapter.major=10, relation=main, sort={major:10, relation_rank:0}
10续     -> chapter.major=10, relation=continuation, sort={major:10, relation_rank:1}
10后篇   -> chapter.major=10, relation=back_part, sort={major:10, relation_rank:2}
11       -> chapter.major=11, relation=main, sort={major:11, relation_rank:0}
```

`153b` is stored as `major=153`, `suffix=b`. It is not converted to a
floating-point number; sibling ordering may later place it after `153` and
before `154`.

Decimal chapter labels are consumed as one token before integer scanning:
`57.2` becomes `chapter="57.2"` and
`{major:57, minor:2, minor_scale:1, relation_rank:0}`; `12.25` uses
`minor_scale=2`.

`01-08`, `1 of 6`, `v01-04` and similar expressions populate `range`, not a
single chapter number. A filename containing only a part marker keeps
`chapter=null` and `sort_key=null` until a chapter number is present.

Composite expressions are consumed before single-number extraction. Numeric
and Roman-member forms such as `1+2`, `1-6+7+8+9`, `01-4.5`, `m1-m50`,
`1-9+番外`, `1-3+特典` and `Ⅰ-Ⅵ+特典` populate `sequence_members`, `range`
(when a dash/tilde is present) and `includes_special`; they cannot leave a
trailing `+` for the title or let the final number become `issue`.

Pure numeric filenames and names such as `123456.zip` remain
`ambiguous_numeric_identifier` unless a parent, sibling, `RJ/BJ/VJ`, `DLsite`,
`gallery`, `nHentai` or other namespace provides support.

## 7. Parent and sibling reconciliation

Reconciliation runs after local token classification:

1. Classify each ancestor as a format bucket (`EPUB`, `PDF`, `CBZ`…), media
   bucket (`漫画`, `小说`…) or publication bucket (`单行本`, `连载`, `合集`…)
   before considering it as identity evidence.
2. Skip those buckets and choose the final meaningful ancestor as the primary
   `work_title` candidate when the filename contributes only a sequence token.
3. Only explicit creator markers or a locally corroborated person-shaped name
   may become an ancestor creator; media/publication buckets can never become
   artists merely because they are adjacent.
4. Group sibling filenames by their parent key. A sequence like `9`, `10`,
   `10续`, `11` supports chapter relation and ordering but never creates a
   title from a number alone.
5. Compare filename residuals with every ancestor candidate. Exact agreement,
   normalized agreement and repeated sibling agreement are separate evidence
   rules.
6. If title evidence is absent, return `Partial` even when chapter evidence is
   high confidence.

For a `special/side_story` filename with a strong ancestor work candidate, the
ancestor wins `work_title`; the filename residual is stored as
`special_title`. A self-contained special filename remains `Ready` and may use
its own residual as the work title.

The current filename is removed from `parent_siblings` before sibling evidence
is recorded, so a file cannot corroborate itself. No reconciliation step may
request a fresh directory listing or remote stat.

## 8. Lexicon organization

The lexicon is versioned data, not scattered string arrays:

```text
LexiconEntry {
  id,
  aliases,
  category,
  subtype,
  precedence,
  hard_negative,
  requires_context,
  locale,
  examples,
}
```

Initial categories:

| Category | Examples | Target fields |
|---|---|---|
| `chapter` | 话, 話, 章, 回, Ch, Chapter, Ep, Episode | chapter/episode |
| `volume` | 卷, 巻, 册, Vol, Volume, v, Tome | volume |
| `part` | 前篇, 前編, 后篇, 後編, Part, 部 | part/relation |
| `season` | Season, S02, 시즌 | season |
| `special` | 番外, 外伝, Extra, Special, SP, One Shot, Annual | special |
| `publication_kind` | 単行本, Tankoubon, Anthology, TPB, Omnibus, Compendium | publication kind |
| `edition` | DL版, Digital, Web版, 完全版, 特装版, 新装版, 愛蔵版, 文庫版 | resource_edition |
| `translation` | 中国翻訳, 汉化, English, Korean, MTL | language/state/method |
| `scan_source` | Raw, 生肉, Scan, Webrip, paper, c2c, noads | source medium/completeness |
| `processing` | 無修正, Decensored, Colorized, Textless, AI Generated, Sample | release attributes |
| `completeness` | Complete, Incomplete, 全卷, 全套, 完结 | resource completeness |
| `event` | C100, COMITIA91, COMIC1, SC40 | release event |
| `external_id` | RJ/BJ/VJ + digits | external id candidate |
| `negative` | Vol, Ch, Raw, Digital, Scan, 完整版, 汉化组 | lower creator/title weight |

Context-sensitive entries such as `raw`, `digital`, `complete` and `scan` do
not imply language or official work status. Translation labels set
`translation_state=translated` but leave `translation_method=null`; only
explicit `MTL`/`machine translation`/`机翻`/`機翻`/`AI翻訳` or an explicit human
translation label sets a method.

## 9. State and confidence invariants

`Ready` is a structural invariant, not a vague score threshold:

```text
work_title != null
AND no high-confidence role conflict
AND all structural numeric tokens are consumed or explicitly unresolved
AND creator/resource/provider namespaces do not overlap
AND an incomplete filename has contextual title evidence
```

`Partial` means useful evidence exists but a required identity context is
missing. `Ambiguous` means two incompatible high-confidence role assignments
remain. `Unmatched` means no meaningful semantic candidate was found.

An unresolved leading attribution warning is reviewable evidence, not a
high-confidence role conflict; a complete title therefore remains `Ready`.

## 10. Work state versus resource state

`Complete`, `Incomplete`, `全卷`, `连载`, `生肉` and `熟肉` are release/resource
signals. They populate `resource_completeness`, `translation_state` or
`resource_tags`. They do not set `work_status=completed`.

`work_status` remains unknown until a later canonical/provider stage verifies it.

## 11. Rule version and correction loop

The first implementation target is `catalog-rules-v3`. Every proposal stores
the rule version and raw token evidence. Manual corrections are recorded as
scoped dictionary candidates:

```text
raw token -> confirmed role -> scope -> evidence count
```

One correction does not become a global rule. Promotion requires repeated
consistent confirmations or an explicit rule-review decision.

## 12. Test strategy

The parser is tested in layers:

1. tokenizer tests preserve spans and bracket position;
2. structural tests consume sequence/range/year/id tokens;
3. role tests enforce namespace separation;
4. context tests use parent and sibling catalog snapshots;
5. golden corpus tests compare stable AST projections;
6. local-first tests assert zero source I/O.

The seed corpus is intentionally small enough to review. Before implementation
is declared usable, grow it to 30-100 examples per ecosystem and report title,
creator, sequence, role-conflict and unresolved-field coverage separately.

## 13. Implementation gate

The user-approved v3 scope is implemented and frozen for the offline catalog
layer after the 347-row local/Quark after8 regression pass. Provider
enrichment, canonical writes and archive/content evidence remain later M8
stages; this parser does not auto-materialize canonical metadata.
