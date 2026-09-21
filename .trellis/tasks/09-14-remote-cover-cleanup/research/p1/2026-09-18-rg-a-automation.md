# RG-A 自动化总结报告（Release Gate A 类）

- **日期**：2026-09-18
- **范围**：把 Release Gate 中**可重复、可回归**的部分固化为自动化证据（RG-A），
  使后续真机/真账号验证（RG-B）不会混入基础语义错误。
- **基线提交**：`3945de5`（P1 全量）+ 本批 RG-A。

## 1. 阶段矩阵

| Gate | 内容 | 状态 |
|---|---|---|
| **A-1** | 20 / 100 / 300 MiB 全量下载正确性 | **PASS** |
| **A-2** | ADR-005 fallback orchestration | **PASS（WebDAV production reference）** |
| **A-3** | HTTP 错误语义与退避契约 | **PASS** |
| **A-4** | 事件驱动封面消费语义（consumer semantics only） | **PASS** |
| **A-5** | 原子权威 + 流式 / 失败清理 | **PASS** |
| — | **真实 FRB `StreamSink` 跨桥 delivery** | **未证明 ⇒ RG-B evidence gap** |

## 2. A-4 的边界（必须与 transport 区分）

**A-4 = event-driven cover consumer semantics**：首次 `requestCover` 恰一次；后续 wake
**只读 durable state**；**无 polling**；revision **dedup**；**missed-event catch-up**；
scan terminal 后 ready/failed 仍能更新；**F 与 E 共用同一 revision wake**。

这部分由 P1 的 Dart focused tests 覆盖（本批 fresh 重跑 **13/13 通过**）：

- `test/comic_cover_state_consumer_test.dart`（E-REQUEST-ONCE / E-NO-POLL / state matrix）
- `test/cover_revision_bridge_test.dart`（dedup / catch-up / 无周期读取 / dispose）
- `test/comic_cover_scan_terminal_test.dart`（scan terminal ready / failed / lifecycle）
- `test/cover_scan_terminal_integration_test.dart`（F 与 E 共用同一 wake）

> **真实 FRB `StreamSink` 端到端 delivery 未被自动化直接证明**，仍属 **RG-B / 真应用环境**
> 验证项。A-4 的 PASS **不得**被表述为 transport E2E PASS：消费语义与跨桥投递是
> 两个不同层级的证据。

## 3. A-2 的证据口径

> **WebDAV production reference path has automated end-to-end ADR-005 fallback coverage.**

> 5 providers / 7 full-download implementations share the atomic publication helper and
> have wiring coverage.

**不得**升级为 115 / Quark / Baidu / SFTP 的真实 provider E2E（未分别模拟各 provider 网络协议）。

## 4. 本批修掉的真实缺陷：raw-cache partial publication

**定性（修复前的三个事实问题，答案全为「是」）**：

1. 下载未完成时最终 raw-cache 路径**已对读取可见**（`File::create(最终路径)` 后才 `write_all`）；
2. 失败后**半文件保留**（无清理、无 `.part` 语义）；
3. 下一次会把它当**完整缓存复用**（复用判据 `metadata(path).len() > 0`）。

**修复**：新增 `cache::AtomicCacheFile`（与既有封面缓存**同一**临时名约定）：

- 同目录临时文件 → 写入 → `flush` + `sync_all` → `rename` 发布；
- 最终路径**只在完整写入成功后出现**（⇒ 复用判据 `len > 0` 语义**不需要**改动）；
- `Drop`（未 commit）与 rename 失败均删除临时文件；
- Windows 安全替换（目标已存在时先移除再 rename）；
- 临时名唯一：`.<name>.part-<pid>-<seq>`（pid ⇒ 跨进程；进程内单调序号 ⇒ 同进程并发不互相截断）。

**迁移**：**7 处** full-download 实现（fresh grep，非旧数字）：

| Provider | full-download 实现 | `AtomicCacheFile::create` | 直接 `fs::File::create(最终路径)` |
|---|---|---|---|
| webdav | 2（`download_full`、`download_to_raw_cache`） | 2 | 0 |
| baidu | 1 | 1 | 0 |
| cloud115 | 2 | 2 | 0 |
| quark | 1 | 1 | 0 |
| sftp | 1 | 1 | 0 |
| **合计** | **7** | **7** | **0** |

## 5. 测试过程中发现的其它事实

1. **分类器要求 206 忠实回显 `bytes 0-0`**（`adapter.rs`：`start != 0 || end != 0 || total == 0`
   ⇒ `MalformedResponse`）。即"声称支持 Range 却用 206 返回整段"被判定为 **malformed（fail closed）**，
   **而不是** supported ⇒ *不支持（200）≠ 声称支持但响应损坏（malformed）* 在字节区间层面被强制。
2. **`Stream` 分支真实依赖 `file_size()` = PROPFIND + `parse_multistatus` 的 `<getcontentlength>`**
   （而 `check_and_probe` 丢弃 PROPFIND 结果）。属生产结构事实。
3. **退避 cap 不可达**：`1_i64 << attempt.clamp(0, 10)` 的 `2^10 s` 上界在既有
   `attempt < 3` 阈值下**不可达**（可达延迟只有 1 s / 2 s / 4 s）；RG-A **未修改阈值**，
   仅把真实语义写入契约。
4. **不存在陈旧 `.part-*` 的清理逻辑**（崩溃残留会累积并计入缓存大小）⇒ 记为 backlog，本轮不做。
5. `downloader::blocking_download` 是**死代码**（零调用点），且它用 `resp.bytes()`
   整包入内存；7 处真实路径均为 `resp.read` 流式循环。本轮**不动**死代码。

## 6. fresh 门禁（本批）

| 目标 | 结果 |
|---|---|
| `cargo test --locked -j 2 -- --test-threads=1` | **EXIT=0 · 24 targets · 483 passed / 0 failed / 0 ignored** |
| `p0_baseline_read_speed`（P0-A 7 + A-1/A-5 6 + A2 6） | **19 / 19** |
| lib `rg_a`（决策 8 + 原子/唯一性 6） | **14 / 14** |
| `remote_cover_error_contract`（分类 4 + 既有 2） | **6 / 6** |
| A-4 Dart focused（4 文件） | **13 / 13** |
| `cargo build` / `git diff --check` | Finished / clean |

RG-A 专项用例合计 **30 条（Rust）+ 13 条（Dart）**。

## 7. 证据边界（必须保留的原话）

- **streaming behavior verified structurally/temporally; no fixed RSS ceiling asserted.**
  （内存模型证据 = server 仍在发送时 `.part-*` 已在增长；**不**声明 RSS 阈值）
- A-1/A-5 只证明 **incomplete bytes cannot create the final authoritative path**；
  未引入 checksum / 远端长度持久化 / manifest / sidecar。
- A-2 的 E2E 仅覆盖 **WebDAV**；其余 provider 只有共享 atomic writer 的 wiring 覆盖。
- A-4 只覆盖**消费语义**；**FRB 真投递**仍为 RG-B 缺口。

## 8. RG-B 仍未覆盖（Release Gate PENDING）

真实 115 / Quark 登录与读取 · 实际是否出现**新增** 403/405/429（与"处理语义正确"分属两层证据）·
WAF · CDN burst / rate / in-flight 实测 · D2 的真实 session / provider 边界 ·
**真实应用环境中的 FRB stream delivery** · 20/100/300 MiB 在真实网络下的 Range 与取消证据。
