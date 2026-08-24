# M8-M3 — Optional Provider Enrichment

## Goal

在 Catalog-Only 本地识别已经可用的基础上，为有在线覆盖的作品提供 AniList/Bangumi 元数据增强；Provider 无结果或不可用时，基础 title/author proposal 仍完整可用。

## Requirements

- 独立 async Provider runtime、typed failure、缓存、限流和超时。
- Query 只由 M8-M1 的本地 title/author 文字 evidence 构成，不接收 ByteSource、远程书源会话、远程 URL 或漫画内容。
- AniList/Bangumi 是可选 enrichment，不是基础刮削的成功前提。
- Provider 失败进入 `local_evidence_only` 或 `provider_unavailable`，不回退访问远程书源。

## Acceptance Criteria

- [ ] AniList/Bangumi search/fetch 契约和真实 smoke test 通过。
- [ ] 离线、超时、429、坏响应有 typed failure，且本地 proposal 可正常展示。
- [ ] 缓存命中可离线重放；Provider 原始响应和凭据不进入 canonical 或同步。

## Out of Scope

- MangaDex/ComicVine、OCR、pHash、CLIP 和在线漫画站爬虫。
