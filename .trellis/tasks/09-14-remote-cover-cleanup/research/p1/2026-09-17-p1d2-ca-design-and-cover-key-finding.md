# P1-D-2 实施（第 1 步）：CA 契约设计 + production cover-key 语义发现

> 工作目录 `D:/Projects/RCH-p1`｜分支 `p1-cover-completion`｜HEAD `1bf2e3743c026581ddaa9d444787fde05fd02f31`（未变）
> **本轮对生产代码零修改**（仅只读追踪）｜未 commit / push / merge / reset / stash
> 未进入 P1-E / P1-F / P2

## 状态（先说清楚）

> **`P1_D2_PASS` 未达成。** §1 的 CA 测试套件**尚未写出**，§2 及之后的 local-only API /
> local A / legacy B / D2-1~D2-6 均**未开始**。本轮交付的是 §1 所必需的
> **production 读写表达式与 CA 设计**，以及一个**会改变 §2 API 形态与 §14 判定**的新发现。

**本轮没有声称 PASS，也没有用任何 STOP code 掩盖预算限制**：这不是你冻结的某个 STOP 条件，
而是本轮可用预算在完成精确追踪后耗尽。我选择停下并把设计交给你，而不是写一个我没跑通的测试套件。

---

# 1. production 的 cover-cache 读写表达式（逐 provider 精确）

`sftp_cover` / `webdav_cover` / `baidu_cover` / `cloud115_cover*` / `quark_cover` 的结构一致：

**读**（`source.rs:1667-1681`，webdav 为例）：

```rust
let cache_lookup_path =
    webdav::raw_cache_path(&origin, &path).or_else(|| Some(std::path::PathBuf::from(&path)));
if let Some(ref lookup) = cache_lookup_path {
    if let Some((rgba, w, h)) =
        cache::cover_cache_read(&lookup.to_string_lossy(), page, width, height, crop_tuple)
    { return Ok(PageImage { .. }); }          // ← HIT：直接返回，不触网
}
// ↓ MISS：才进入 spawn_blocking（governor permit → raw 本地解码 或 HTTP Range）
```

**写**（`source.rs:1705-1712`）：**同一个表达式** → `cache::cover_cache_write(&wp.to_string_lossy(), page, width, height, crop, &rgba)`。

`cache::cover_cache_key(path, page, w, h, crop)`（`cache.rs:99-108`）对**传入的 path 字符串**
做 `DefaultHasher`，文件名形如 `{hash:x}_{page}_{w}_{h}{crop}.cover`，位于 `CacheDir::Cover`。

## 1.1 六个 authority 路径的 authority 输入

| # | Provider | authority 表达式 | sessionless 构造方式（均**不触网**） | raw path helper |
|---|---|---|---|---|
| 1 | WebDAV | `format!("{}://{}[:port]", scheme, host)`（`webdav.rs:154-159`） | `WebDavClient::new(base, user, pass)` → `.origin()` | `webdav::raw_cache_path(origin, path)` |
| 2 | SFTP | `endpoint_for(host, port)`（P1-D-2 已抽取） | **纯函数**，零依赖 | `sftp::raw_cache_path(endpoint, path)` |
| 3 | Baidu | `format!("baidu:{}:{}", app_key, root)`（`baidu.rs:181-183`） | `BaiduClient::new(app_key, secret, refresh_token, root)` | `baidu::raw_cache_path(origin, path)` |
| 4 | 115 app | `format!("115:{}:{}", app_id, root_id)`（`cloud115.rs:309-311`） | `Cloud115Client::new(app_id, refresh_token, root_id)` | `cloud115::raw_cache_path(origin, path)` |
| 5 | 115 web | `format!("115web:{}", root)`（`cloud115.rs:1199-1202`） | `Cloud115WebClient::new(cookie, root)` | `cloud115::web_raw_cache_path(origin, path)` |
| 6 | Quark | `format!("quark:{}", root)`（`quark.rs:258-261`） | `QuarkClient::new(cookie, root)` | `quark::raw_cache_path(origin, path)` |

（`root` 空值规范化：quark/115web `"0"`、baidu `"/"`、115app `"0"` —— 均已在前轮 README 级核对中确认。）

