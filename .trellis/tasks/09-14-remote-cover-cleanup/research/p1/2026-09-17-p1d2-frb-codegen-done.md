# P1-D-2 接线轮报告：FRB codegen 完成；Flutter 接线未开始

> `D:/Projects/RCH-p1`｜分支 `p1-cover-completion`｜HEAD `1bf2e3743c026581ddaa9d444787fde05fd02f31`（未变）
> 未 commit / push / merge / reset / stash｜未进入 P1-E / P1-F / P2

## 状态

> **`P1_D2_PASS` 未达成。**

本轮唯一完成项：**FRB codegen + generated diff 审核 + fresh Rust 门禁**。
**未开始**：local A、legacy B、D2-1~D2-6、D2-ORDER、Flutter analyze / fresh Flutter gate。

**为什么停在接线前**：接线（删除 `comic_cover.dart` 顶层 network guard、在 legacy 分支前插入
local-only 查找）是对**生产 Dart 行为**的实质改动，按本会话一贯纪律它必须同时带来
D2-1~D2-6 + D2-ORDER 的 RED→GREEN 证据。本轮剩余预算不足以在**跑通并迭代**这些用例的前提下
完成该改动；按 §14「不要为了预算回到 reconnaissance」与"不留下未验证改动"两条约束，
我选择只交付已完整验证的 codegen 增量，而**不**在无测试覆盖的情况下改动 widget 行为。

---

# 1. FRB codegen（完成）

`cd app && flutter_rust_bridge_codegen generate` → **EXIT=0**（`Done!`），未手改任何 generated file。

生成结果：

| 位置 | 内容 |
|---|---|
| `lib/src/rust/api/source.dart:367` | `Future<PageImage?> readLegacyCoverLocal({required LegacyCoverLocalLookupDto lookup})` |
| `lib/src/rust/api/source.dart:760` | `class LegacyCoverLocalLookupDto`（+ `:785` const ctor / `:820` `==`/`hashCode`） |
| `lib/src/rust/frb_generated.dart:656` | binding 注册 |
| `rust/src/frb_generated.rs` | Rust 侧 wire code |

## generated diff 审核

```
app/lib/src/rust/api/remote_cover.dart   |   2 +-   （P1-A 的 cause 参数）
app/lib/src/rust/api/remote_scan.dart    |  82 +-   （P1-C notifySourceSessionReady，前一轮 codegen）
app/lib/src/rust/api/source.dart         | 109 +-   （本轮 readLegacyCoverLocal + DTO）
app/lib/src/rust/frb_generated.dart      | 412 +-
app/lib/src/rust/frb_generated.io.dart   |  46 +
app/lib/src/rust/frb_generated.web.dart  |  46 +
app/rust/src/frb_generated.rs            | 411 +-
```

| 检查项（§1） | 结果 |
|---|---|
| 只新增预期 API / DTO binding | ✅ `readLegacyCoverLocal` + `LegacyCoverLocalLookupDto` |
| 没有意外删除现有 API | ✅ `source.dart` 的删除行**仅为注释重排**（"not marked as pub" 的忽略清单换序 + `webdav_cover` 文档注释因插入位置下移），无任何现有 API 签名消失 |
| 没有无关类型大面积漂移 | ✅ 改动集中在两个新 API 的 binding |
| 不手改 generated files | ✅ 全部由 codegen 产出 |
| **DTO 不暴露 cache path/hash/endpoint/origin** | ✅ 字段仅：`kind, url, host, port, appKey, appId, rootId, root, logicalPath, page, width, height, crop` |

# 2. fresh Rust gate（完成）

| 命令 | exit | 结果 |
|---|---|---|
| **`cargo test --locked -j 2 -- --test-threads=1`** | **0** | **18 个目标全部 ok；415 passed / 0 failed / 2 ignored** |

（codegen 改动 `frb_generated.rs` 后重新执行，**未引用上一轮结果**。）

| 项 | 结果 |
|---|---|
| `git diff --check` | clean |
| staged | 空 |
| `P1_RECOVERY_INTEGRITY_PASS` | 未受影响（`cover_store.rs` 仍 `+465/−9`） |
| 临时产物 | 无 |

# 3. 未开始：Flutter 接线（已冻结、未实施）

以下为**已冻结待实施**的接线方案，本轮**未改一行 Dart**：

## 3.1 local A（`bookCover` 不受网络开关阻止）

删除 `comic_cover.dart` 顶层 `_maybeLoad` 中的
`if (widget.remoteAssetId == null && _remoteCoverNetworkPaused) return;`。
该路径已证明为本地文件 + Rust decode + 无 session + 不触网，被该 guard 错误短路。
**不重构 widget，仅删除这一条顶层 guard。**

## 3.2 legacy B（顺序不变量）

在 `_load` 的 legacy 分支链（`widget.source.isWebDav` 之前的插入点，`comic_cover.dart:585/587`）
**之前**插入：

