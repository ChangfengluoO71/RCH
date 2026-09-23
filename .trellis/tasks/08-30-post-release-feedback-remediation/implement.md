# 8月29日反馈整改：实施编排

## 启动关卡

- [ ] 三个子任务的 `prd.md`、`design.md`、`implement.md` 经评审确认。
- [ ] 条漫保持独立切换到 `in_progress`。`cover-and-update` 批次在两份子任务文档都完成评审后可同时进入实施；Trellis 的 current-task 指针始终指向当前正在修改的一项，两个任务仍各自记录状态、测试和提交。
- [ ] 开始实现前加载对应 Flutter/Rust 层的 Trellis 规范，并记录基线命令输出。

## 交付顺序与评审关卡

1. **条漫稳定性（P1，独立）**
   - 完成纯状态/布局模型测试与阅读器 widget 回归。
   - 人工验证 50 页不等高条漫后，评审是否单独发布。
2. **cover-and-update 批次（封面 P1 + 更新 P2）**
   - 封面先交付可重复的请求基线和 fake provider 测试，再改调度与远程封面策略；更新先以平台启动器替身验证状态机与下载互斥。
   - 两项可共享同一工作分支和全量验证轮次，但封面的 Rust/FRB 代码生成、真实书源手工验证，与更新的 Windows/Android 系统交接冒烟必须分别完成。
   - 评审时分别检查封面的 Range 降级与请求上限、更新的镜像回退与 UAC/未知来源失败恢复；任一项可单独回滚或延后发布。

## 父任务完成条件

- [ ] 三个子任务各自通过其实施计划中的自动化与人工验证。
- [ ] 三项变更可独立回滚，且发布说明明确平台限制。
- [ ] 在同一构建上运行完整 `cargo test`、`flutter analyze` 与 `flutter test`。
- [ ] 汇总性能前后数据、已知限制及下一步（插件系统与自动化需求仍在范围外）。

## Batch checkpoint (2026-09-11)

The cover/update batch and the independent webtoon stability task now have implementation and automated regression coverage in the working tree. The repository remains a development checkout: full Rust/Flutter verification and real-device/provider smoke are the remaining release gates. Online smart scraping, E-site metadata scraping, and 115 automation stay explicitly deferred to a future plugin and are not part of this parent task.

## Progress (2026-09-23)

- `08-30-webtoon-page-stability` — **done and archived** (`archive/2026-09/`). The reported symptom (fast
  downward scrolling in webtoon mode jumping back several pages) was fixed by scroll-anchor compensation for
  placeholder→real height convergence (`WebtoonAnchorKeeper` + `position.correctBy`), with a real-`ListView`
  control-experiment regression (anchor delta <1px compensated vs 5600px pushed away uncompensated) and
  on-device validation (OPPO PGFM10, 0.6.2+102602; user confirmed the jump is gone).
  Not closed by that task and carried as its own TODO: wiring `WebtoonNavigationModel` into the reader
  (page-number / reading-progress correctness), which the original design assumed but never shipped.
- `08-30-update-download-install-handoff` and `08-02-cover-loading-perf` — still open.
