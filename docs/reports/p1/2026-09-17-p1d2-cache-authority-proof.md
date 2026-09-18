# P1-D-2 cache-authority proof 报告

> 工作目录 `D:/Projects/RCH-p1`｜分支 `p1-cover-completion`｜HEAD `1bf2e3743c026581ddaa9d444787fde05fd02f31`（未变）
> **本轮对生产代码零修改**（全部为只读追踪）｜未 commit / push / merge / reset / stash
> 未进入 P1-E / P1-F / P2；**local A 的代码修改按你的要求一并暂缓**

## 结论（先说）

> **`P1_D2_CACHE_AUTHORITY_AMBIGUOUS: SFTP`**

五个 provider 中 **4 个通过**（WebDAV / Baidu / 115 / Quark —— 其中 115 有 app 与 web 两种模式，
两条 authority 路径都通过，故表内共 6 行、5 行通过），**SFTP 无法证明** writer 与 proposed reader
会得出**完全一致**的 cache identity。

按你 §7 的明确要求：**不用"多数 provider 可以"掩盖问题** → P1-D-2 legacy 方案 B **整体不能判 PASS**，
在此 STOP，**不自行**改 cache schema、不引入 cache index、不持久化 session metadata、不做旧缓存迁移。

---

# 1. 五 provider cache-authority table（writer → key → reader 反向追踪）

关键前提（先证）：**所有 provider 的 key 后半段都是可 sessionless 调用的自由函数**
`raw_cache_path(origin_or_endpoint, path)` → `cache_hash(...)` → `CacheDir::Raw`，
例如 `sftp::cache_hash(endpoint, path)` / `raw_cache_path(endpoint, path)`
（`sftp.rs:300-315`）**不持有任何 client/session**。cover API 把这个 raw path 的
**字符串**再交给 `cache::cover_cache_read/write(path, page, w, h, crop)` 作为 cover 缓存键
（`webdav_cover` source.rs:806-814、`sftp_cover` :1079-1115）。

**因此唯一可能不共享的部分就是 `origin` / `endpoint` 的 derivation。**

| Provider | Cache writer | Writer 输入的 authority | Authority 来源 | Canonicalizer / helper | Durable in `book_sources`? | Sessionless 可复用同一 helper? |
|---|---|---|---|---|---|---|
| **WebDAV** | `cache::cover_cache_write(raw_path,…)`（raw 由 `webdav::raw_cache_path` 探测/生成） | `origin = scheme://host[:port]` | **纯字符串**：由 `source.url` 解析 | **内联在 `WebDavClient::new()`**（`webdav.rs:154-159`）+ `normalize_base` | ✅ `path`/`url`（`db/mod.rs:163`） | ✅ **通过**：`WebDavClient::new(base,user,pass)` **不触网**（只建 reqwest Client + 解析 URL，`:149-176`）→ reader 可调用**同一个** `new()` 再取 `origin()` |
| **SFTP** | 同上（`sftp_source::raw_cache_path`） | `endpoint = host`（port 22）否则 `host:port` | `host`/`port` | ❌ **内联在 `SftpClient::connect()` 内、且在 SSH 握手之后**（`sftp.rs:89-98`） | ✅ `path`/`port`（`db/mod.rs:163/168`） | ❌ **不通过**：`connect()` 必然发起 SSH 连接；全仓**不存在**独立 helper（`grep "port == 22"` 全仓仅此一处），reader 只能**复制**该逻辑 |
| **Baidu** | 同上（`baidu_source::raw_cache_path`） | `origin = "baidu:{app_key}:{root}"` | `app_key` + `root` 字段 | `BaiduClient::origin()`（`baidu.rs:181-183`，纯格式化） | ✅ `client_id` / `path`(root) | ✅ **通过**：`BaiduClient::new()`（`baidu.rs:164-178`）不触网（`access` 初始为 `None`，refresh 是独立方法） |
| **115（官方 APP）** | 同上（`cloud115_source::raw_cache_path`） | `origin = "115:{app_id}:{root_id}"` | `app_id` + `root_id` 字段 | `Cloud115Client::origin()`（`cloud115.rs:309-311`） | ✅ `client_id` / `root_id` | ✅ **通过**：`Cloud115Client::new()`（`:293-306`）不触网 |
| **115（网页 Cookie）** | 同上（`cloud115_source::web_raw_cache_path`） | `origin = "115web:{root}"` | `root` 字段（**cookie 被刻意排除**，注释："Cookie 会变，只用 root 保持稳定"） | `Cloud115WebClient::origin()`（`cloud115.rs:1199-1202`） | ✅ `root_id` | ✅ **通过**：`Cloud115WebClient::new()`（`:1174-1197`）不触网 |
| **Quark** | 同上（`quark_source::raw_cache_path`） | `origin = "quark:{root}"` | `root` 字段（注释同样说明 cookie 轮换、只用 root） | `QuarkClient::origin()`（`quark.rs:258-261`） | ✅ `root_id` | ✅ **通过**：`QuarkClient::new()`（`:241-256`）不触网 |