---

# 2. ⚠️ 新发现：legacy cover cache key **不是纯函数**，它依赖 raw 文件当前是否存在

`raw_cache_path(origin, path)` **不是路径推导**，而是**探测**（`sftp.rs:306-315`、其余同理）：

```rust
match std::fs::metadata(&file_path) {
    Ok(meta) if meta.len() > 0 => Some(file_path),   // 只有「存在且非空」才返回 Some
    _ => None,
}
```

而 cover API 对探测结果做了 `.or_else(|| Some(PathBuf::from(&path)))` **回退到逻辑路径**。
于是同一个 (origin, path) 会因 raw 文件的有无而映射到**两个不同的 cover 缓存键**：

| 读/写时的状态 | 传入 `cover_cache_key` 的 path | 后果 |
|---|---|---|
| raw 缓存文件**存在且非空** | raw cache 文件路径字符串 | 键 = hash(raw path) |
| raw 缓存文件**不存在**（Range 流式取回、或缓存已被清理） | **逻辑路径**字符串 | 键 = hash(logical path) |

**两个真实后果**：

1. **可达性依赖状态**：若某封面是在 raw 文件存在时写入的（键 = raw path），之后 raw 文件被
   缓存清理删除，则同一 (origin, path) 在**读**时会回退到逻辑路径 → 键不匹配 →
   **旧封面缓存变成不可达**（即使 `.cover` 文件仍在磁盘上）。
   这是**既有行为**，不是本任务引入；D2 的 reader 必须**照抄同一表达式**才不会额外制造不一致。
2. **对 §2 API 形态的硬约束**：local-only reader **不能**接受 Dart 传来的 raw path 或 cache key，
   也**不能**只做 `raw_cache_path()` 而省略 `.or_else(logical)` ——
   否则它会在上述状态下与 writer 得出不同键。reader 必须**封装完整表达式**：
   `authority → raw_cache_path(探测) → or_else(logical path) → cover_cache_read`。

因此 CA 套件必须覆盖**两种状态**（raw 存在 / raw 不存在），否则无法证明"磁盘上已存在的缓存可被找到"。

---

# 3. CA 契约设计（§1 的执行方案，待实施）

落点：`src/source/` 下新增 `#[cfg(test)] mod cache_authority;`（单元测试模块，
避免依赖 crate 外部可见性；`src/source/mod.rs` 目前没有测试模块）。

| 用例 | 内容 |
|---|---|
| **CA-1** existing-cache compatibility | 对 6 个 authority 路径 × **两种 raw 状态**：用**生产 helper** 推 authority → 用**生产表达式**得 key path → `cover_cache_write` 写 → **不创建任何 session** → 用拟议的 sessionless derivation 读回 → **同一 `.cover` 文件命中**。测试**不手拼** 最终 cache path（全程调用 `raw_cache_path` / `cover_cache_read`）。 |
| **CA-2** no session | 每个 provider 单独断言：`source.rs` 的 session 注册表（`webdav`/`sftp`/`baidu`/`cloud115`/`quark` 的 `SESSION` map）**为空**、`remote_scan_epoch` **无行** 的情况下，disk hit **仍然成功**。 |
| **CA-3** miss side-effect-free | miss 时断言：返回 `None`；session 注册表条目数不变；`remote_cover_job` 行数不变；未创建任何新文件（对比目录快照）；无 wake（`cover_workers()` 为空）。 |
| **CA-4** historical canonicalization | 逐 provider 冻结 authority 字面量：WebDAV `scheme://host[:port]`；SFTP 默认/自定义端口 + `[::1]` 形式；Baidu `baidu:{app_key}:{root}`（含 `root` 空 → `"/"`）；115 app `115:{app_id}:{root_id}`（空 → `"0"`）；115 web `115web:{root}`；Quark `quark:{root}`。**不定义任何新 normalization**。 |

覆盖矩阵（6 行，115 拆两行，**不得只测一条**）：

