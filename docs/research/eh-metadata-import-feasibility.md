# manifest ↔ RCH 刮削/标签体系：E 站元数据导入可行性调研

- 日期：2026-09-22
- 关联：SPEC §9（M8 智能刮削）、§10（可选插件边界）、`docs/research/eh-subscription-115-feasibility.md`
- 状态：**静态调研 + 实测匹配/翻译验证完成；未写代码**
- 结论：**可行**，但有三处必须先解决的问题（见 §5），其中一处涉及数据结构变更，按项目规则**需你确认**

---

## 1. 结论摘要

| 问题 | 结论 | 依据 |
|---|---|---|
| manifest 能否与 RCH 刮削/标签体系对齐？ | ✅ **应当对齐，且当前字段名不一致** | `[仓]` 刮削已产出 `work_title / creators / source_series / resource_language / censorship / resource_tags`；manifest 目前用的是 `title_jpn / tags[]` |
| 本地作品 → E 站同名画廊，能否匹配？ | ✅ **可行，实测 6/7** | `[实测]` 用"作者名或作品名"作锚点搜索 + 字符二元组相似度打分，阈值 0.5 时命中 6/7（详见 §4） |
| E 站标签能否翻成中文？ | ✅ **85%~90% 可译** | `[实测]` EhTagTranslation 数据库 862 条；真实画廊实测 27/30、6 画廊合计 80/94 |
| 未译标签是哪些？ | ⚠️ **是标识型命名空间** | `[实测]` 未命中全部落在 `artist / group / parody / character`（这些本身就是名字，不需要翻译） |
| 直接导入会怎样？ | ⚠️ **会污染标签体系** | `[仓]` `tags` 表是**扁平表**（无命名空间/分类字段），一个作品 27~33 个标签灌进去会淹没用户自建标签 |

**一句话**：**技术上完全可行**（匹配与翻译都有实测支撑），真正要决策的是"**导入成什么形状**"——是原样灌平标签，还是先给标签体系补上命名空间/来源标记。

---

## 2. RCH 现有数据模型（实测，非记忆）

### 2.1 刮削产出（`app/rust/src/scraper.rs` + 真实产出核对）

M8 catalog-only 解析器的产出结构是 **`proposal`（兼容投影）+ `prompt.semantic`（v3 语义）** —— 这一点很关键，
因为 v3 字段**不在顶层**，而是在 `semantic` 子对象里（`docs/reports/catalog-dry-run-2026-08-23-after-semantic-fixes.json`，
schema `rch.catalog-dry-run/v2`）：

```json
{ "book_key": "...", "asset_key": "...", "filename": "05（上+下）.zip", "state": "ready",
  "title": "我的合租女室友是不是过于淫荡了？", "authors": [], "provider": null, "chapter": "5",
  "semantic": { "work_title": "…", "creators": [...], "source_series": [...],
                "resource_language": null, "censorship": null, "resource_tags": [],
                "external_id_candidates": [], "state": "ready", "...共 51 个键" } }
```

`semantic` 里与 EH 命名空间**同构**的字段：

| 刮削字段（`semantic.*`） | 含义 | 对应 EH |
|---|---|---|
| `work_title` | 作品名（与 `publication_title_raw` 分离） | `title_jpn` / `title` |
| `creators[]` | 按角色分离的创作者（circle/artist…） | `group:*` / `artist:*` |
| `source_series[]` | 系列/原作 | `parody:*` |
| `resource_language` | 资源语言 | `language:*` |
| `censorship` | 无修/有修 | `other:uncensored` |
| `color_state` | 彩色状态 | `other:full color` |
| `resource_tags[]` | 资源属性标签 | 其余 `other:*` |
| `translation_state` / `translation_method` | 翻译状态/方式 | `language:translated` |
| `external_id_candidates[]` | 外部 ID 候选 | **正好是放 EH gid 的位置** |
| `state: ParseState` | `Ready/Partial/Ambiguous/Unmatched` | 决定是否够格去外部匹配 |

### 2.1.1 真实语料覆盖度（决定锚点策略的关键数据）

对既有 dry-run 的 **389 条真实 proposal** 统计：

| 字段 | 非空条数 | 覆盖率 |
|---|---|---|
| `semantic.work_title` | **389 / 389** | **100%** |
| `semantic.creators` | **158 / 389** | **41%** |

