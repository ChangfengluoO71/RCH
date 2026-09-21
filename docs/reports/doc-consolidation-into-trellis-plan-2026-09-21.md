# 零散文档整理进 Trellis 体系 —— 方案（2026-09-21）

> 状态：**方案待批**（本文档只做分析与路由，不移动任何现有文件）。
>
> **进展记录（2026-09-21 晚，v0.6.0 发布后）**：
> - 批 1（spec 补实）：**部分完成** —— `.trellis/spec/backend/remote-cover-update-contracts.md`
>   已回写（该契约与代码不一致处改为 *as implemented* 并新增 **Known gaps**），
>   `spec/backend/index.md` 的漏登记已补；**其余模板骨架仍未补实**。
> - 批 2（文件移动进 `.trellis/tasks/*/research/`）：**未开始**（`docs/reports` 36 文件、
>   `docs/superpowers` 8 文件，均原位）。
> - 批 3（14 MB JSON 体积治理）：**未开始**。
> - 动因说明：批 1 的那两处并非为整理而做，而是修 CI 门禁时发现"契约文档描述了未实现的
>   设计"（导致 5 个测试文件写不出来）而必须回写。
> 依据：`.trellis/workflow.md`、`AGENTS.md` 的知识归位规则、以及 `trellis-spec-bootstrap` 技能的五步流程
> （确认初始化 → 分析仓库 → 按包/层分解 → 写实 spec → 校验无占位）。

---

## 1. 现状审计

| 目录 | 文件数 | 体量 | 性质 | 备注 |
|---|---|---|---|---|
| `.trellis/spec/backend` | 13 | — | 规范 | **仅 5 份是实文**（契约 328 行、automation 212、refresh-cleanup 135、local-first 98、115-web 72）；其余 42–62 行多为模板骨架；`index.md` 仍标 `To fill` |
| `.trellis/spec/frontend` | 6 | — | 规范 | `quality-guidelines.md` 150 行（实）；`state-management.md` 82 行（半实）；`index.md` 标 `To fill` |
| `.trellis/spec/guides` | 3 | — | 跨层思考指南 | 223 / 348 / 97 行，均已写实 |
| `docs/project` | 9 | 436K | 施工历史与计划 | `LOG.md` 为主体；`LOG-INDEX` / `TODO` / `CHANGELOG` —— **CLAUDE.md 约定 append-only，位置不动** |
| `docs/development` | 1 | 12K | 工程手册 | `setup.md`（环境/发布/签名/CI 门禁） |
| `docs/releases` | 21 | 77K | 发布说明 | **`release.yml` 按 `docs/releases/release_notes_<tag>.md` 取 Release 正文 ⇒ 路径不可改** |
| `docs/reports/p0·p1·rg-b` | 24 | — | 门禁与实验证据 | P0 读取基线、封面放大、P1 契约与覆盖证明 |
| `docs/reports`（根） | 11 | — | 验收/审计/原始数据 | 含 **6 个 JSON 原始 dump（14 MB 主体）** |
| `docs/superpowers/{plans,specs}` | 8 | 200K | 设计计划与规格 | 含被否方案与演进过程 |
| `docs/{README,architecture,user-guide}.md` | 3 | — | 门面与手册 | 面向使用者 |

**核心矛盾**：Trellis 只有三类承接位（spec / tasks / workspace），而现有文档散在 5 个目录、跨越
"规范 / 计划 / 证据 / 历史 / 手册"五种性质。整理的本质是**按性质归位**，而不是把所有文档塞进 `.trellis/`。

---

## 2. Trellis 三类的职责（来自 `workflow.md`）

| 承接位 | 放什么 | 判定标准 |
|---|---|---|
| `.trellis/spec/<package>/<layer>.md` | **长期有效的规范与契约** | 半年后仍该被遵守；能被"违反/遵守"判定 |
| `.trellis/tasks/<MM-DD-name>/{prd,design,implement,research}` | **一次性任务的产物** | 有明确验收与结束点；做完即归档 |
| `.trellis/workspace/<dev>/journal-N.md` | **会话记录** | 按时间线记录谁做了什么 |

`AGENTS.md` 补充：持久决策 → ADR/architecture；工程规则 → spec；需求与验收 → tasks；
调研与被否方案 → research/design；反复踩的坑 → 测试与 lessons；交接 → workspace 会话记录。

---

## 3. 逐文件路由方案

### 3.1 规范类 ⇒ `.trellis/spec/`

