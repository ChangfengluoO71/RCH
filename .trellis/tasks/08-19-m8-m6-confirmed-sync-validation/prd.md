# M8-M6 — Corpus Validation

## Goal

用 100 本真实漫画验证 Catalog-Only 基础识别器是否可交付，并单独报告 Provider enrichment 与 canonical/sync 能力。

## Requirements

- 验证 confirmed works、external IDs、work links 与用户确认 provenance/规则可被既有同步机制处理。
- jobs、candidates、raw evidence、provider cache、fingerprints 与评分缓存不得同步。
- 为新实体实现稳定 key、merge、tombstone、引用完整性与双端冲突测试。
- `confirm_proposal` 只标记 sync-dirty；sync transport 必须保持在既有调度或用户触发路径，不能成为 scrape job 的隐式后续步骤。
- 固化 100 本 corpus、人工真值、运行版本和可复现指标报告。

## Acceptance Criteria

- [ ] 双端确认、离线编辑、unlink、删除、重放同步均不产生重复或悬空关系。
- [ ] 同步包/快照不含任何 scrape working-state 或 Provider 原始缓存。
- [ ] 确认路径与同步传输解耦：同步端点离线时确认仍成功，且 transport instrumentation 显示确认期间请求数为 0。
- [ ] 100 本全部进入 `ready`、`partial`、`ambiguous`、`unmatched` 或 `rejected` 明确终态。
- [ ] 报告给出 title coverage、author coverage、作者/提供者混淆矩阵、标题/章节混淆矩阵、缺失/拒绝原因，并单独记录 AniList/Bangumi coverage。
- [ ] 产品复核报告及 Windows 全链路回归通过后，M8 才可完成。

## Dependencies / Out of Scope

- 依赖 M8-M1 至 M8-M5。
- 质量不足时先改进本地规则和层级关系；不以加入 OCR、CLIP 或额外 Provider 替代基础识别器验收。