> **这条数据决定了锚点顺序**：作品名覆盖 100%，作者只覆盖 41%。因此外部匹配必须
> **以作品名为主要锚点**，作者名作辅助（用于同分消歧），而不是反过来。
> 之前的实测（§4.3）也印证：作者锚点对冷门作品召回不足，换作品名锚点能把失败样本救回。

> 注：该 dry-run 的 389 条 `state` 全为 `ready`（是筛选过的样本），因此 `Partial/Ambiguous/Unmatched`
> 的真实占比**尚无数据**——这属于 P3 需要在完整语料上补的测量项。


### 2.2 标签与元数据存储（`app/rust/src/db/mod.rs`）

```sql
book_metas(key, author, genre, series, title, chinese_title, summary, comment, ...)
tags(id, name UNIQUE, created_at, updated_at, deleted)
book_tags(book_key, tag_id, updated_at, deleted)   -- 多对多
```

关键事实：

- `tags` 是**全局扁平命名空间**：只有 `id/name`，**没有** namespace、category、来源（source）字段。
- Dart 侧 `Tag{id,name,createdAt}` 同样没有分类维度。
- 因此：**无法区分"用户手打的标签"与"从 E 站导入的标签"**，也无法把 `female:big breasts` 与用户自建标签 "巨乳" 归并。

---

## 3. E 站元数据形状（实测样本）

以 gid=4202535 为例（30 个标签）：

```
language:chinese        parody:original      group:akatukiya     artist:akatsuki myuuto
male:blindfold  male:bondage  male:multiple orgasms  male:sole male
female:ball sucking  female:big ass  female:big breasts  female:blowjob  female:collar
female:defloration  female:demon girl  female:femdom  female:focus blowjob  female:gokkun
female:harem  female:kissing  female:masturbation  female:monster girl
female:multiple orgasms  female:nakadashi  female:nipple stimulation  female:tall girl
female:very long hair  other:multi-work series  other:uncensored
```

命名空间分布（6 个画廊 94 个标签的实测统计）：

| 命名空间 | 数量 | 性质 |
|---|---|---|
| `male` / `female` | 39 / 15 | **性癖/内容标签**（最需要中文翻译） |
| `language` | 12 | 语言/翻译状态 |
| `other` | 12 | 属性（无修/全彩/系列/AI…） |
| `artist` / `group` | 6 / 2 | **创作者**（对应 `creators`，不需要翻译） |
| `parody` / `character` | 5 / 1 | **原作/角色**（对应 `source_series`） |
| `mixed` | 2 | 组合形式 |

> 重要：`gdata` 返回的标签**带命名空间前缀**，而 RCH 的扁平 `tags` 没有。这是两边最大的结构差异。

---

## 4. 匹配可行性（实测）

### 4.1 搜索锚点：`artist:` 命名空间不可用

| 搜索词 | 结果 |
|---|---|
| `artist:"朝凪"` / `artist:"Fatalpulse"` / `artist:fatalpulse` / `artist:"GSUS"` | ❌ **全部 0 条** |
| 裸词 `朝凪` / `Fatalpulse` | ✅ 各 25 条，且 top1 就是该画师的作品 |

> **结论**：命名空间过滤（`artist:`/`group:`）实测基本不可用；**必须用裸词作锚点**，再在本地打分排序。
> 与之前的发现一致（`rating>=4`、`torrents=1`、`high resolution$` 也都不可用）——**EH 的搜索只可靠支持语言与内容标签**。

### 4.2 打分：字符二元组 Dice 相似度

匹配必须做**归一化 + 模糊打分**，因为本地名与 EH 名存在系统差异：

- 本地文件常带 `[作者]` 方括号块、`(原作)` 括号、`[DL版]`/`[中国翻訳]` 等标记 → 需剥离；
- EH 的 `title` 是**罗马字**，`title_jpn` 才是原名 → **只能用 `title_jpn`**（此前种子映射阶段已实测：用罗马字全部失败）；
- 日文标题里假名/汉字与本地可能有写法差异 → 纯相等不可靠。

采用：`归一化（剥离方括号/括号、仅保留字母数字与日文假名汉字、小写）` + `字符二元组 Dice 系数`。

### 4.3 实测结果（7 个真实本地样本，数据取自项目既有 dry-run 报告）

