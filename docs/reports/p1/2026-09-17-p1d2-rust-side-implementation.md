# P1-D-2 实施报告（Rust 侧完成；Flutter 侧未开始）

> 工作目录 `D:/Projects/RCH-p1`｜分支 `p1-cover-completion`｜HEAD `1bf2e3743c026581ddaa9d444787fde05fd02f31`（未变）
> 未 commit / push / merge / reset / stash｜未进入 P1-E / P1-F / P2

## 状态

> **`P1_D2_PASS` 未达成。** 本轮完成 **Rust 侧**：Family 1 candidate helper、Family 2 目录 helper、
> local-only local-only lookup API、CA 套件 6/6 GREEN、全量门禁 EXIT=0。
> **未完成**：FRB codegen、local A、legacy B、D2-1~D2-6、D2-ORDER、Flutter analyze —— 即
> PASS 条件中的 8/9/10/11/12（offline 可见性、online 不拿 session 等 Flutter 行为）**尚无证据**。

## 1. SFTP ambiguity 的消除（沿用上一轮）

`endpoint_for(host, port)` 为**纯函数**（零副作用、零网络），writer（`SftpClient::connect`）与本轮新增的
reader（`legacy_cover_authority` 的 `"sftp"` 分支）**调用同一个 helper**；reader 内**没有**任何
`if port == 22` 复制。`parse_endpoint` 继续冻结（未接入、未删除、未修改），其"不得成为 authority"的
回归测试仍在。

## 2. 五 provider CA matrix（六条 authority 路径）

| authority 路径 | authority 分类 | CA-1 old-cache | CA-2 no-session | CA-3 miss 无副作用 | CA-4 字面冻结 | present→absent |
|---|---|---|---|---|---|---|
| WebDAV | DURABLE_DERIVABLE_SHARED | ✅ | ✅ | ✅ | ✅ `https://dav.example.com` | ✅ **HIT** |
| SFTP(22 / 2222) | DURABLE_DERIVABLE_SHARED | ✅ | ✅ | ✅ | ✅ `nas.local` / `nas.local:2222` | ✅ **HIT** |
| Baidu | DURABLE_DERIVABLE_SHARED | ✅ | ✅ | ✅ | ✅ `baidu:appkey:/` | ✅ **HIT** |
| 115-app | DURABLE_DERIVABLE_SHARED | ✅ | ✅ | ✅ | ✅ `115:appid:0` | ✅ **HIT** |
| 115-web | DURABLE_DERIVABLE_SHARED（目录） | ✅ | ✅ | ✅ | ✅ `115web:0` | ⚠️ **UNRECOVERABLE**（见 §3） |
| Quark | DURABLE_DERIVABLE_SHARED（目录） | ✅ | ✅ | ✅ | ✅ `quark:0` | ⚠️ **UNRECOVERABLE**（见 §3） |

`absent→present`：六条路径**全部 HIT**（在同一个 table-driven 矩阵用例中验证）。

测试：`src/api/source.rs` 的 `mod d2_cache_authority_tests`（6 个用例）
- `d2_ca4_authority_literals_are_frozen`（CA-4，六条字面规则）
- `d2_ca1_ca2_sessionless_lookup_reads_existing_caches`（CA-1/CA-2，**零 session** 读回旧缓存）
- `d2_ca_state_transition_matrix`（四种 raw 状态转换 × 六条路径，table-driven）
- `d2_ca_state3_unrecoverable_for_family2_is_a_clean_miss`（见 §3）
- `d2_ca3_miss_is_side_effect_free`（miss 不产生任何缓存文件）
- `d2_r5_local_lookup_never_falls_back_to_provider`（R5）

## 3. 冻结的不可恢复边界（按 §6 显式保留，**不写成 PASS**）

> `115-web` / `Quark`：raw 存在时以 raw-key 写入 cover → 删除 raw → 保留 `.cover` →
> sessionless lookup = **clean MISS**。