## 1.1 authority 分类结果

| Provider | 分类 |
|---|---|
| WebDAV | **`DURABLE_DERIVABLE_SHARED`**（durable = `url`；shared helper = `WebDavClient::new` + `origin()`，均不触网） |
| Baidu | **`DURABLE_DERIVABLE_SHARED`** |
| 115 app | **`DURABLE_DERIVABLE_SHARED`** |
| 115 web | **`DURABLE_DERIVABLE_SHARED`** |
| Quark | **`DURABLE_DERIVABLE_SHARED`** |
| **SFTP** | ❌ **`AMBIGUOUS`** |

## 1.2 对照"现有实现都要求 session"这一事实

`open_cached_remote_book`（`source.rs:795`）—— 文档明确写"缓存命中时直接本地打开远程书…全程不联网" ——
但它的**每一个**分支第一步都是取 session（`:803/811/819/828/840/849`），然后才 `client.origin()` /
`client.endpoint()`。**所以整个仓库至今没有一条 sessionless 的 `origin` 获取路径**；
上述 5 个"通过"是我核对**构造函数不触网**后得出的**可复用性**结论，
而 SFTP 连这一点都不成立。

---

# 2. SFTP 的 session-only 最小事实链（为什么不能继续）

```
Dart  comic_cover.dart:607  sftpCover(session:, path:, …)
      └─ comic_cover.dart:603  sftpSessionFor(source)        ← session 获取（网络）
           └─ lib/store/sftp_session.dart:14  _parseHostPort(source)   ← ★ Dart 拥有 host/port 解析
                lib/store/sftp_session.dart:15  sftpConnect(host:, port:, …)
Rust  source.rs:1067  sftp_cover(session, …)
      └─ source.rs:1073  get_sftp_session(session)?          ← session 查找（先于磁盘查询）
      └─ source.rs:1076-1079  client.raw_cache_path(&path)   ← 磁盘查询（依赖 endpoint）
           └─ sftp.rs:114-116  raw_cache_path(&self.endpoint, path)
                └─ sftp.rs:300-315  cache_hash(endpoint, path) → CacheDir::Raw
Rust  sftp.rs:47  SftpClient::connect(host, port, user, pass)
      └─ sftp.rs:89-92  SSH 握手（**必然网络 I/O**）
      └─ sftp.rs:93-97  let endpoint = if port == 22 { host } else { format!("{host}:{port}") }  ← ★
      └─ sftp.rs:100-105  SftpClient { endpoint, … }
```

**最小事实链**：

1. SFTP 的 cache namespace authority = `endpoint`（`sftp.rs:108-116`）；
2. `endpoint` 的**唯一**产出点是 `SftpClient::connect()` 内部（`sftp.rs:93-97`），
   而该函数**必须先完成 SSH 握手**（`:89-92`）——即"不建 session 就拿不到 endpoint"；
3. 全仓**没有**任何可 sessionless 调用的 `endpoint` derivation helper
   （`grep -rn "port == 22"` 在整个 `rust/src` 只命中 `sftp.rs:93` 这一处）；
4. 该 authority 的**输入解析**也不在 Rust：`host`/`port` 由 **Dart** 的
   `_parseHostPort`（`sftp_session.dart:27`）从 `source.url`/`source.port` 解析后传入；
5. 因此一个 sessionless reader 想产出 `endpoint`，只能**复制** Dart 的解析
   **并且**复制 Rust 的 `port == 22` 规范化 —— 两处复制。

按你 §0 的冻结判定（"在新 API 复制一套 URL normalization / 写一个'看起来一样'的第二套
cache-key 算法 → 全部视为失败"）→ **`AMBIGUOUS`**，且按 §3 的硬条件，
"读取用户磁盘上**已存在**的 SFTP legacy 缓存"也因此**无法被证明**。