| authority 路径 | CA-1 | CA-2 | CA-3 | CA-4 |
|---|---|---|---|---|
| WebDAV | ☐ | ☐ | ☐ | ☐ |
| SFTP | ☐（authority 部分已由 P1-D-2 的 5 个测试覆盖） | ☐ | ☐ | ☐ |
| Baidu | ☐ | ☐ | ☐ | ☐ |
| 115 app | ☐ | ☐ | ☐ | ☐ |
| 115 web | ☐ | ☐ | ☐ | ☐ |
| Quark | ☐ | ☐ | ☐ | ☐ |

（☐ = 未实施）

---

# 4. 后续步骤（§2 起，未开始）

1. **§2/§3 local-only API**：单一入口，Dart 只传 logical source fields
   （source 类型 + 已有业务字段；SFTP 传 `host`+`port`，**不得**传 endpoint/key/path）。
   Rust 内部按 provider dispatch，复用上表 6 条 authority 构造 + **完整表达式**（含 `or_else` 回退）。
   硬契约：no session / no connect / no refresh / no provider / no job / no wake / no scan /
   no retry mutation / no durable mutation。
2. **R1~R5**：五 provider 旧缓存 → sessionless hit；无 session → hit；miss 无副作用；
   SFTP reader 经共享 `endpoint_for`；**reader 永不 fallback 到 provider**（网络 fallback 属上层）。
3. **§5 local A**：收窄 `comic_cover.dart` 顶层 guard，让 `bookCover`（本地、无 session、零网络）
   先执行；不重构 widget。
4. **§6 legacy B**：统一流 `local-only lookup → HIT 显示并停止 / MISS+off 占位 / MISS+on 走原 session+fetch`；
   **disk hit 必须早于 `_guardRemoteCoverIo` / session 获取**。
5. **§7**：unified remote 保持原样（`remote_cover_read` 不触网、disk-first），不并入新 abstraction。
6. **§8/§9/§10**：D2-1~D2-6 + `D2-ORDER`（用注入设施记录调用序列
   `["local-cache"]` / `["local-cache","session","provider"]`）+ 115 双模式分别覆盖。
7. **§11**：`parse_endpoint` 继续冻结（不删、不接、不改、不顺带格式化），
   回归测试继续钉住；登记 `SFTP source authority ownership consolidation`。
8. **§12**：新增 FRB API 后跑 codegen 并审核 generated diff（禁止手改）。
9. **§13**：Rust（SFTP 5 tests + CA suite + local-only suite + cover-cache suite，必要时
   `cargo test --locked -j 2`）／Flutter（D2-1~6 + ORDER + 既有 disk-first + legacy 回归 + analyze）／
   Repo（`git diff --check`、generated diff audit、`P1_RECOVERY_INTEGRITY_PASS`、临时产物清理）。

---

# 5. 本轮门禁与完整性

| 项 | 结果 |
|---|---|
| 生产代码修改 | **零**（全部只读 grep/sed/read） |
| `P1_RECOVERY_INTEGRITY_PASS` | 未受影响（本轮零文件改动） |
| `cargo test` | 本轮未跑（无代码改动；上次为 409 passed / 0 failed / 18 targets） |
| `git diff --check` | clean |
| branch / HEAD / staged / commit / push / merge / reset / stash | 全部无变化 |
| 临时产物 | 无 |

## Remaining risks

1. **§2 API 必须封装 `or_else(logical path)` 回退**，否则在 raw 文件缺失状态下与 writer 键不一致
   （§2 的发现）。若把该回退留在 Dart 或省略，P1-D-2 会在真实缓存上静默失效。
2. 上述"raw 删除后旧封面不可达"是**既有**局限；D2 不修复它（修复需要新的键策略 = 架构决策）。
   若你要求修复，应作为独立事项，因为它会**改变已有 cache identity**（违反本任务第 10 条 PASS 条件）。
3. Baidu/115/Quark 的 `new()` 不触网已由前轮核对，但**尚未有测试**断言"构造 authority 不产生任何
   网络/注册表副作用"；CA-2/CA-3 应显式覆盖。
4. 仓库级既有红灯仍在：`cargo clippy`（`src/reader.rs:277`）、`flutter analyze`（121 issues）。
5. P1-C residual（仅记录）：**session-ready notification delivery is non-durable; failure delays
   reconciliation but does not corrupt cover state.**
