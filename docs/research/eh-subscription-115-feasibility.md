# EH 订阅 → 本地存档（115 推送搁置）：可行性调研与原型

- 日期：2026-09-22
- 状态：**筛选 + 固定文件夹存档已实测可用；115 推送按用户决定搁置（探针已就绪）**
- 关联：SPEC §12 非目标、M8-M3 插件边界、TODO Backlog
- 产出：`app/rust/examples/eh_subscription_probe.rs`（已实测：basic/search/torrents/scan/collect/init-config）、`app/rust/examples/cloud115_offline_probe.rs`（待凭据）

> **证据标注**：`[实测]` = 本次在本机真实请求验证；`[仓]` = 本仓库文档/代码；`[外]` = 外部公开资料；`[待验]` = 未实证。
> 本文件不含任何 Cookie / token / 直链 / infohash。

---

## 1. 结论（可直接决策）

| 需求条件 | 结论 | 依据 |
|---|---|---|
| 中文（`language:chinese$`）可筛 | ✅ **可行** | `[实测]` 服务端过滤有效，返回 25 条/页且抽样 25/25 全为中文 |
| 覆盖范围（能否翻页拿更多候选） | ✅ **可分页（已修正初版判断）** | `[实测]` 游标在 JS 变量 `nexturl`（`next=<本页最后 gid>`）；跟 6 页得 150 条不重复；`?page=N` 无效。单查询约 1.1 万条候选 |
| 评分 > 4 可筛 | ⚠️ **可筛，但必须本地过滤** | `[实测]` `rating>=4` **不是有效搜索语法**（0 命中）；gdata 提供 `rating`（字符串），需取回后本地比较 |
| 有无修正（`uncensored`）可筛 | ✅ **可行** | `[实测]` 作普通标签词可筛；抽 25 条中 24 条命中 `other:uncensored` |
| 高清标签 | ❌ **不存在（已用有效语法证伪）** | `[实测]` `other:"high resolution"$` → **NO HITS**；同批次对照 `other:"full color"$`（有效标签）**正常返回且样本确实带 `other:full color`** → 语法有效、标签确实不存在 |
| 下载数 > 500 | ✅ **可达，命中率约 1/3** | `[实测]` 端到端实跑：25 候选 → **8 条 ≥500**（32%）；全站 DLs 降序前 71 条中 67 条 ≥500（最高 118,251）；当天新发种子为 19/68/84/167/730/923 |
| 只推"能确认命中目标画廊"的种子 | ✅ **有确定解且已跑通** | `[实测]` 种子名 == gdata `title_jpn`（逐字一致）；infohash 由 gdata `torrents[].hash` 直接给出；映射未确认项会被拒绝落盘（实测 5/14 被拒） |
| 推送到 115 离线下载 | ⏸ **本轮不做（用户决定）** | `[实测]` 官方开放平台 `/open/offline/*` 统一返回 `access_token 格式错误`（未鉴权时不区分路径）；`[外]` 生态 SDK 已封装离线下载。探针 `cloud115_offline_probe.rs` 已就绪，待你日后给凭据再验 |
| **筛完保存到固定文件夹** | ✅ **已跑通（本轮交付）** | `[实测]` `collect` 子命令：9 个 .torrent + manifest.json 落盘，全部为合法 bencode；重跑按 infohash 幂等去重（第二次新增 0） |
| **筛选规则可编辑** | ✅ **已实现** | `[实测]` `init-config` 生成带中文说明的 JSON 规则；改 `search` / `title_markers` / `age_tiers` 后行为随之变化（已实测三组配置） |

**一句话**：**除"高清标签"这一条不成立外（已改用主标题标记 `[Digital]/[DL版]` 近似），其余条件全部成立**——EH 种子是"一画廊一种子"，gdata 直接给出 infohash 与日文原名，映射问题基本消失。**"筛选 → 保存到固定文件夹"已跑通并可重复运行**（实测 9 个 .torrent + manifest，重跑幂等）；**115 推送按你的决定本轮不做**，探针保留待用。

---

## 2. 本轮实测证据（可复现命令）

### 2.1 gdata 契约（原实现被证伪并修正）

```
POST https://e-hentai.org/api.php
Content-Type: application/json
{"method":"gdata","gidlist":[["4202535","2b4fcbcc38"]],"namespace":1}
```

