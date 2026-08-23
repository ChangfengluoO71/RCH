# M8 Catalog Rules v3: Cross-Ecosystem Local Catalog Grammar

## Status

This artifact supersedes the narrower v2 rule proposal. The design and seed
corpus were approved by the user on 2026-08-23. The after8 repair pass has
completed the 347-row local/Quark validation: offline architecture and core
proposal parsing are frozen; canonical materialization remains a later M8
stage.

## Problem

Real collections mix naming ecosystems rather than following one manga naming
convention. E-Hentai-style releases, MangaDex exports, Japanese RAW archives,
Chinese netdisk archives, DLsite-organized works, Korean webtoons, Western
comic-scene files and Kavita/ComicTagger names use the same brackets and numbers
for different meanings. A parser that treats every bracket as author/provider
and every number as chapter will silently corrupt identity and sequence data.

The current v2 design also collapses circle, artist, release group, publisher,
publication source and distribution platform into too few fields. It cannot
represent a release event, a parody/source series, a volume range, a technical
timestamp suffix, or the difference between a resource being complete and the
original work being complete.

## Goal

Define a deterministic `catalog-rules-v3` grammar that consumes only local
catalog snapshots and produces an explainable semantic AST. The grammar must
remain useful when the file is RemoteOnly, because it may read SQLite catalog
text but must never open a remote book source.

## P0 invariants

1. **Local-only input**: filename, ancestor directory names, parent metadata and
   persisted sibling names come from SQLite. No `ByteSource`, Downloader,
   source session, remote URL, stat/HEAD/PROPFIND, Range read or download is
   allowed.
2. **Position-sensitive parsing**: bracket and parenthesis meaning depends on
   delimiter, position, adjacency and the surrounding token grammar.
3. **Namespace separation**: creator, release group, publisher, publication
   source, distribution platform, source series and resource tags are distinct
   namespaces. A token cannot be silently assigned to two incompatible roles.
4. **Structure before identity**: consume chapter/issue/volume/season/part,
   ranges, years, event codes and technical suffixes before selecting a title.
5. **Context before guessing**: parent and sibling catalog evidence may resolve a
   chapter-only filename; missing context produces `Partial`, never a fabricated
   title. The current file is excluded from sibling corroboration.
6. **Resource state is not work state**: `Complete`, `Raw`, `Digital`, `c2c`,
   `Incomplete` and similar labels describe the acquired release unless a later
   Provider confirms the original work status.
7. **Explainability**: every non-empty field has token spans, rule id, source,
   confidence and contextual support.
8. **Unknown attribution is non-destructive**: an unresolved leading bracket is
   retained as an attribution candidate/warning, but is not projected into
   `authors`, `provider` or `release_groups`, and does not make an otherwise
   complete identity `Ambiguous`.
9. **Ancestor semantics are explicit**: format (`EPUB`, `PDF`, `CBZ`…), media
   (`漫画`, `小说`…) and publication buckets (`单行本`, `连载`, `合集`…) are
   context, never creator/title candidates. A final meaningful ancestor is the
   work candidate when the filename contains only a sequence token.

## Product requirements

### PR-1: Semantic output model

The parser emits `Identity`, `Publication`, `Sequence`, `Release`, `Evidence`
and `State` groups. All fields are optional except the raw input and rule
version.

### PR-2: Cross-ecosystem grammar

The first corpus covers at least these families:

- E-Hentai/ExHentai and nHentai-style releases;
- MangaDex and scanlation-group exports;
- Japanese RAW and Japanese edition names;
- Chinese netdisk naming, including chapter-only files;
- DLsite work numbers and circle/artist names;
- Korean webtoon season/episode names;
- Western Comic Scene, Kavita and ComicTagger conventions.

### PR-3: Parent and sibling reconciliation

When a filename is only `10续.zip`, `番外2.zip`, `第11话.zip` or a timestamped
variant, the parser uses the persisted parent path and sibling names to infer
the work title and ordering. If the catalog has not populated that context,
the result is explicitly incomplete.

### PR-4: Sequence fidelity

The AST distinguishes `chapter`, `issue`, `volume`, `episode`, `season`,
`part` and `special`. `前編`/`前篇`/`後編`/`后篇` are `part` labels, not
`special` or chapter titles. It preserves raw labels and produces a structured
`ChapterOrderKey`; `10续` sorts after `10` and before `11` without a float.

### PR-5: Resource and publication fidelity

Decimal labels are atomic chapter tokens: `57.2` is represented as
`{major:57, minor:2, minor_scale:1}`, while `12.25` uses
`minor_scale:2`; the parser never splits a decimal into unrelated integers.

The AST represents resource-level edition, translation state/method, language, release group, publication
source, publisher, platform, event, source series/parody, range, scan medium,
scan completeness, censorship and color state without treating them as title
or creator. Explicit scanlation identities are retained in `release_groups`
without being promoted to `provider`; unresolved parenthetical context is
retained separately from asserted `source_series`.