```
readLegacyCoverLocal(...)   ← 纯本地；不建 session、不触 provider、不受网络开关影响
 ├─ HIT  → return image      （0 session / 0 provider）
 └─ MISS → 继续下方既有流程：
            _guardRemoteCoverIo(sessionFor)   ← offline 时抛 _RemoteCoverFetchDisabled
                                               → 既有占位图（0 session / 0 provider）
            → 既有 legacy cover API（provider fetch）
```

**不变量**：local lookup 严格早于 `_guardRemoteCoverIo` 与 session getter ✓（由插入位置保证）。

## 3.3 Dart 参数（§4）

`_legacyLocalKind(source)`：`webdav / sftp / baidu / (clientId 非空 ? "115" : "115web") / quark`。
SFTP 复用**同一个** `_parseHostPort`（在 `sftp_session.dart` 加一个公开薄包装，不复制解析逻辑），
只传 logical `host`/`port`；Rust 内部继续用共享 `endpoint_for()`。
Dart **不构造也不保存** origin / endpoint / raw path / `.cover` path / hash。

## 3.4 §3：miss 不是错误

`None` 只表示"当前没有可确定读取的本地 cover"（含 115-web/Quark 的已冻结不可恢复历史状态）。
offline 时表现为**普通 placeholder**：不 throw provider error、不改 durable state、
不写 failed job、不显示 cache corrupted、不自动建 session。Dart **不为 Family 2 做任何特殊处理**
（不做 provider filename lookup、不做 background repair、不扫 cover 目录、不做 orphan 检测）。

## 3.5 §9：CA-3 不再扩展 instrumentation

最终报告如实采用该措辞：

> provider fallback is directly tested; job/wake absence is guaranteed by the local-only call graph
> and lack of those dependencies, not by production counters.

# 4. `P1_D2_PASS` checklist（逐条）

| # | 项 | 状态 |
|---|---|---|
| 1 | 六 authority Rust CA GREEN | ✅（6/6 + SFTP 5/5） |
| 2 | Family 1 四状态兼容 | ✅ |
| 3 | Family 2 unrecoverable boundary 保持 | ✅ |
| 4 | writer/cache namespace 不变 | ✅ |
| 5 | local-only API 0 network fallback | ✅（R5） |
| 6 | offline legacy recoverable hit visible | ❌ **未做** |
| 7 | offline miss session=0/provider=0 | ❌ 未做 |
| 8 | online hit session=0/provider=0 | ❌ 未做 |
| 9 | online miss 走原 session/provider path | ❌ 未做 |
| 10 | offline local/custom visible | ❌ 未做 |
| 11 | unified remote no regression | ❌ 未验证 |
| 12 | D2-ORDER 正确 | ❌ 未做 |
| 13 | 115 app/web kind mapping 正确 | ❌ 未做（映射规则已定，见 §3.3） |
| 14 | SFTP reader/writer 共用 `endpoint_for` | ✅ |
| 15 | FRB generated diff 正常 | ✅ **本轮完成** |
| 16 | fresh Rust/Flutter gates 无本轮新增失败 | Rust ✅ / **Flutter 未跑** |

**未做原因**（不属 §14 的 STOP 事实）：测试数量 + fixture 注入 + codegen 耗时 + 单轮工作较长 ——
即 §14 明确要求"继续完成"的类别。我如实标注为**未完成**而非 STOP，也不把它写成 PASS。

# 5. 本轮改动文件

```
M app/lib/src/rust/api/source.dart        （codegen）
M app/lib/src/rust/frb_generated.dart     （codegen）
M app/lib/src/rust/frb_generated.io.dart  （codegen）
M app/lib/src/rust/frb_generated.web.dart （codegen）
M app/lib/src/rust/api/remote_cover.dart  （codegen，P1-A 参数）
M app/lib/src/rust/api/remote_scan.dart   （codegen，P1-C 新 API）
M app/rust/src/frb_generated.rs           （codegen）
（本轮无 Rust 源码改动、无 Dart 业务代码改动）
```

原有 dirty（P1-A/B/C/D + Rust local-only 实现）全部原样保留；无 reset/stash/checkout 覆盖。

# 6. Remaining risks

1. **PASS 条件 6~13 无任何证据**：offline legacy 可见性**尚未被证明**；widget 行为未改。
2. `readLegacyCoverLocal` 已有 Dart 绑定但**无调用方**。
3. SFTP 公开包装 `sftpHostPortOf` 尚未添加（`_parseHostPort` 目前是 `sftp_session.dart` 私有函数）；
   接线时必须用薄包装复用**同一个** parser，不得复制。
4. Family 2 的 Dart 侧"不特殊处理"是**约定**，尚未有测试钉住（既无 provider filename lookup 也无 orphan 检测）。
5. 仓库级既有红灯仍在：`cargo clippy`（`src/reader.rs:277`）、`flutter analyze`（121 issues baseline）。