| 错误写法 | 实际返回 | 结论 |
|---|---|---|
| `GET /api.php?method=gdata&gid=..&token=..` | `{"error":"Empty JSON Request"}` | GET 查询串不被接受 |
| POST body 用 `gid` 字段 | `{"error":"gdata request needs a gidlist"}` | 字段名必须是 `gidlist` |
| POST body `{"method":"gdata","gidlist":[[gid,token]],"namespace":1}` | `{"gmetadata":[{...}]}` | ✅ 正确契约 |

**字段类型（`[实测]`）**：`rating` / `torrentcount` / `filecount` 是**字符串**（如 `"4.80"`、`"2"`）；`filesize` / `posted` 是数字；
**不存在 `language` 字段**（语言在 `tags` 的 `language:*`）；无种子画廊 `torrents` 为 `[]`。
`title` 是**罗马字**，`title_jpn` 才是**日文原名**。

### 2.2 搜索语法（有效 / 无效清单）

| 语法 | 结果 |
|---|---|
| `/?f_search=language:chinese$`（GET） | ✅ 25 条/页（POST 到 `/` 只返回首页 → **必须 GET**） |
| `language:chinese$ uncensored` | ✅ 命中且服务端真的过滤（抽检 24/25 带 `other:uncensored`） |
| `other:"full color"$` | ✅ 有效（抽检 6/6 带 `other:full color`）——**多词标签必须加引号 + `$` 精确匹配** |
| `other:"uncensored"$` | ✅ 有效（抽检 5/6 带 `other:uncensored`） |
| `other:"high resolution"$` | ❌ **NO HITS** → 该标签不存在 |
| `rating>=4`、`rating:4`、`rated:4`、`rating_count>=20` | ❌ 全部 No hits（**评分不是搜索维度**） |
| `torrents=1`、`torrents:1` | ❌ 全部 No hits |
| `reclass:uncensored$` | ❌ No hits（但 `other:uncensored` / 裸词 `uncensored` 有效） |
| 裸词 `full color$`（无引号） | ❌ No hits → 多词标签**必须加引号**，不加引号的 `$` 会变成无效精确匹配 |
| 裸词 `full color` / `high resolution` / `resolution`（无 `$`） | ⚠️ 返回 25 条但**抽样标签完全不对应**（把 `resolution` 查成了无关集合）→ **不带 `$` 的裸词结果不可信** |

> 结论：`rating` 与 `torrents` 必须在取回 gdata 后**本地过滤**；"高清"需换口径（**建议替代：`other:"full color"$` 全彩**，或文件大小/页数阈值）。
### 2.3 种子 / 文件清单 / 映射（关键：比预期简单）

真实输出（gid 已脱敏为示例，数值为实测值）：

```
[D0] rating=4.80 torrentcount=2
     title_jpn=[赤月屋 (赤月みゅうと)] 僕にしか触れないサキュバス三姉妹に搾られる話4〜... [中国翻訳] [無修正] [DL版]
[D1] 页面统计 2 组： Seeds=124 Peers=4 Downloads=924 / Seeds=67 Peers=13 Downloads=5
     #0 infohash=<gdata 直接给出> downloads=924
         name=<与 title_jpn 逐字一致的 .zip>
         torrent 文件数=0（单文件 zip）   [映射] title_jpn 归一化包含于种子名: true
     #1 downloads=5
         torrent name=e-hentai Torrent File（上传者自制，内含 .rar）
         [映射] false
```

由此确认：

1. **一次 gdata 调用即可拿到**：infohash、种子名、`rating`、`torrentcount`、tags——**不需要解析种子文件就能做映射**。
2. 下载数在 `gallerytorrents.php?gid=&t=` 页面（`Seeds: n Peers: n Downloads: n`），**每个 gid 一次请求**。
3. 种子本体是**单文件 zip/rar**，即使用户要求"文件清单确认"，也只需解析 `info.name`（单文件）——多文件种子是少数上传者自制包。
4. 映射判定必须用 **`title_jpn`**（用罗马字 `title` 实测**全部失败**）。

### 2.4 全站热门对照（回答"500"合不合理）

