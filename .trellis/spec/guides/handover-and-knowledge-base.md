# 接手本项目的入口与知识库（Handover & Knowledge Base）

> 目的：任何人（或 AI）接手本仓库时，用最少时间建立正确心智模型。
> 本文件是 **guides**（跨包通用），2026-09-21 建立。

## 1. 先读什么（顺序固定）

| 顺序 | 文件 | 为什么 |
|---|---|---|
| 1 | `CLAUDE.md`（仓库根） | 全局原则 + **文档规范**（README/SPEC/LOG/LOG-INDEX/DECISION/TODO 的更新时机与"必须确认"的操作清单） |
| 2 | `.trellis/workflow.md` | 工作流：Spec 系统、Task 系统、何时建任务、经验如何回写 spec |
| 3 | `.trellis/spec/backend/index.md`、`.trellis/spec/frontend/index.md` | 各层规范入口（含开发前检查清单与质量检查） |
| 4 | `.trellis/spec/backend/remote-cover-update-contracts.md` | **冻结契约**：封面/读取预算/唤醒与回退/重试/缓存治理，含 v0.6.0 追加段与可执行边界（测试名） |
| 5 | `docs/project/LOG.md` 最近 2–3 轮 + `docs/project/LOG-INDEX.md` | 最近发生了什么、为什么这么改 |
| 6 | `docs/project/TODO.md` | 未完成项与已知缺口 |

## 2. 代码知识库：Graphify（派生知识）

**定位**：Graphify 是**导航工具**，不是事实来源。用它定位文件/关系，然后**回到源码、契约与测试验证**。

```bash
graphify update .                 # 首次构建 / 增量重建（纯 AST，不需要 LLM 与 API key）
graphify query "封面为什么先读 legacy 缓存"   # BFS 遍历问答
graphify path "ComicCover" "read_legacy_cover_local"   # 两个节点的最短路径
graphify explain "RemoteScanCoordinator"               # 单节点+邻居的白话解释
graphify affected "SourceReader" --depth 2             # 反向影响面（改了它会影响谁）
graphify god-nodes --top 15                            # 架构枢纽
```

产物：`graphify-out/{graph.html, GRAPH_REPORT.md, graph.json}`。
**不入库**（`.gitignore` 已排除 `graphify-out/`）：它是派生产物，随源码演进会过期；
需要时本地重建即可（首次约数分钟，835 个源文件）。**不要**把它当作"项目文档"提交。

**图谱规模与"污染"警告（2026-09-21 首次构建实测）**：
- 规模：**14,503 节点 / 26,186 边 / 678 社区**；产物 `graph.html`（>5000 节点时自动聚合为社区视图）、
  `graph.json`（16.8 MB）、`GRAPH_REPORT.md`，共约 35 MB。
- **污染**：graphify 没有忽略文件机制，`app/rust/vendor/unrar_sys/vendor/**`（vendored unrar C++）
  与生成文件会被一起索引 ⇒ `god-nodes` 与报告里的"Surprising Connections"基本是 unrar 内部调用，
  **不要**据此判断架构枢纽。**定向查询不受影响**（实测有效）：
  ```bash
  graphify explain "SourceReader"     # 给出 源文件:行 + 社区 + 连接（含证据）—— 最好用的一条
  graphify query "封面为什么回退 legacy 缓存"
  graphify path "ComicCover" "read_legacy_cover_local" --undirected   # 默认有向图，查不到时加 --undirected
  ```
- **想要干净图谱**（报告类输出才有意义）时的配方：先临时把 `app/rust/vendor/unrar_sys/vendor`
  移出工作区（或改用"分路径构建 + `graphify merge-graphs`"），再 `graphify update . --force`，
  最后把目录移回。**不要**把 `graphify-out/` 提交进仓库。

**已知盲点（2026-09-21 实测）**：
- 76 个文件抽不出节点（`task.json`、`.template-hashes.json` 等纯数据/配置）——预期行为；
- 38 个文件有语法错误只能部分抽取（`generated_plugin_registrant.cc`、`main.cc`、vendored 的
  `arccmt.cpp` 等 C++/生成文件）——**分析 Rust/Dart 时不受影响**，但别指望图谱覆盖它们。

## 3. 本地门禁（提交前必跑；**与 CI 完全对齐**）

```bash
cd app && flutter analyze                 # 全量！CI 会分析 test/ 与 tool/，定点分析查不出来
cd app/rust && RUSTFLAGS="-D warnings" cargo check --locked --all-targets   # CI 带 -D warnings
cd app/rust && cargo test --locked -- --test-threads=1                      # 必须串行（缓存根是进程级全局）
cd app && flutter test test/doc_settings_paths_test.dart      # 文档里的设置路径必须与 UI 一致
```

**为什么必须全量**：2026-09-21 曾因 `RUSTFLAGS=-D warnings`（CI 默认）与全量 `flutter analyze`
（含 `test/`）在本地被"定点检查"掩盖，导致 master 连续三次 CI 红、发布被卡 ✗。

## 4. 交接 checklist

- [ ] `git status` 干净（除既有的 `app/{linux,macos,windows}/flutter/generated_*`，按约定不动）
- [ ] 上面三条门禁全绿
- [ ] 读 `docs/project/TODO.md` 与本文件第 1 节清单
- [ ] 需要代码导航时先 `graphify query`，再回源验证
- [ ] 施工结束：追加 `docs/project/LOG.md`、同步 `LOG-INDEX.md`、更新 `TODO.md`；
      用户确认功能后再更新 `README.md`；`SPEC.md` 原则上不动
- [ ] 会话收尾：`python ./.trellis/scripts/add_session.py`（记录会话日志）；
      完成的 Trellis 任务用 `task.py archive`