| 现有 | 去处 | 动作 |
|---|---|---|
| `docs/reports/p0/2026-09-17-p0b2-zip-read-amplification.md` 等 P0 结论 | `spec/backend/reader-read-budget.md`（**新建**） | 把"读取预算 / 窗口合并 / 放大上限"的**可判定规则**提炼成规范，原文留在原地作证据 |
| `docs/reports/p1/2026-09-17-p1a-cover-job-transition-matrix.md` | `spec/backend/remote-cover-update-contracts.md`（已存在 ✓） | 状态矩阵与发射契约已在该文件；补充指向证据的链接 |
| `docs/reports/cover-status-flicker-audit.md` | 同上（作为"读路径不得回写状态"的论据） | 规则入 spec，审计原文留作证据 |
| `docs/superpowers/specs/2026-09-14-remote-cloud-scan-design.md` | 任务产物（见 3.2） | 该文是设计而非规范 |
| `docs/architecture.md` | **保留**；要点摘进 `spec/*/index.md` | 架构总览仍是门面文档 |
| `spec` 内的模板骨架（`database-guidelines` / `error-handling` / `directory-structure` / `logging-guidelines` / `frontend/hook-guidelines` / `frontend/type-safety` 等） | 原地**补实或删除不适用小节** | 技能 Done Criteria 明确禁止留占位 |
| `spec/backend/index.md`、`spec/frontend/index.md` 的 `To fill` | 改为真实清单 | 与最终文件集合一致 |

### 3.2 任务产物类 ⇒ `.trellis/tasks/<任务>/research/`

| 现有 | 去处 | 理由 |
|---|---|---|
| `docs/superpowers/plans/*.md`（7 份，日期化） | 对应任务（如 `09-14-remote-cloud-scan/`、`08-02-cover-loading-perf/`）的 `research/` | 它们本就是任务的设计计划，含被否方案 |
| `docs/superpowers/specs/2026-09-14-remote-cloud-scan-design.md` | `09-14-remote-cloud-scan/design.md` | 与任务设计文档同质 |
| `docs/reports/rch-remote-cloud-scan-2026-09-14.md`、`rch-v057-*` 等验收报告 | 对应任务的 `research/` | 验收证据属任务生命周期 |
| `docs/reports/p0|p1|rg-b/*.md`（24） | 对应任务 `research/`（按主题挂到 `09-14-*` / `08-30-*` 等） | 门禁证据随任务归档 |
| 已完成且已归档的旧任务 | `task.py archive` 后自然进入 `tasks/archive/{YYYY-MM}/` | Trellis 既有机制 |

### 3.3 位置不动（有硬约束）

| 现有 | 约束来源 |
|---|---|
| `docs/releases/release_notes_v*.md` | `release.yml` 按该路径取 Release 正文 |
| `docs/project/{LOG,LOG-INDEX,TODO,CHANGELOG}.md` | 根 `CLAUDE.md` 的文档规范（append-only 与固定更新时机） |
| `docs/development/setup.md` | 工程手册；规则摘要另进 spec |
| `docs/{README,architecture,user-guide}.md` | 面向使用者的门面 |
| `SPEC.md` / `DECISION.md`（仓库根） | `CLAUDE.md` 规定 SPEC 原则上不得修改 |

### 3.4 体积治理（单独议题）

`docs/reports/*.json` 共 6 个文件、约 14 MB（`docs/reports` 总量 14M 的主体）。
建议：移出仓库（`docs/reports/data/` + `.gitignore`）或压缩。
**注意**：它们**已进入 git 历史**，仅移动/忽略不会减小历史体积；彻底瘦身需 `git filter-repo` 类操作，
属改写历史，必须单独评估与批准。

---

## 4. 分批执行计划

| 批次 | 内容 | 风险 | 验收 |
|---|---|---|---|
| **批 1** | spec 补实：用真实代码把模板骨架写实、删不适用小节；把 P0/P1 已成契约的结论提炼成 2–3 个新 spec 文件；修两个 `index.md` 的 `To fill` | 低（只增改 `.trellis/spec/`，不动既有路径） | `spec/**` 无占位；每个规则都能指向真实文件/测试 |
| **批 2** | 文件移动：`docs/superpowers/**` 与 `docs/reports/{p0,p1,rg-b}/*` 按任务归位；**同步修正 25 处交叉引用**（含 `ci.yml` 注释、`.trellis/tasks/**`） | 中（引用断裂；需全局检索 + 逐条改） | `grep -rn "docs/reports\|docs/superpowers"` 无失效引用；GitHub 上旧路径有跳转说明 |
| **批 3** | 体积治理：6 个 JSON 移出仓库 + `.gitignore`；是否瘦身历史另议 | 中（历史改写属不可逆） | 仓库工作树无大 JSON；文档说明历史体积现状 |

**回滚**：每批独立提交；批 2 用 `git mv` 保留可追溯性。

---

## 5. 不做的事

- 不改 `docs/releases/` 路径（CI 依赖）、不改 `docs/project/` 四件套（CLAUDE.md 约定）、不动 `SPEC.md`。
- 不把"证据/历史"伪装成"规范"：证据只做**引用**，规范只收**可判定规则**。
- 不在本方案批准前移动或删除任何现有文件。

---

## 6. 待批问题

1. 是否按批 1 → 批 2 → 批 3 顺序执行？或只做批 1？
2. 批 2 的目标结构是否认可（`docs/superpowers` 与 `docs/reports/{p0,p1,rg-b}` 全部并入 `.trellis/tasks/*/research/`）？
3. 批 3 的历史瘦身是否需要（不可逆，需单独评估）？