---

# 3. 你 §3 的硬条件（旧缓存必须可读）逐 provider 状态

| Provider | 旧缓存可读性 | 说明 |
|---|---|---|
| WebDAV / Baidu / 115(app/web) / Quark | ✅ 可证明 | 用**同一** `raw_cache_path`/`cache_hash` 自由函数 + **同一** 不触网构造函数产出的 origin |
| **SFTP** | ❌ **无法证明** | endpoint 只能靠复制逻辑近似；一旦与 writer 有任何偏差（默认端口、host 大小写、IPv6 括号等），旧缓存就会**静默 miss** |

我**没有**采用任何被禁止的替代手法：不新建 cache namespace、不修改 key 只让未来缓存生效、
不要求重新联网迁移、不遍历缓存目录模糊匹配、不按文件时间/大小猜归属。

---

# 4. 最小的两个可选方向（**不擅自选择**，等你决定）

## 方向 1：抽出共享的纯函数（最小、不改变现有 cache identity）

把 `sftp.rs:93-97` 的 4 行规范化提取为 `pub fn endpoint_for(host: &str, port: u16) -> String`，
让 **`connect()` 与新的 sessionless reader 调用同一个函数**；并在 Rust 侧提供与 Dart
`_parseHostPort` **等价**的地址解析（最好由 Rust 拥有、Dart 改为复用，以消除第二份实现）。

- 优点：writer 与 reader 的 cache identity **按构造相同**，旧缓存天然可读；分类可从
  AMBIGUOUS 升为 `DURABLE_DERIVABLE_SHARED`。
- 代价：**修改现有生产代码**（`connect()` 与 Dart 解析），属于你要求"proof 之后才允许"的范畴；
  且需要你确认"host/port 解析的 authority 归 Rust"这一所有权变化。

## 方向 2：把 SFTP 从本轮范围中显式排除（分阶段）

P1-D-2 只为 4 个通过的 provider 实施 B，SFTP **保持现状**（offline 时仍显示占位图），
并在文档/TODO 中登记为"待 cache authority 决策"。

- 优点：零架构扩张、不改既有生产代码。
- 代价：legacy B 不是全 provider 一致的终态；SFTP 用户 offline 仍看不到已有封面
  （与你"不得隐藏已存在本地内容"的意图部分冲突）。

**我不替你选**：方向 1 触碰既有生产代码与所有权边界，方向 2 是范围决策 —— 两者都属于你的决策面。

---

# 5. 本轮未做与未破坏

| 项 | 状态 |
|---|---|
| 生产代码修改 | **零**（全部只读 grep/sed/read） |
| local/custom 方案 A 的代码修改 | **按你要求暂缓**（保持 D2 为可审阅的原子阶段） |
| D2-1 ~ D2-6 测试 | 未写 |
| CA-1 ~ CA-4 RED 测试 | **未写**——因为 authority proof 未 PASS，按 §6 的前提条件不应开始 |
| `P1_RECOVERY_INTEGRITY_PASS` | **未被破坏**（本轮零文件改动；`cover_store.rs` diff 仍为 `+465/−9`、16 hunk、零函数删除） |
| temp artifacts | 无（本轮未生成任何临时文件） |
| branch / HEAD / staged / commit / push / merge / reset / stash | 全部无变化 |

# 6. P1-C residual（按你 §11，仅记录不改）

> **session-ready notification delivery is non-durable; failure delays reconciliation but does not
> corrupt cover state.**

本轮**没有**顺手处理 `RemoteSessionSuccessHub.emit` 的静默错误；未来若要加日志，作为独立的极小变更。

---

# 7. Remaining risks

1. 若你选择方向 1，`endpoint_for` 的提取必须**同时**改 `connect()`；任何"两边各写一份但看起来一样"
   都会重新落入 AMBIGUOUS。
2. 方向 1 若还要消除 Dart 的 `_parseHostPort`，会动到 5 个 provider 之外的 Dart 会话层，
   需确认不触碰 P1-C 的 lifecycle 边界（该层已冻结）。
3. 4 个"通过"的 provider 目前只是**证明可复用**，尚未写 CA-1~CA-4 的 RED 测试；
   正式实现 B 时**必须**先写 CA-1（用**当前 production writer/helper** 生成缓存再 sessionless 读取），
   且不得在测试里手工构造"预期 cache path"。