测试名与注释显式写明原因：**`remote filename was not durably persisted; historical raw key
cannot be reconstructed`**。用例先断言"那个 `.cover` 确实在磁盘上"（证明是**不可达**而非"没写过"），
再断言 reader 返回 `None`。**这是冻结已证明的局限，不是失败测试。**

**禁止猜测补齐**：reader 在 raw 缺失时 `alternate = None` → 不扫 `.cover` 目录、不枚举 hash、
不猜远端 filename、不调 provider、不建 session、不下载 raw（R5 用例钉住）。

## 4. 实现方式（按 §4 的真实能力分族，不强行统一抽象）

| | 新增 helper | 语义 |
|---|---|---|
| **Family 1**（webdav / sftp / baidu / 115-app） | `raw_cache_candidate_path(authority, path) -> PathBuf` | 从既有 `raw_cache_path` **原样抽取**的**纯计算**部分（hash / 目录 / 文件名逐字保持），**零 filesystem probe** |
| **Family 2**（115-web / Quark） | `web_raw_cache_dir` / `raw_cache_dir` | 只给**确定性目录**；**不提供文件级 candidate**（文件名不可推导） |

四个 Family-1 的 `raw_cache_path` 均已改为 `ensure()? → candidate → metadata/len 判定`，
**返回值与副作用逐字保持**（probe 语义未变）。Family-2 的 `*_raw_cache_path` 改为
`ensure()? → dir → read_dir`，同样保持原语义。

## 5. local-only Rust API

`src/api/source.rs`：

```rust
pub struct LegacyCoverLocalLookupDto { kind, url, host, port, app_key, app_id, root_id, root,
                                       logical_path, page, width, height, crop }
pub fn read_legacy_cover_local(lookup: LegacyCoverLocalLookupDto) -> Option<PageImage>
fn legacy_cover_authority(lookup: &LegacyCoverLocalLookupDto) -> Option<String>  // 私有
```

- **Dart 不传**：origin / endpoint / raw path / cache key / hash。只传 logical source fields
  （`kind` 与仓库既有 `open_cached_remote_book(kind, ..)` 风格一致；SFTP 传 logical host/port）。
- authority 复用各 provider **现有的不触网构造函数**：`WebDavClient::new`、`endpoint_for`、
  `BaiduClient::new`、`Cloud115Client::new`、`Cloud115WebClient::new`、`QuarkClient::new`。
- 双 key 逻辑：current = production 表达式（probe→raw path，None→logical）→ miss →
  **可恢复时** alternate（Family 1 = 确定性 candidate；Family 2 = raw 存在时用 logical，
  不存在时 **None**）→ 仍 miss 则 **clean return None，绝不 fallback provider**。
- 硬契约满足：不建 session、不连接、不刷新凭据、不访问 provider、不建 job、不 wake、
  不 scan、不改 retry、不改任何 durable cover state。

## 6. 门禁

| 命令 | exit | 结果 |
|---|---|---|
| `cargo test --lib d2_ -- --test-threads=1` | 0 | **6 passed / 0 failed** |
| `cargo test --lib source::sftp -- --test-threads=1` | 0 | **5 passed / 0 failed**（SFTP CA 仍绿） |
| **`cargo test --locked -j 2 -- --test-threads=1`（全量）** | **0** | **18 个目标全部 ok；415 passed / 0 failed / 2 ignored**（上一轮 409，+6 CA） |
| `git diff --check` | 0 | clean |
| `P1_RECOVERY_INTEGRITY_PASS` | 0 | `cover_store.rs` 仍 `+465/−9`（未受影响） |

### 我引入并修掉的一个回归（如实记录）

首次全量门禁出现 **`cache::tests::cache_root_defaults_to_appdata` FAILED**。
原因：我的 CA 测试调用进程级 `cache::set_custom_cache_root(...)` 后**未恢复**，
污染了同进程内后运行的既有测试。修法：加 `CacheRootGuard`（`Drop` 中
`set_custom_cache_root("")`），即使 panic 也恢复；6 个用例各持有一个 guard。
修复后全量 **EXIT=0**。**这是我的测试问题，不是生产缺陷。**

