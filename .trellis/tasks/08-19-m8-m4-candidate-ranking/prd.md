# M8-M4 — Candidate & Explainable Ranking

## Goal

把 M8-M1 的本地名称/角色 proposal 与可选 Provider 候选转成稳定、可解释、可审阅的 proposal；Provider 没有结果时仍保留本地结果。

## Requirements

- 建立 `scrape_jobs`、`scrape_candidates`、`scrape_evidence` 与明确状态机，包含 `local_only`、`provider_unavailable`、`ambiguous`、`unmatched` 等语义。
- ranker 是纯函数，输出总分、逐项评分、证据引用、警告和版本。
- 同输入同版本排序一致，tie-break 稳定；工作态可安全清理和重建。
- title、author、provider、chapter 的角色证据不得在 ranking 中互相覆盖。
- 不使用分数阈值绕过用户确认。

## Acceptance Criteria

- [ ] 标注 fixtures 可重现排序及 score breakdown，Top-1/Top-K 可从结果计算。
- [ ] Provider 部分失败产生 partial result 和逐源原因，本地 proposal 仍可审阅。
- [ ] 角色冲突保留为 warning/conflict，不把 provider 当 author 或 chapter 当 title。
- [ ] 并发、取消、重试和过期 proposal 状态转换有测试。
- [ ] 端到端生成候选期间 canonical 表没有新增或更新。

## Dependencies / Out of Scope

- 依赖 M8-M1 与 M8-M3；M8-M2 canonical schema 只提供后续确认目标，不是 parser 运行前置条件。
- 不实现确认 UI、canonical materialization 或学习排序。