4. 仓库级既有红灯仍在：`cargo clippy`（`src/reader.rs:277`）、`flutter analyze`（121 issues）。

---

# 8. 【后续追加】SFTP ambiguity 的消除 → `P1_D2_CACHE_AUTHORITY_PROVEN`

> 本节是**追加**，不覆盖上文历史：证据链为
> **`AMBIGUOUS`（§1.1 原始发现）→ shared-helper extraction → `PROVEN`**。

## 8.1 方向选择与最小化边界（按审阅决定）

采用**方向 1 的最小化版本**：把 `SftpClient::connect()` 中既有的 endpoint 生成规则抽成
纯 Rust helper，让 **现有 writer 与新 sessionless reader 调用同一个 helper**。

**本轮未迁移 SFTP host/port parsing ownership**：保留 Dart `_parseHostPort(source)` 与
`sftpConnect(host, port, ...)`；Rust 新 reader 同样复用该 Dart parser 的输出
（logical `host` + `port`），Dart **不传** endpoint / cache key / cache path。最终链条：

```
same Dart _parseHostPort → same Rust endpoint_for → same raw_cache_path → same cover_cache key
```

## 8.2 抽取（extraction，非 redesign）

`src/source/sftp.rs` 新增纯函数（零副作用、零网络）：

```rust
pub fn endpoint_for(host: &str, port: u16) -> String {
    if port == 22 { host.to_string() } else { format!("{host}:{port}") }
}
```

`SftpClient::connect()` 原有的内联表达式被**原地替换**为 `endpoint_for(&host, port)`
（仅此一处改动，未改写整个 `connect`）。**刻意不做任何规范化**：不小写 host、
不去尾点、不改 IPv6 方括号、不做 IDNA、不 normalize localhost、不改默认端口规则、
不参与 username、不动 cache hash 与 namespace。

## 8.3 RED → GREEN（SFTP-CA-1 ~ CA-4）

先把 `endpoint_for` 作为**故意错误**的桩（总是拼 `host:port`）跑出真实 RED：

```
panicked at src/source/sftp.rs:394:
assertion `left == right` failed
  left: "nas.local:22"   right: "nas.local"
test result: FAILED. 3 passed; 2 failed
```

实现后：

```
test source::sftp::tests::sftp_ca_endpoint_freezes_historical_rules_without_normalization ... ok
test source::sftp::tests::sftp_ca_raw_cache_identity_is_unchanged_by_the_extraction ... ok
test source::sftp::tests::sftp_ca_parse_endpoint_must_not_become_the_cache_authority ... ok
test result: ok. 5 passed; 0 failed
```

| 用例 | 内容 | 结果 |
|---|---|---|
| **SFTP-CA-1** 默认端口 | `endpoint_for(host, 22) == host` **逐字**，含 `NAS.LOCAL`（不小写）、`nas.local.`（不去尾点）、`[::1]`（不改方括号）、`""`、`" nas "`（不 trim） | ✅ |
| **SFTP-CA-2** 自定义端口 | `endpoint_for(host, 2222) == "{host}:2222"` | ✅ |
| **SFTP-CA-3** 现有 Dart parser 可传的形状 | `[::1]`+`2222` → `"[::1]:2222"`；空 host + 2222 → `":2222"` | ✅ |
| **SFTP-CA-4** writer compatibility | 对 4 组 (host, port) 断言 helper 产出 == 抽取前 `connect()` 的历史字面量，且 `cache_hash` / `raw_cache_path` 两侧**完全相同**（测试**未手拼 cache path**，两侧都调用 production helper） | ✅ |

**没有出现** `P1_D2_SFTP_CACHE_COMPATIBILITY_REGRESSION`：raw cache identity 逐字未变。

## 8.4 额外发现（重要，请纳入后续登记）

`sftp.rs:322` 存在一个 `parse_endpoint(addr) -> (String, u16)`：

- 它**除自己的单测外没有任何生产调用者**（`grep -rn parse_endpoint src/` 只命中定义 + 测试）——**死代码**；
- 它的规范化与**现行唯一规则不同**：会 `trim()`、并**剥离 IPv6 方括号**，
  例如 `parse_endpoint("[::1]:2222") == ("::1", 2222)`；
- 因此若有人把 `parse_endpoint` 接成 cache authority，IPv6 输入会得到 `"::1:2222"`，
  而 writer 历史上写的是 `"[::1]:2222"` → **旧缓存静默 miss**。