- `/torrents.php?o=cd` = 全站按 Downloads 降序；列名 `Added / Torrent Name / Gallery / Size / Seeds / Peers / DLs / Uploader`。
- 前 71 条：**67 条 DLs ≥ 500**，最高 118,251，全部是**单画廊 zip**（无"整批归档包"现象）。
- 前 25 条联合 gdata：**rating 全部 ≥4**（4.61–4.86），但 **中文/无修/高清标签命中数 = 0/0/0** —— 头部热门被日文原版占据。
- 中文+无修候选（25 条）：有种子 23、评分≥4 16；抽样下载数 **19 / 68 / 84 / 167 / 730 / 923**。

**推论**：用户的四条条件（中文 + 无修 + 评分>4 + DLs>500）**同时成立是可能的，但属于小众交集**；若要求"每天都有推送"，命中率会很低。**这不是技术不可行，而是阈值取向问题。**

### 2.5 115 侧

- `[实测]` 官方开放平台 `proapi.115.com/open/*`：任何路径（含我编造的假路径）在无 token 时都返回
  `{"state":false,"message":"access_token 格式错误","code":40140123}` → **未鉴权时无法用状态码区分路径是否存在**。
- `[实测]` `webapi.115.com/offline/list`、`/offline/add_task_url` 无 Cookie 时返回错误页；`/files` 正常 → 离线端点**需要凭据**。
- `[外]` 官方开放平台与生态 SDK（[115open-js-sdk](https://github.com/dustink66/115open-js-sdk)、[p115client](https://github.com/ChenyangGao/p115client)、[115-MCP-Server](https://github.com/lingwunb666/115-MCP-Server)）均提供离线下载能力。
- `[仓]` RCH 已有 115 两种凭据链路（开放平台 APP ID / 扫码 Cookie）与限流、WAF 冷却实现
  （`.trellis/tasks/archive/2026-08/08-03-m6-netdisk-official-api/research/115-openapi-contract.md`）。

→ **唯一剩余未知**：加任务端点的准确路径/参数与配额。用 `cloud115_offline_probe -- open|web` 一次性钉死（默认只读；无效磁力不会产生任务）。

### 2.6 限流观察

`[实测]` 全程约 60+ 次请求、间隔 ≥2s，**未出现 403/429/挑战页**；同一查询重复三次返回完全一致的 74,056 字节。
`[实测]` 一度观察到"搜索只剩 25 条/1 页"的现象，复测证明是**我的分页解析把 `inline_set=` 里的 `page=` 误当页数**，不是限流 —— 记录此点以免后续误判。

---

### 2.7 端到端实跑（修正后流程的真实产出）

命令：`cargo run --release --example eh_subscription_probe -- scan --pages 1 --min-dl 500`

```
候选 25 → 评分≥4 16 → 有种子可查 16 → 下载数≥500 的 8 条
```

- **命中率 8/25（32%）**：仅取 1 页候选即可筛出 8 条满足"中文 + 无修 + 评分>4 + DLs>500"的画廊。
- 通过者的评分区间 4.04–4.83，下载数区间 543–874（同页也出现 70 / 378 / 473 / 487 等不达标的）。
- **映射自动确认 5/8**，另 3 条 `map=false` 需人工确认——即"只推可确认命中"的策略在真实数据上是**有区分度的**，
  不是"要么全过要么全不过"（失败样本多为种子名使用罗马字/不同前缀的版本）。
- 每页成本：1 次搜索 + 1 次 gdata 批量 + 约 16 次种子页 ≈ 18 次请求，间隔 ≥2s ≈ 45 秒/页。

> 记录口径：本文件只保留聚合数值与结论，不保存 infohash / 直链 / 凭据。

---

### 2.8 本轮交付：可编辑规则 + 固定文件夹存档（已实测）

**范围决定（2026-09-22）**：115 端先不做，**先做"筛选 → 保存 .torrent 到固定文件夹"**，后续操作留给用户手工。

#### 用法

```bash
cd app/rust

# 1) 生成可编辑规则文件（带中文说明）
cargo run --release --example eh_subscription_probe -- init-config --init-config eh_rules.json

# 2) 按规则筛选并保存到固定文件夹
cargo run --release --example eh_subscription_probe -- collect --config eh_rules.json --out D:/eh_torrents
```

#### 规则文件（可改标签与下载次数，无需改代码）

```json
{
  "search": "language:chinese$ uncensored",
  "min_rating": 4.0,
  "title_markers": "Digital|DL版|DL",
  "exclude_markers": ["AI Generated"],
  "age_tiers": [
    { "min_age_years": 5.0,   "min_downloads": 800 },
    { "min_age_years": 2.0,   "min_downloads": 500 },
    { "min_age_years": 0.5,   "min_downloads": 300 },
    { "min_age_years": 0.083, "min_downloads": 100 },
    { "min_age_years": 0.0,   "min_downloads": 0 }
  ],
  "out_dir": "eh_torrents",
  "request_interval_secs": 2.5
}
```

- `search`：EH 搜索语法（可换标签，如 `other:"full color"$`、`language:japanese$`）
- `title_markers`：主标题标记白名单（`any` = 不筛）；实测 `[Digital]/[DL版]` 在**标题**里（13/25），不在标签里（1/25）
- `age_tiers`：**时间越久要求越高**；按 `min_age_years` 从大到小取第一个满足项
- 最年轻一档默认 `min_downloads: 0` 是**有意设计**：新发种子下载数天然是个位数（实测首页候选仅 18–287），
  靠"重复扫描 + manifest 按 infohash 去重"自然实现**"累积到达标线才保存"**

#### 产出物

```
<out>/<infohash>-<画廊名>.torrent      # 合法 bencode，可直接给下载器/网盘
<out>/manifest.json                    # 去重与审计依据
```

manifest 字段：`infohash / gid / title / title_jpn / rating / downloads / age_years / required_dl / posted / file / bytes / source_url / saved_at`

#### 实测结果（三组配置，证明规则真的生效）

| 配置 | 结果 | 说明 |
|---|---|---|
| 默认规则（中文+无修+评分≥4+[Digital/DL版]+分档下载数） | **保存 9 个**；拒绝分项：评分不足 9 / 标记排除 3 / 无种子 0 / 下载不达标 0；映射未确认跳过 5 | 全部为合法 bencode |
| 改 `search=other:"full color"$` + `title_markers=any` + 近期档 300 | 保存 0；拒绝分项：**评分不足 16 / 下载数不达标 8**（诊断输出 `dl=85/18/287/150/114 < need=300`） | 证明规则被读取并生效；失败原因是"全彩中文首页均为当天新发" |
| 同配置**重跑一次** | **新保存 0**（逐条 `已存在，跳过 <infohash>`），文件数仍为 9 | 幂等去重成立 |

**缺陷修复记录**：首轮保存的文件名出现 HTML 实体（`&#039;`），已加实体解码并复验为 0 处残留。
**证据盲区修复记录**：初版只输出"条件未命中 N"，无法判断是哪条规则导致 0 命中；已改为**分项拒绝统计 + 前若干条拒绝原因**（这是"规则可编辑"的必要配套）。

#### 时间衰减是否成立（实测支撑）

- 全站按日期回溯样本：**五年前的种子下载数 4005 / 4048 / 7982**；同期新种子（2026 年）中位数 **24**，仅 11% 达到 800。
- 结论：**画龄与累积下载数强相关**，"时间越久要求越高"的方向正确。
- 需要如实说明的样本局限：能回溯到的五年前与中文/无修条件**同时命中**的样本只有 3 条，因此
  **"五年前 800" 这条阈值本身尚未被充分校准**（现有样本都远高于 800）。建议先按默认跑一段时间，用 manifest 的实际分布再调。

---

### 2.9 搜索分页机制（第二轮实测修正，覆盖面 ×2~6）

| 尝试 | 结果 |
|---|---|
| `?page=2&f_search=<q>`（常见直觉） | ❌ 返回与第 1 页**完全相同**的内容（同一批 gid、同样 73,131 字节）→ 该参数无效 |
| 页面上的 `<a>Next</a>` / `id="next"` 锚点 | ❌ **不存在**（HTML 里根本没有分页锚点） |
| **JS 变量 `nexturl`** | ✅ **真实分页游标**：`var nexturl="https://e-hentai.org/?f_search=...&next=4201693"`；`next=<本页最后一个 gid>` |
| 跟随 `nexturl` 连续翻页 | ✅ 实测 6 页 → **150 条不重复画廊**（gid 区间逐页向旧推进：4204755 → 4201660 → 4197109 → 4191445 → 4186682 → 4182718） |
| 同段脚本的 `maxdate` / `mindate` / `rangeurl` | ❌ 服务端**不生效**：`&maxdate=`、`&f_dd=` 等参数对结果无影响（结果数恒为 "Found about 12,000 results"） |

**修正后的探针实跑**（`collect --pages 3`）：

```
page 1: 本页新增 25 条（累计 25）
page 2: 本页新增 25 条（累计 50）
候选合计 50 条（2 页）→ gdata 50 条
新保存 13 个 .torrent；映射未确认跳过 8
拒绝分项：评分不足 16；标记规则排除 14；无种子 0；下载数不达标 0
```

对照修正前（单页 25 候选 → 保存 9 个）：**2 页即把落盘量从 9 提到 13**，且第 2 页的新画廊确实被纳入（`French letter`、`茶の魔王`、`UU-ZONE` 等），证明翻页链路真实可用。

规模与成本：单查询 **"Found about 11,000 results"**（约 1.1 万条候选 ≈ 440 页）；每页成本 ≈ 1 次搜索 + 1 次 gdata 批量 + N 次种子页。

> 结论修正：报告初版把"分页不可用"当成限制（R8），**该判断已被推翻**。真实限制是"没有日期窗口过滤"（R9），所以画龄只能本地按 `posted` 判断。

### 2.10 人工核对参数发现的缺陷（第 3 轮修复）

用户要求核对落盘参数（名称/标签/下载数/发布时间）时，暴露出一个**静默失效**的缺陷：

| 缺陷 | 现象 | 根因 | 修复与验证 |
|---|---|---|---|
| **`posted` 解析错误 → 分档阈值从未生效** | 13 条全部显示 `发布=1970-01-01 UTC`、`画龄=0.0 年`，于是**永远落在最宽松档 `need=0`** | gdata 的 `posted` 实测是**字符串**（`"1790042681"`），而代码用 `as_i64()`（只认数字）→ 静默返回 `None` → 被 `unwrap_or(0)` 吞掉 | 新增 `int_field()` 统一兼容字符串/数字；`posted_of()` 解析失败返回 `None` 并在清单标注"未知"，**绝不静默当 0**。修复后 `posted` 显示真实日期（2026-09-19~22） |
| manifest 字段不足以核对 | 落盘记录没有标签/发布时间/体积，无法人工复核 | 只存了少量字段 | manifest 增补 `tags / category / filecount / filesize / uploader / torrentcount / torrent_name / posted_utc / age_years / saved_at_utc` |
| 文件名含 HTML 实体 | 文件名出现 `&#039;` | gdata 标题带 HTML 实体 | `sanitize_filename` 先解码实体（复验 0 处残留） |

**分档机制验证**（修复后新增 `now_offset_days` 调试开关，因为现场数据全是 1–3 天的新作，够不到高画龄档）：

```
now_offset_days=0    → 13 条 age≈0.0y，need=0，全部通过
now_offset_days=2000 → 同一批 13 条 age≈5.5y，need=800，仅 2 条通过（下载 926 / 875），12 条被拒
```

即：**"时间越久要求越高"确实会改变行为**，不是死代码。参数清单见
`docs/research/eh-saved-torrents-2026-09-22.md`（含逐条完整标签）。

> 这一条也是对报告自身的修正：§2.8 里"时间衰减实测支撑"当时引用的是**外部 Python 采集**的数据，
> 它证明了两者的**相关性**，但**没有**证明探针自身的画龄逻辑可用。现在两者都有实测了。

---

## 3. 最终流程（已实现，对应 `collect` 子命令）

```
1) 搜索（GET）：/?f_search=<rules.search>
   分页 = 跟随页面脚本里的 nexturl（next=<本页最后 gid>），实测 6 页拿到 150 条不重复
   ⚠ 服务端不支持日期窗口（&maxdate / &f_dd 无效），画龄只能本地按 posted 过滤
   ⚠ 单查询总量约 1.1 万条（"Found about 11,000/12,000 results"），25 条/页
2) gdata 批量（POST，每次 ≤25 个 gid）→ rating / torrentcount / tags / title_jpn / posted / torrents[].hash
3) 本地过滤（服务端做不到的都在这里）：
     rating ≥ rules.min_rating
     标题标记符合 rules.title_markers，且不含 rules.exclude_markers
     torrentcount > 0（无种子直接跳过，省掉后面所有请求）
4) 每个通过者请求一次 gallerytorrents.php 取 Downloads（与 rules.required_dl(画龄) 比较）
5) 映射确认：title_jpn 归一化后须能在种子名中命中，未确认则不落盘
6) 保存：<out>/<infohash>-<画廊名>.torrent + <out>/manifest.json（按 infohash 幂等去重）
7) （搁置）推送 magnet:?xt=urn:btih:<infohash> → 115 离线下载
```

**请求量估算**：1 次搜索 + 每 25 候选 1 次 gdata + 每个"评分达标且有种子"的候选 1 次种子页。
实测 25 候选约 18 次请求，按默认 2.5s 间隔 ≈ 45 秒/轮，全程未触发 403/429。

---

## 4. 口径决定（2026-09-22 已定，替代原"待你定"）

| # | 问题 | 你的决定 | 落地情况 |
|---|---|---|---|
| D1 | "高清"没有对应标签（已证伪） | 用主标题标记 **`[Digital]` 或 `[DL版]`** 近似 | ✅ 已实现为 `title_markers`（可编辑；实测标记在标题里而非标签里），并附 `exclude_markers` 可排 AI |
| D2 | 下载数阈值 | **分时间段，时间越久要求越高；先定"五年前 800"** | ✅ 已实现为 `age_tiers` 数组；默认 `5年:800 / 2年:500 / 半年:300 / 1月:100 / 新发:0`；实测方向正确（老种子 4005–7982 vs 新种子中位 24），但 800 本身样本不足未校准 |
| D3 | 评分票数门槛 | 未单独答复 | 默认仅用 `min_rating`（0 票画廊 rating=0.00 会被自动排除）；票数门槛留待后续 |
| D4 | 115 推送 | **本轮不做**，先保存到固定文件夹 | ✅ 已实现 `collect` 落盘 + manifest；115 探针保留待用 |

---

## 5. 实现边界与分阶段方案

### 5.1 推荐边界（无论最终形态如何都成立）

| # | 边界 | 理由 |
|---|---|---|
| B1 | **只读来源**：仅访问 EH 元数据接口与 .torrent 文件；不读图片、不解析压缩包内容 | 与 SPEC §9.1「刮削不读远程内容」一致；也避免触碰图库配额 |
| B2 | **写出口唯一**：只写用户指定的输出目录（.torrent + manifest.json）；不写 RCH 的 canonical metadata、不动书架数据库 | 插件与主程序解耦，可随时停用/删除 |
| B3 | **独立配置**：规则全在可编辑 JSON（搜索语法/评分/标题标记/分时间段下载数/页数/间隔） | 你明确要求"可自定义标签和下载次数"；也避免阈值硬编码导致静默筛空 |
| B4 | **可解释**：每条落盘记录带来源 URL 与命中数值；每轮输出分项拒绝统计与原因样本 | 实测教训：只有"未命中 N"时无法定位是哪条规则的问题 |
| B5 | **限流优先**：请求间隔可配（默认 2.5s）、gdata 批量 ≤25/次、不重试不放大 | 本仓库 115 风控事故的教训（问题记录_2026-08-08 §5） |
| B6 | **默认不自动运行**：由用户显式执行（或日后显式授权的计划任务）；输出目录由用户指定 | 成人内容与第三方站点 ToS 风险由用户知情决定 |
| B7 | **不进入主阅读链**：如果要升级为 RCH 功能，必须按插件边界立项，并经你确认 SPEC 增补（§12 现为非目标） | CLAUDE.md：架构级新增需先确认 |

### 5.2 分阶段方案

| 阶段 | 内容 | 退出条件 | 状态 |
|---|---|---|---|
| **P0 原型（本轮）** | 实测契约 + 可编辑规则 + 筛选落盘到固定文件夹 | 真实数据落盘、重跑幂等、规则改动可见生效 | ✅ 已完成 |
| **P1 实跑校准** | 按默认规则定期手动跑 1–2 周，用 manifest 的实际分布校准阈值 | `age_tiers` 各档命中率与人工抽查符合预期；确定 `pages` 取值 | 待你使用后反馈 |
| **P2 自动化（可选）** | 定时执行（Windows 计划任务 / 循环模式），新达标自动落盘 | 连续多日无人值守运行、无 403/429、无重复文件 | 待你决定 |
| **P3 115 接入（搁置）** | 用 `cloud115_offline_probe` 钉死加任务契约 → 把落盘的 magnet 推给 115 | P1–P6 待验项全部有实测结论 | ⏸ 按你的决定暂缓 |
| **P4 产品化（远期）** | 若升级为 RCH 插件：订阅管理 UI、失败重试、配额面板、SPEC 增补 | 你确认形态与 SPEC 变更 | 未启动 |

> 关键判断：**P0 已经能独立产生价值**（把符合规则的种子自动收进固定文件夹），不依赖 115 与 RCH 改动。P1 的校准数据会决定 P2/P3 是否值得做。

---

## 5.3 已落地：RCH 内置面板（P4 提前，2026-09-22）

用户确认要在 RCH 内部打开，因此按 §5.1 的插件边界做了**设置页内的独立面板**（不碰主阅读链）。

### 实现位置

| 层 | 文件 | 说明 |
|---|---|---|
| Rust 核心 | `app/rust/src/eh_subscription.rs` | 规则模型、gdata/分页/种子页解析、Bencode 前的映射判定、落盘与清单；**10 个单测**覆盖规则分档、`posted` 字符串解析、分页游标、映射、文件名安全化 |
| Rust 桥接 | `app/rust/src/api/eh_subscription.rs` | 7 个 FRB 接口（默认规则/读写规则/进度/取消/扫描/清单），阻塞 HTTP 一律 `spawn_blocking`，JSON 过桥避免无谓 DTO |
| Dart 状态 | `app/lib/store/eh_subscription_store.dart` | 规则以 Rust JSON 为单一事实来源；600ms 轮询进度；清单解析 |
| Dart UI | `app/lib/ui/eh_subscription_panel.dart` | 设置页卡片面板：规则编辑、分档表格、目录选择、运行/停止、进度分项、已保存列表 |
| 接线 | `app/lib/ui/home_page.dart:1539` | 挂在「智能刮削」之后，同类可选功能 |

生成的 FRB 绑定：`app/lib/src/rust/api/eh_subscription.dart`（`ehDefaultRules / ehLoadRules / ehSaveRules / ehProgress / ehCancel / ehCollect / ehManifest`）。

### 界面要点（视觉已核对）

- **规则全部可视化可编辑**：搜索语法、评分滑杆、标题标记白名单、排除标记、**分档表格可增删档位**、保存目录、每轮页数、请求间隔；底部有「恢复默认规则」。
- **可解释的进度**：运行中显示阶段与 7 个分项计数（候选 / 本轮新增 / 评分不足 / 标记排除 / 无种子 / 下载数不达标 / 映射未确认），让用户一眼看出是哪条规则挡住的结果。
- **已保存清单**：每条显示评分、下载数与阈值、发布时间、画龄、分类、页数、体积、标签数、种子文件名，并可一键打开目录。
- 面板测试注入口：`EhSubscriptionStore.forTest` + `EhSubscriptionPanel(storeOverride:)`，使视觉核对可在**无设备/无 FFI** 环境下确定性渲染。

### 视觉核对证据

`app/test/eh_subscription_panel_preview_test.dart` 渲染 1280×2400 面板并写出 `app/build/eh_panel_preview.png`
（121,308 字节，两次渲染字节一致 = 确定性）。测试环境默认占位字体（Ahem）会把所有文字渲染成方块，
因此测试内 `FontLoader` 加载微软雅黑/等线后再出图——**否则"看起来对了"是假象**。
该测试同时断言关键控件存在，避免"截了一张空白图"当证据。

### 与 SPEC 的关系（需你确认）

SPEC §12 现为非目标「不做在线漫画站爬虫/聚合」。本次实现是**设置页内的独立插件面板**：
只读 EH 元数据与 `.torrent`、只写用户指定目录、默认不运行、不触碰书源/目录库/阅读数据。
建议在 SPEC 中新增一节「可选插件：外部订阅源」，写明上述边界；**该 SPEC 修订仍待你确认**。

---

## 6. 风险清单（更新）

| # | 风险 | 依据 | 缓解 |
|---|---|---|---|
| R1 | 115 WAF 风控（本项目曾触发 IP 级 405） | `[仓]` 问题记录_2026-08-08.md §5 | 本轮不做 115；若日后接上：只对"确认命中"发 1 次加任务，复用既有 1.5/s 门与 405 冷却，禁止多端点重试 |
| R2 | EH 限流/挑战 | `[实测]` 目前 2s 间隔正常（全程 403/429 = 0） | 保持 ≥2s（可配 `request_interval_secs`）；gdata 批量（≤25/次）压请求数；失败快速失败 |
| R3 | 映射误判 | `[实测]` 用 `title_jpn` 命中；罗马字必失败 | 只用 `title_jpn`；映射未确认**不落盘**（实测 5/14 被拒）；manifest 保留 `source_url` 可复核 |
| R4 | 重复保存 | `[实测]` 同画廊可有多个种子 | 去重键 = **infohash**（manifest 持久化，跨次运行）；实测重跑新增 0 |
| R5 | 115 离线配额 | `[待验]` 本轮不做 | 日后再验 |
| R6 | 成人内容与 ToS | — | 默认不自动运行（需显式执行命令）；输出目录由用户指定；不进入主阅读链 |
| R7 | SPEC §12 边界 | `[仓]` SPEC.md:232「不做在线漫画站爬虫/聚合」 | **若要从探针升级为产品功能，必须按插件边界立项并经你确认 SPEC 增补** |
| R8 | ~~搜索只能取最新 25 条~~ **（已修正）** | `[实测]` 分页**可用**，游标在 JS 变量 `nexturl` 里（`next=<本页最后 gid>`），不是 `?page=N` | 已实现跟随 `nexturl` 翻页；`pages` 可配。全查询约 1.1 万条 ≈ 440 页，按需设页数 |
| R9 | **服务端不支持日期窗口** | `[实测]` `&maxdate=` / `&f_dd=` 等参数对结果无影响（恒 "Found about 12,000 results"） | 画龄过滤只能本地做（gdata 的 `posted`）；"只看近 N 天"需自行按 posted 过滤 |

---

## 7. 待验清单（拿到凭据即可闭环）

| 编号 | 待验项 | 命令 |
|---|---|---|
| P1 | 115 官方开放平台加离线任务的**准确路径与参数** | `cargo run --release --example cloud115_offline_probe -- open` |
| P2 | 开放平台是否给离线下载 **scope**（APP ID 权限） | 同上（`/open/user/info` 与 `/open/offline/*` 的响应差异） |
| P3 | webapi 路径与 `wp_path_id` 参数形状 | `... -- web` |
| P4 | 离线任务列表字段（去重索引依赖） | 同上 list 分支 |
| P5 | 离线**配额**与并发上限 | 同上 quota 分支 |
| P6 | 磁力能否真的拉下来（EH 种子做种数实测 124/67/34 不等，活跃度够） | `... -- open --add "magnet:?xt=urn:btih:<真实>"`（需你授权） |

---

## 8. 自检

- [x] EH 侧实测：搜索语法（含同批次控制组与负对照）、gdata 契约（纠正三处错误假设）、字段类型、种子页统计、infohash、bencode 文件清单、`title_jpn` 映射 —— **均有真实响应**
- [x] 探针可编译（`cargo build --release --examples` exit 0，产物已落盘）且四个子命令均可运行
- [x] **本轮新增交付**：`init-config` 生成可编辑规则 + `collect` 筛完落盘到固定文件夹 + manifest 去重，**三种配置实测行为随之变化**，重跑幂等
- [x] 分布数据：全站 DLs 降序前 71 条 + 中文候选 25 条 + 全彩中文候选 25 条 + 时间回溯样本
- [x] 缺陷与盲区修复：文件名 HTML 实体解码；"条件未命中 N"改为**分项拒绝统计 + 拒绝原因样本**
- [ ] 115 加任务契约（P1–P6）：**本轮按你的决定不做**，保持 `[待验]`，探针已就绪
- [ ] 时间衰减阈值（五年前 800）**尚未充分校准**：可回溯样本仅 3 条（4005/4048/7982），均远高于 800

**结论强度**：
- 筛选 + 保存到固定文件夹：**已实证可用**（真实数据、真实落盘、幂等去重）。
- 115 推送：**未验**（按你的决定搁置），不得当作已通过。
- 阈值校准：**方向已证**（老种子下载数远高于新种子），**具体数字待用实际运行分布回调**。