| 本地作品名 | 锚点 | 最佳分 | 命中 | 最佳候选（E 站 `title_jpn`） |
|---|---|---|---|---|
| 清楚ビッチな巫女先輩1 | KAROMIX | **0.95** | ✅ | `[KAROMIX (karory)] 清楚ビッチな巫女先輩 [中国翻訳] [無修正]` |
| ヒミツの睡眠学習 | Bicolor | **1.00** | ✅ | `[Bicolor (黒白音子)] ヒミツの睡眠学習 [ドイツ翻訳] [無修正] [DL版]` |
| 田舎にはこれくらいしか娯楽がない5 | しゃよー | **1.00** | ✅ | `[陸の孤島亭 (しゃよー)] 田舎にはこれくらいしか娯楽がない5 [中国翻訳] [無修正]` |
| 孕ませ屋2 | なかじまゆか | 0.75 | ✅ | `[Digital Lover (なかじまゆか)] 孕ませ屋4`（同系列，需人工确认） |
| 舞台の裏側1 BEHIND THE STAGE | GSUS | 0.65 | ✅ | `[GSUS] Oshi No Ko BEHIND THE STAGE #2` |
| 人生リサイクル | Fatalpulse | 0.00 | ✗ → **换锚点救回** | 用作品名重搜 → 分 **1.00**，命中 `[Fatalpulse (朝凪)] 人生リサイクル [中国翻訳] [無修正] [DL版]` |
| いっぱいわけてね | Bicolor | 0.00 | ✗ | 作者名锚点只返回该画师最新 10 条（冷门作品不在列），换作品名锚点也无结果 |

**结论与推荐策略（多轮锚点，而非单次搜索）**：

```
1) 锚点 A = 作品名（semantic.work_title，实测覆盖 100%）→ 搜索 → 取前 N 条 gdata → 打分
2) 命中（分 ≥ 阈值）→ 采纳
3) 未命中 → 锚点 B = 创作者名（semantic.creators[].name，实测仅 41% 非空）→ 搜索 → 打分
   注意：作者锚点召回依赖作品热度（实测 Bicolor 冷门作品不在前 10 条内）
4) 仍未命中 → 判定 Unmatched，**不猜**（与 M8「proposal-only、不静默解析冲突」一致 [仓]）
5) 多候选接近（如 0.75 vs 0.7）→ 落"待人工确认"，不自动写入
```

**实测命中率**：单轮锚点（作者）5/7；加"换作品名锚点重搜"后 **6/7**；剩余 1 条为极冷门作品
（`いっぱいわけてね`，作者锚点与作品名锚点均无结果）。
阈值建议 0.5 起步，并在真实语料上校准（避免把"同系列不同卷"误判为同一本——
`孕ませ屋2` vs `孕ませ屋4` 得 0.75 就是风险样例）。

---

## 5. 标签翻译（实测）

### 5.1 数据源