已用 `sftp_ca_parse_endpoint_must_not_become_the_cache_authority` 把这条边界钉住。
**本轮未删除该死代码、未改其行为**（避免 unrelated cleanup 与行为变更）。
按 §11 登记为待办：**`SFTP source authority ownership consolidation`**
（含"死代码 `parse_endpoint` 是否应删除"与"host/port 解析是否应全部归 Rust"）。

## 8.5 最终判定

> ## **`P1_D2_CACHE_AUTHORITY_PROVEN`**

| Provider | 抽取前 | 抽取后 |
|---|---|---|
| WebDAV | `DURABLE_DERIVABLE_SHARED` | `DURABLE_DERIVABLE_SHARED` |
| **SFTP** | ❌ `AMBIGUOUS` | ✅ **`DURABLE_DERIVABLE_SHARED`** |
| Baidu | `DURABLE_DERIVABLE_SHARED` | `DURABLE_DERIVABLE_SHARED` |
| 115 app / web | `DURABLE_DERIVABLE_SHARED` | `DURABLE_DERIVABLE_SHARED` |
| Quark | `DURABLE_DERIVABLE_SHARED` | `DURABLE_DERIVABLE_SHARED` |

五个 provider（6 条 authority 路径）全部满足：writer 与新 reader 走**同一套**
authority derivation + canonicalization + cache-key path，且**旧缓存 identity 逐字可复现**。

## 8.6 本轮范围与未完成项（如实）

| 项 | 状态 |
|---|---|
| SFTP helper extraction + compatibility RED→GREEN | ✅ **完成**（`src/source/sftp.rs` +102/−5） |
| cache authority 判定升级 | ✅ **`P1_D2_CACHE_AUTHORITY_PROVEN`** |
| 五 provider 的 **CA-1~CA-4 正式测试套件** | ❌ **未写**（本轮只写了 SFTP 的 authority 兼容测试；其余 4 个 provider 的"用 production writer 写旧缓存再 sessionless 读回"套件尚未落地） |
| **local A**（收窄 `comic_cover.dart` 顶层 network guard） | ❌ **未实施** |
| **legacy B**（sessionless local-only lookup + 五 provider dispatch） | ❌ **未实施** |
| **D2-1 ~ D2-6** Flutter 测试 | ❌ **未写** |
| 新增 FRB API / codegen | ❌ 未做（B 尚未实施） |

因此 **P1-D-2 仍不是终态**：本轮交付的是它被门控的前置条件（authority proof + 判定），
而非 D2 本体。下一轮应从 **local A** 与 **legacy B** 开始，并先补五 provider CA-1~CA-4。

## 8.7 门禁（本轮）

| 命令 | exit | 结果 |
|---|---|---|
| `cargo test --lib source::sftp -- --test-threads=1` | 0 | **5 passed / 0 failed** |
| **`cargo test --locked -j 2 -- --test-threads=1`（全量）** | **0** | **18 个目标全部 ok；409 passed / 0 failed / 2 ignored**（P1-C 后为 406，+3 为 SFTP CA） |
| `git diff --check` | 0 | clean（曾报 `sftp.rs:470 new blank line at EOF`，已修） |
| 恢复完整性再核 | 0 | `cover_store.rs` 仍 `+465/−9`、**16 hunk**、**HEAD 函数被删数 = 0** → `P1_RECOVERY_INTEGRITY_PASS` 未被破坏 |
| 临时产物 | — | 已清理（`D:\Temp\p1d2_sftp_stub.py` 用完即删） |

## 8.8 Remaining risks

1. **CA-1~CA-4 目前只覆盖 SFTP 的 authority**；WebDAV/Baidu/115/Quark 的"旧缓存可 sessionless 读回"
   仍只是**代码层可复用性论证**，尚无测试。B 实施时必须先补。
2. `endpoint_for` 目前只有 `connect()` 一个调用者；在 B 落地前，其"共享"性由
   `connect()` 一处 + 测试冻结来保证。
3. SFTP 的 host/port 解析仍在 Dart，死代码 `parse_endpoint` 仍在 —— 见 §8.4 登记项。
4. 仓库级既有红灯仍在：`cargo clippy`（`src/reader.rs:277`）、`flutter analyze`（121 issues）。

> P1-C residual（仅记录）：**session-ready notification delivery is non-durable;
> failure delays reconciliation but does not corrupt cover state.**