### PR-6: Safe ambiguity

Unknown bracket groups become `unknown_tag`, `title_alias_candidate`,
`sequence_candidate` or another explicit candidate. They are not automatically
authors. The implementation retains an `attribution_candidate` and an
`unresolved_leading_attribution` warning for unknown leading brackets;
high-confidence conflicts, not that warning alone, produce `Ambiguous`.

### PR-7: Golden corpus

The seed corpus in `corpus/catalog-rules-v3-golden.jsonl` is a contract, not a
provider fixture. It stores only names and expected AST projections. The full
validation target is 30-100 real filename examples per ecosystem, with no comic
bytes or network access.

## Acceptance criteria for implementation

- A fixture matching the first supplied screenshot yields circle/artist
  candidates, the Japanese work title, `前編` as `part=1`, and resource tags
  for Chinese translation, uncensored and digital edition; `authors` contains
  only the artist, and none of those tags become an author or provider.
- A complete filename is `Ready` without ancestors or siblings. A chapter-only
  filename is `Partial` until persisted ancestor context supplies its work title.
- A plain translation label sets `translation_state=translated` and leaves
  `translation_method=null`; only explicit machine/human labels set a method.
- `10续.zip` yields a parent-derived work title, chapter major `10`, relation
  `continuation`, and a sort key between chapter 10 and chapter 11. Without a
  parent title it is `Partial`, not a title named `10续`.
- `(C100)`, `(COMITIA143)`, `(2016)`, `(2 covers)`, `(of 06)` and
  `(COMIC Megastore 2010-02)` are classified by position and lexical shape,
  not by a single generic parenthesis rule.
- `RJ/BJ/VJ` identifiers are external-id candidates and never chapter numbers
  without namespace/context evidence.
- `Complete`/`Incomplete` affect resource completeness only; work status stays
  unknown until canonical/provider evidence exists.
- A RemoteOnly run has zero book-source requests and reads only the persisted
  catalog snapshot, while retaining all evidence and unresolved candidates.
- HTML entities are decoded before tokenization; bilingual separators produce a
  primary title plus `title_aliases` without canonical correction only when both
  sides are title-shaped. A release/scanlation signature on the right-hand
  side is retained as `release_groups`, not a title alias.
- A special filename with a strong ancestor work title keeps the ancestor as
  `work_title` and stores the filename residual as `special_title`.
- Leading platform-looking attributions such as `[kakao]` remain unresolved
  candidates and never populate provider/platform fields without trailing or
  ancestor context.
- Composite numeric/Roman sequence expressions are consumed atomically and
  expose `range`, `sequence_members` and `includes_special` without leaking a
  final number into `issue`.
- `[中国語]`, `[中国语]` and `[中文]` set `resource_language=zh` only;
  translation state is set by explicit translation labels such as
  `[中国翻訳]`, `[中国翻译]`, `[汉化]` or `[漢化]`. Plain language labels do
  not invent a translation method.
- Nested leading creator grammar is evaluated before provider detection, and
  numeric ranges such as `1-4`/`1-6` never become providers.
- `NO.41` and standalone `(1)`/`（1）` never lose their numeric token: they
  produce a structured unresolved numeric label until local sibling evidence
  proves an issue sequence.
- A filename containing both `前編` and `後編` produces a composite part
  proposal (`sequence_members=["front_part","back_part"]`, `is_collection=true`)
  rather than silently retaining only the back part.
- Terminal numbers are stripped only with explicit, sibling, serial-context or
  matching-bilingual corroboration. Rating contexts (`评分5`, `評価5`,
  `rating5`, star scores) remain title text and never become issues.
- Explicit identities such as `[無邪気漢化組]`, `[Amerins漢化]` and
  `[汉化组]` populate `release_groups` while `provider` remains unset; plain
  `[中国翻译]`/`[汉化]` labels do not invent a group identity.
- A trailing parenthesis becomes `source_series` only for a known local
  franchise/work-shape entry. Unrecognized values such as `(しぐれうい)` or
  `(exodus626)` populate `source_context_candidates` and the
  `unresolved_parenthetical_context` warning instead.
- The consumed-token invariant holds for the offline catalog layer: every
  removed numeric/structural token has evidence or a structured candidate, and
  the same 347-row local/Quark corpus has zero known false title, sequence or
  provider assignments.

## Out of scope

- Opening archives or reading ComicInfo, OPF, MOBI, page counts, covers or OCR.
- Provider calls, canonical materialization, automatic confirmation or file
  renaming.
- Filename spelling correction, fuzzy canonicalization, alias promotion and
  external creator/title matching.
- Inferring official work completion from a filename alone.
- Treating a single unknown token as a globally learned dictionary entry.

## Review gate

The approval gate is complete. The tokenizer/classifier/context reconciler and
catalog-only workflow are implemented and verified; Provider enrichment and
canonical writes remain later M8 stages until manual samples are accepted.
