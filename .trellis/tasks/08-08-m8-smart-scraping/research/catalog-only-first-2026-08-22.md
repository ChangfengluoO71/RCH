# M8 Product Scope Change — Catalog-Only First — 2026-08-22

## Decision

The first usable M8 version is a generic filename/ancestor-directory role extractor. It reads only persisted catalog data and does not require a raw cache, embedded metadata, a Provider response, or a remote source request.

## Required Output

For each catalog item, produce an explainable proposal containing:

- optional manga/work title;
- zero or more optional authors;
- optional provider/platform/group;
- optional volume/chapter/edition;
- original text span, filename/ancestor source, ancestor level, matched rule, confidence, and conflicts.

Missing title or author is valid. Provider and author are mutually exclusive roles unless an explicit marker creates a visible conflict. Chapter/volume markers must not be retained as title text.

## Context Relation

Compare the filename with the nearest three ancestor directories by default. The parser must distinguish work-like repeated tokens from provider/group labels and collection/category directories. The depth is configurable and included in the rule version.

## Why Provider Is Optional

Manga collections contain many works absent from online databases. AniList/Bangumi therefore enrich local proposals when available but cannot be the success condition for the first release.

## Boundary

RemoteOnly assets use persisted SQLite filename and ancestor names. M8-M1 never opens `ByteSource`, refreshes a source directory, calls stat/HEAD/PROPFIND, downloads content, reads embedded metadata, or invokes sync transport.