[EhTagTranslation/Database](https://github.com/EhTagTranslation/Database)（GNU FDL 许可，可二次分发）的 markdown 表格：`原始标签 | 中文名 | 描述 | 外链`。

实测抓取结果：

| 命名空间 | 条目数 |
|---|---|
| female | 613 |
| male | 575 |
| language | 87 |
| other | 60 |
| mixed | 23 |
| reclass | 11 |
| **合计** | **862** |

译名样例：`lolicon→萝莉`、`big breasts→巨乳`、`netorare→NTR`、`nakadashi→中出`、`uncensored→无修正`、`full color→全彩`、`ai generated→AI生成`、`chinese→汉语`。

### 5.2 覆盖率实测

- 单画廊（gid=4202535，30 标签）：**27/30 = 90%**
- 6 个画廊合计（94 标签）：**80/94 = 85%**
- **未命中全部集中在 `artist / group / parody / character`** —— 这些是专有名词（画师名、社团名、原作名、角色名），本就不该翻译，而应映射到 RCH 的 `creators` / `source_series` / 标签。

### 5.3 翻译不是"随便取"，有两个坑

1. **译名可能带 emoji/HTML**：实测 `female:kissing → 接吻💏`，需清洗后再存标签名。
2. **同一英文标签在不同命名空间可能不同义**：映射键应为 `命名空间:原始标签` 而非裸标签名。

---

## 6. 推荐数据模型与导入策略（**需你确认的决策点**）

### 6.1 manifest 应当改成"摄入格式"（对齐刮削词汇）

建议 manifest 每条记录增加/改名（保持向后兼容，旧字段保留）：

| 建议字段 | 来源 | 用途 |
|---|---|---|
| `gid` / `token` / `infohash` | 已有 | 唯一标识、去重 |
| `work_title`（= 现在 `title_jpn`） | gdata `title_jpn` | 与刮削 `work_title` 对齐 |
| `title_aliases[]` | `title`（罗马字）+ 各语言标题 | 匹配时的候选别名 |
| `creators[]` | `artist:*` / `group:*` 拆出 `{role, name}` | 对齐 `creators` |
| `source_series[]` | `parody:*` | 对齐 `source_series` |
| `resource_language` / `translation_state` | `language:*` | 对齐同名字段 |
| `censorship` / `color_state` | `other:uncensored` / `other:full color` | 对齐同名字段 |
| `tags[]` | 其余标签 | 原样保留 `命名空间:名` |
| `tags_zh[]` | 翻译库 | 中文展示/检索 |
| `posted` / `rating` / `downloads` | 已有 | 可信度与排序 |

好处：**同一份 manifest 既能给下载器用，也能直接喂给未来的"元数据导入"**，且字段名与刮削产出同构，不需要二次翻译层。

### 6.2 决策点 D1：标签导入成什么形状（三选一）

| 方案 | 做法 | 优点 | 代价 |
|---|---|---|---|
| **A. 原样导入（最省事）** | 把中文译名直接写进扁平 `tags` | 零结构变更 | 一个作品带 27~33 个标签；**无法区分来源**，用户自建标签被淹没；同一标签中英两份（`big breasts` 与 `巨乳`） |
| **B. 命名空间前缀化（推荐）** | 存为 `女性:巨乳` / `属性:无修正` / `作者:朝凪`，并保留 `源:e站` 标记 | 不改表结构（用 name 承载），可筛选、可归并、可回滚 | 标签名变长；现有标签选择器需支持按前缀分组（UI 工作量） |
| **C. 扩表（最规范）** | `tags` 增 `namespace` / `source` / `external_id` 列 | 语义最干净，支持"只清空 E 站标签" | **DDL 变更 + 迁移**，按项目规则需你确认；同步协议要带上新列 |

> 我倾向 **B 起步、C 作为后续**：B 不动表结构即可验证价值，且天然满足"可解释/可回滚"；等语料证明有用再上 C。

### 6.3 决策点 D2：导入的边界

- **只增不覆盖**：绝不改写用户已有的 `author/series/title/summary`；E 站数据只填空白或进"候选"。
- **来源可辨**：导入的标签必须能一键清除（B 方案靠前缀，C 方案靠 source 列）。
- **只读 + 显式触发**：沿用 SPEC §10 的 B1/B2/B4 —— 导入动作由用户显式发起，写的是本地库（这一步是**主程序功能**，不是插件；插件只负责产出 manifest）。
- **离线可复现**：导入应基于**已落盘的 manifest**（含标签快照），而不是每次重连 E 站；否则站点改版就不可复现。

### 6.4 与 SPEC 的关系

"把外部元数据导入本地标签体系"属于**主程序**能力（不是插件），因此：

- 若采纳方案 C（扩表），属架构级变更 → **需你确认 + SPEC/ADR 记录**；
- 方案 B 不改表，但会改变标签命名约定 → 建议在 SPEC §9 增补一段"外部元数据导入的标签命名约定"。

---


---

### 6.5 决策已确认（用户 2026-09-22 答复）与落地方案

| 决策 | 用户答复 | 落地方式 |
|---|---|---|
| D1 标签形状 | **命名空间前缀 + 源分色，`源:e站` 单独置顶一列** | **零表结构变更**：项目已在用前缀命名约定（`TagRepository.isVisibleInTagManager` 已识别 `resource:` / `sequence:` / `publication:` / `release:` / `release-group:` 等）。沿用同一套约定新增 `源:e站` 与 `女性:巨乳` 形式，来源由前缀解析得出 |
| 隐藏 E 站书签 | 点击 `源:e站` 后隐藏/移除**该漫画**的 E 站导入标签 | 复用现成的 `TagRepository.removeBookTagsByPrefix(bookKey, prefix)` 与 `removeBookTagsByPrefixAndPrune`，天然按书作用域、可回滚 |
| D2 阈值 | **0.5 起步** | 匹配引擎阈值默认 0.5；0.6~0.8 的同系列不同卷仍不自动写入 |
| D3 导入范围 | **author / series / summary 一起补空白** | 只填空字段，绝不覆盖已有值；写入前落"候选" |
| D4 数据来源 | 只从落盘 manifest 导入（沿用离线可复现原则） | manifest 为唯一输入，不实时连主页 |

#### 颜色约定（按来源分色方框）

| 来源 | 前缀 | 颜色语义 |
|---|---|---|
| 用户自建 | 无前缀 | 默认中性色 |
| E 站导入 | `源:e站`（置顶一行）+ 各命名空间标签 | 独立色系，与用户标签一眼可分 |
| 刮削生成 | `resource:` / `sequence:` / … | 现有色系不变 |

#### 与现有代码的接入点（已实读）

| 需求 | 接入点 |
|---|---|
| 标签可见性规则 | `TagRepository.isVisibleInTagManager(name, metadataNames:)` |
| 按前缀清除（= 隐藏该书的 E 站导入） | `TagRepository.removeBookTagsByPrefix` / `removeBookTagsByPrefixAndPrune` |
| 元数据标签投影 | `BookMeta.metaTags` + `TagRepository.syncMetadataLinks` |
| 详情页标签渲染 | `app/lib/ui/book_detail_page.dart:579`（identitied tags chip 区）与 `:663`（可删标签区） |

> 结论：**方案 B 不需要 DDL 变更、不需要动同步协议**，因为"前缀即命名空间"是本项目既有约定。
> 这也把 §6.2 里 C 方案（扩表）的必要性降为"以后若要按来源做数据库层查询再说"。

#### 本轮（第 48 轮）实际完成的范围

- ✅ EH 订阅：`pages` 上限放开到 1–500（UI 增加 1/2/5/25/100 快捷档 + 自定义输入框）
- ✅ EH 订阅：**主站域名可配**（`rules.host`，默认 `e-hentai.org`）+ **连通性预检**
  （`eh_probe`：主站与 `ehtracker.org` 分别判定，UI 显示"哪一段不通"并给出代理/镜像提示）
- ⏳ 标签前缀/分色/隐藏、元数据导入引擎：设计已定（本节），实现待下一轮

---

## 7. 分阶段建议

| 阶段 | 内容 | 退出条件 |
|---|---|---|
| P0（本轮） | 调研 + 实测（匹配 6/7、翻译 85~90%、字段对齐方案） | ✅ 已完成 |
| P1 | manifest 增补为摄入格式（§6.1，向后兼容） | 旧消费方不受影响；新字段有实测样本 |
| P2 | 离线翻译表内置（862 条 → 资源文件）+ 清洗规则 | 覆盖率与清洗结果可复现 |
| P3 | 匹配引擎原型：多轮锚点 + 打分 + `Unmatched` 不猜 | 在 20~50 本真实语料上给出准确率/误判样本 |
| P4 | 导入策略落地（B 或 C，待你定） + 一键回滚 | 用户确认标签形状与可回滚性 |
| P5 | （可选）导入后的"系列/作者"自动补全与书架联动 | 按届时验收标准 |

---

## 8. 待确认清单

1. **D1 标签形状**：A 原样 / **B 命名空间前缀（推荐）** / C 扩表（需 SPEC+ADR）。
2. **D2 匹配阈值**：0.5 起步是否可接受；是否允许 0.6~0.8 的"同系列不同卷"自动写入（我建议不允许，一律待确认）。
3. **D3 导入范围**：只导标签？还是连 `author/series/summary` 一起补空白字段？
4. **D4 数据来源**：是否接受"只从落盘 manifest 导入"（离线可复现），而不是实时连 E 站。

---

## 9. 自检

- [x] 仓库数据模型核对：`scraper.rs` 结构体、`db/mod.rs` 的 `book_metas/tags/book_tags` DDL、Dart `Tag/BookTag` 模型（**均为源码实读**）
- [x] **产出结构纠偏**：v3 语义字段在 `proposal.semantic` 子对象里（51 个键），不在顶层；真实语料实测 `work_title` 覆盖 389/389、`creators` 158/389 —— 据此把锚点顺序改以作品名为主
- [ ] 未测：`Partial/Ambiguous/Unmatched` 在完整语料中的真实占比（该 dry-run 全部为 `ready`，是筛选过的样本）
- [x] 匹配实测：7 个真实本地样本，单轮 5/7、换锚点 6/7，含失败样本与"同系列误判"风险样例
- [x] 翻译实测：862 条映射；单画廊 27/30、6 画廊 80/94；未命中命名空间已归类
- [x] 反例记录：`artist:` 命名空间搜索不可用（3 个画师全部 0 条）——与 `rating>=4`/`torrents=1` 一致
- [ ] **未做**：真实本地漫画语料上的端到端导入验证（需你确认 D1~D4 后进入 P1）
- [ ] **未做**：标签形状变更（B/C）未落地——按项目规则等待确认

**结论强度**：可行性为**已实测**；导入形状为**待决策**，且其中 C 方案涉及结构变更必须先经确认。
