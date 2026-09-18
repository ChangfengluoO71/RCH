# P1-D-2 实施：`P1_D2_RAW_KEY_RECONSTRUCTION_AMBIGUOUS`

> 工作目录 `D:/Projects/RCH-p1`｜分支 `p1-cover-completion`｜HEAD `1bf2e3743c026581ddaa9d444787fde05fd02f31`（未变）
> **本轮对生产代码零修改**｜未 commit / push / merge / reset / stash｜未进入 P1-E / P1-F / P2

## 判定

> ## **`P1_D2_RAW_KEY_RECONSTRUCTION_AMBIGUOUS`**
>
> 受影响 authority 路径：**115-web** 与 **Quark**（2 / 6）。
> 其余 4 条（WebDAV / SFTP / Baidu / 115-app）**无歧义，可精确重建**。

这是你 §二 明确指定的 STOP 条件，且有 `file:line` 证据；**不是**预算/工作量借口。
按 §二「不要猜」的要求，我在此 STOP，未继续实现 reader，也未改任何生产代码。

---

# 1. 为什么需要 candidate path（你的 §二 前提）

`raw_cache_path(authority, path)` 同时承担两件事：**计算** raw 文件应在哪里、
以及用 `fs::metadata` **探测**它当前是否存在且非空（`sftp.rs:306-315` 等）。
而 cover cache key 是**对传入 path 字符串做 hash**（`cache.rs:99-108`），
且 cover API 在探测为 `None` 时**回退到逻辑路径**（`source.rs:1668-1669` / `:1706-1707`）。

所以 D2 reader 需要在 raw 文件**已被删除**时，知道"它原本的确定性 raw cache path 是什么"——
这正是 `raw_cache_candidate_path(authority, path)` 要提供的能力。

# 2. 六条路径的 writer 文件路径构造（决定能否重建）

## 2.1 可精确重建（Family 1，4 条）

统一形态：

```
name  = <逻辑 path>.rsplit('/').next().unwrap_or("file.cbz")
hash  = DefaultHasher(format!("{authority}{key}")) → {:016x}
path  = CacheDir::Raw/<hash>/<name>
probe = metadata.len() > 0 → Some(path)
```

| 路径 | writer 的 name 来源 | 证据 |
|---|---|---|
| WebDAV | `path.rsplit('/').next().unwrap_or("file.cbz")` | 读写同式：`webdav.rs:129`（读）/ 写入同源 |
| SFTP | 同上 | `sftp.rs:306-315` |
| Baidu | `path.rsplit('/').next().unwrap_or("file.cbz")` | `baidu.rs:500`（writer）、`:633-646`（reader） |
| 115 app | `path.rsplit('/').next().unwrap_or("file.cbz")` | `cloud115.rs:607`（writer）、`:721-737`（reader） |

这四条：candidate 可从 `(authority, logical key)` **纯函数**重建，且 name 与 writer 逐字一致。

## 2.2 **不可精确重建（Family 2，2 条）**

| 路径 | writer 的 name 来源 | 证据 |
|---|---|---|
| **115 web** | **`self.downurl(pick_code)?` → `info.name`**（远端真实文件名），仅当其为空才回退 `"file.cbz"` | `cloud115.rs:1753-1762`，随后 `let file_path = dir.join(&name);`（`:1774`） |
| **Quark** | **`self.downlink(fid)?` → `info.name`**（远端真实文件名），仅当其为空才回退 `"file.cbz"` | `quark.rs:576-585`，随后 `let file_path = dir.join(&name);`（`:597`） |

两者的 raw 路径为：

```
CacheDir::Raw/<hash(authority+key)>/<provider 在下载时返回的远端文件名>
```

而该文件名：

1. **是网络响应派生**（`downurl` / `downlink` 的返回），
2. **没有持久化在任何 durable 表**（`book_sources` 无此字段；`remote_cover_*` 也不记录 raw 文件名），
3. 因此**无法**由 `(authority, logical key)` 重建。

这也解释了为什么它们的 `raw_cache_path` 必须用 **`std::fs::read_dir` 扫描目录**取
"第一个非空条目"（`cloud115.rs:1895-1902`、`quark.rs:722-728`）——
**读取者本来就无法预测这个文件名**。

## 2.3 精确的失败边界（不是全部失败）

按你 §五 的四种 raw 状态转换，逐族判定：

| 変換 | Family 1（WebDAV/SFTP/Baidu/115app） | Family 2（115-web/Quark） |
|---|---|---|
| raw present → present（CA-STATE-1） | ✅ 可重建 | ✅ 可行（probe 返回扫描路径，与写入时同一路径） |
| raw absent → absent（CA-STATE-2） | ✅ | ✅（两侧都用逻辑路径） |
| **raw present → absent（CA-STATE-3）** | ✅ | ❌ **无法重建**：写入时的键基于"当时的远端文件名"，文件删除后该名字不可获得 |
| raw absent → present（CA-STATE-4） | ✅ | ✅（alternate = 逻辑路径，完全可推导） |

