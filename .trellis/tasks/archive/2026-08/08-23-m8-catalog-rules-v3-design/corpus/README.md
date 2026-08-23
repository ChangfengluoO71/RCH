# Catalog Rules v3 Golden Corpus

## Purpose

`catalog-rules-v3-golden.jsonl` is a filename-and-directory-only contract for
the cross-ecosystem parser. It contains no comic bytes, archive metadata or
network responses. A fixture may be run against a `CatalogSnapshot` without
opening a file or contacting a book source.

## Record shape

Each line is a JSON object:

```json
{
  "id": "stable-fixture-id",
  "ecosystem": "ehentai",
  "filename": "raw filename including extension",
  "ancestors": ["depth-1 parent", "depth-2 parent"],
  "siblings": ["other filename in the same persisted parent"],
  "expected": {
    "identity": {},
    "publication": {},
    "sequence": {},
    "release": {}
  },
  "state": "Ready",
  "must_not": ["author:Digital"]
}
```

`expected` is a partial projection. Fields omitted from a fixture are not
asserted. `must_not` records the regressions that are especially important for
role separation.

The v3 proposal keeps `work_title` separate from the raw
`publication_title_raw`. `creators` distinguishes `circle` from `artist`,
while compatibility `authors` contains only person creator roles. Resource
edition, censorship and translation state are release-level fields; a plain
translation label does not imply a translation method. Chapter ordering is a
structured `sort_key` object (`major`, `minor`, `minor_scale`,
`relation_rank`), never a floating-point chapter number.
Unresolved numeric tokens such as `NO.41` and `(1)` are represented by
`identity.numeric_labels`; a combined `前編`/`後編` resource uses
`sequence.sequence_members` and `is_collection` instead of overwriting one
part with the other. Explicit scanlation identities are represented by
`release.release_groups`; an unrecognized trailing parenthesis is represented
by `identity.source_context_candidates` rather than being asserted as a
`source_series`. Bilingual separators create aliases only when the right-hand
side is title-shaped; release signatures remain release metadata.

## Seed coverage

The seed includes the two supplied failure cases plus E-Hentai/nHentai,
MangaDex, Japanese RAW/edition, Chinese netdisk, DLsite, Korean webtoon and
Western Comic Scene/Kavita patterns, plus the final numeric-label,
parenthetical-sibling, composite-part, rating-context, release-group,
metadata-gated-alias and unresolved-parenthetical regressions. It is
deliberately a starting point, not the final accuracy claim.

The validation target is 30-100 reviewed examples per ecosystem. Each added
fixture must preserve the raw filename and the expected semantic projection;
it must not require storing comic content.

## Review rules

- Keep expected values semantic and stable; do not assert implementation-private
  token indexes unless a span rule is the subject of the test.
- Add a `must_not` assertion whenever a token could regress into author,
  provider or title.
- Include an ancestor/sibling fixture for every chapter-only naming pattern.
- Keep ambiguous cases as `Ambiguous` or `Partial`; do not make the corpus pass
  by inventing missing identity fields.