## 7. §15 要求的措辞（如实采用）

> all deterministically recoverable historical cover identities are supported. Legacy raw-key
> covers for 115-web and Quark become unrecoverable if their raw file is removed, because the
> provider-returned filename was never durably persisted.

**不写**"all historical cover caches are recoverable"。

## 8. 未完成项（PASS 条件对照）

| PASS 条件 | 状态 |
|---|---|
| 1 六 provider authority 已测试 | ✅ |
| 2 六 provider 当前可恢复 old-cache 已测试 | ✅ |
| 3 Family 1 `present→absent` 可恢复 | ✅ |
| 4 115-web/Quark `present→absent` 明确测为 unrecoverable | ✅ |
| 5 所有 provider `absent→present` 可恢复 | ✅ |
| 6 writer/cache namespace 完全不变 | ✅（probe 语义与返回值未变；无迁移） |
| 7 local-only lookup 0 network | ✅（代码层：API 只读 `.cover`；R5 钉住不 fallback） |
| 8 offline legacy recoverable hit 可见 | ❌ **未做**（需 FRB codegen + legacy B） |
| 9 offline miss 0 session | ❌ 未做 |
| 10 online hit 0 session | ❌ 未做 |
| 11 online miss 走原 provider path | ❌ 未做 |
| 12 local/custom offline 可见 | ❌ 未做 |
| 13 unified remote 不回归 | ❌ 未验证（未改 unified 代码） |
| 14 SFTP writer/reader 共用 `endpoint_for` | ✅ |
| 15 无目录猜测或模糊匹配 | ✅（reader 无 scan/prefix/fuzzy） |

**未做原因**：Dart 要调用新 API 必须先跑 FRB codegen（`read_legacy_cover_local` 尚未生成绑定），
而 codegen + local A + legacy B + 8 个 Flutter 用例 + analyze 超出本轮剩余预算。
按 §17「能完成多少就完成多少并如实报告」，我在 Rust 侧完整闭环后停下，未把未验证的 Dart 改动留下。

## 9. Git / 清理

| 项 | 值 |
|---|---|
| branch / HEAD | `p1-cover-completion` / `1bf2e37`（未变，无 commit） |
| staged | 空 |
| 本轮改动（tracked） | `app/rust/src/api/source.rs`、`app/rust/src/source/{webdav,sftp,baidu,cloud115,quark}.rs` |
| 原有 dirty | P1-A/B/C/D 全部原样保留 |
| 临时产物 | 已清理（`D:\Temp\p1d2_*.py`、`p1d2_ca_tests.rs` 用完即删） |

## 10. Remaining risks

1. **PASS 条件 8~13 无证据**：Flutter 侧未动，因此"offline 能看到已有 legacy 封面"**尚未被证明**。
2. `read_legacy_cover_local` 是 `pub fn` 但**尚无 FRB 绑定**，Dart 目前无法调用。
3. Family 2 的 `absent→present` 依赖 reader 的 **current-then-alternate** 顺序；若上层将来调整顺序会失效
   （矩阵用例已钉住当前语义）。
4. Family 1 的 `raw_cache_candidate_path` 改变了 `raw_cache_path` 内部的 `ensure()` 调用位置
   （仍在 probe 之前，返回值与副作用不变），但这是**生产代码改动**，已由 CA/全量门禁覆盖。
5. `CA-3` 的副作用断言停留在"不新增缓存文件 + 无 session/network 代码路径"层面；
   "0 job / 0 wake" 由"API 根本不触碰 DB/worker"这一事实保证，**未用计数器直接观测**。
6. 仓库级既有红灯仍在：`cargo clippy`（`src/reader.rs:277`）、`flutter analyze`（121 issues）。