所以失败**仅限 CA-STATE-3**，且仅限 115-web / Quark：

> 一个 cover 在 raw 文件存在时以 raw-key 写入；随后 raw 文件被删除（缓存清理）；
> 之后**无法**重建当时的 raw path 字符串 ⇒ 无法计算 alternate key ⇒
> 即使 `.cover` 文件仍留在磁盘上，也**读不回来**。

这直接影响你的 PASS 条件 **#2「raw present→absent 后旧 `.cover` 仍可读」**：
对 115-web / Quark 无法满足。

# 3. 这条 STOP 对应你 §十七 的哪一项

- ✅ `P1_D2_RAW_KEY_RECONSTRUCTION_AMBIGUOUS`（原文列出）
- ✅ "historical alternate key 无法由 production algorithm 精确重建"（原文列出）

且**不涉及**任何被禁止的补救：我没有改 writer、没有迁移 `.cover`、没有新建 namespace、
没有删除旧 key、没有扫目录找近似文件、没有重新联网生成 cover。

---

# 4. 最小的两个可选方向（**不擅自选择**）

## 方向 α — 接受 per-provider 不对称，把 PASS 条件 #2 限定为可按保存（推荐先评估）

dual-key reader **照常为全部 6 条路径实现**；CA-STATE-3 对 Family 1 的四条**必须 PASS**，
对 115-web / Quark 作为**有证据的既有局限**记录（并写进 remaining risks + TODO）。
连带效果：CA-STATE-1/2/4 对六条全部 PASS，PASS 条件 #2 需按 provider 拆开表述。

- 优点：零架构变更、零 writer 改动、零 migration；不动 cache identity（你的第 4 条 PASS 条件仍成立）。
- 代价：115-web / Quark 上"raw 被清理后旧封面不可达"保留为用户可见的既有缺陷；
  需要你同意把 PASS 条件 #2 限定表述。

## 方向 β — 让 writer 持久化远端文件名（未来缓存可重建，旧缓存仍不可）

在下载写入 raw 时，把该远端文件名（或完整 raw 相对路径）记录到 durable 表（新增字段/表），
使**将来**的 raw-key cover 在 raw 删除后仍可重建。

- 优点：从根上消除 Family 2 的双 identity 可达性问题（面向未来）。
- 代价：① **需要 schema 变更**（属架构决策，我不自建）；
  ② **无法修复磁盘上已存在的旧缓存** —— 那些封面从未记录过文件名，
  因此对"旧缓存兼容"目标无效；
  ③ 触及 writer 与其持久化路径，超出本轮"writer 不动"的冻结边界。

**我不替你选。** 建议：先按方向 α 把 dual-key reader 与 CA/R 套件落地（可按保存地满足
PASS 条件 1、3~12），再由你决定是否为 Family 2 单独立项（β 或 `Cover cache identity unification`）。

---

# 5. 本轮范围与状态（如实）

| 项 | 状态 |
|---|---|
| §二 candidate path 可行性证明 | ✅ **完成**：4 条可重建，**2 条不可**（本轮结论） |
| `raw_cache_candidate_path` 抽取 | ❌ 未实施（被本条 STOP 阻断：无法为 6 条路径给出统一定义） |
| CA-STATE-1~4 套件 | ❌ 未写 |
| local-only Rust API / R1~R7 | ❌ 未实施 |
| local A / legacy B / D2-1~6 / D2-ORDER | ❌ 未实施 |
| FRB codegen | ❌ 未做 |
| **生产代码修改** | **零**（本轮仅只读 grep/read） |
| `P1_D2_PASS` | **未达成** |
| `P1_D2_CACHE_AUTHORITY_PROVEN` | 仍然成立（authority 侧未变；本条是 **raw-key 重建**侧的新事实） |
| `P1_RECOVERY_INTEGRITY_PASS` | 未受影响（零文件改动） |

## 门禁

| 项 | 结果 |
|---|---|
| `git diff --check` | clean |
| branch / HEAD / staged / commit / push / merge / reset / stash | 全部无变化 |
| 临时产物 | 无 |
| `cargo test` | 本轮未跑（无代码改动；上次 409 passed / 0 failed / 18 targets） |

## Remaining risks

1. **115-web / Quark 的 CA-STATE-3 不可达**（本轮结论）；若你选方向 α，需同步修订 PASS 条件 #2 的表述。
2. `read_dir` 扫描式 `raw_cache_path` 与 Family-1 的确定性定位是**两套查找语义**；
   reader 必须对这两族分别处理（Family 2 的 Candidate A 是"扫描结果"，不是纯计算）。
3. Family 1 的 4 条路径的 candidate 抽取虽已判定可行，但**尚未实现、尚未测试**。
4. 仓库级既有红灯仍在：`cargo clippy`（`src/reader.rs:277`）、`flutter analyze`（121 issues）。
5. P1-C residual（仅记录）：**session-ready notification delivery is non-durable; failure delays
   reconciliation but does not corrupt cover state.**
