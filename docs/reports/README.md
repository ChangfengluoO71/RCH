# docs/reports/ 已按性质归位（2026-09-21）

门禁与实验证据原先放在 `p0/`、`p1/`、`rg-b/` 与若干主题报告里。按 Trellis 的
"**证据随任务走**"原则，它们已迁入对应任务的 `research/`：

| 原路径 | 现路径 |
|---|---|
| `docs/reports/{p0,p1,rg-b}/*` | `.trellis/tasks/09-14-remote-cover-cleanup/research/{p0,p1,rg-b}/` |
| `cover-status-flicker-audit.md`、`rch-stability-remote-download-batch-2026-09-11.md` | `.trellis/tasks/09-14-remote-cover-cleanup/research/` |
| `rch-remote-cloud-scan-2026-09-14.md` | `.trellis/tasks/09-14-remote-cloud-scan/research/` |
| `rch-v057-*.md` | 保留在本目录（发布证据） |
| `catalog-*.json`（6 份，约 13.6 MB） | **已移出仓库**（本地归档于 `D:/Documents/RCH/evidence-archive/docs-reports/`），并加入 `.gitignore`：它们是 M8 目录规则的干跑/落地原始数据，体量大且非源码。**注意**：git 历史里仍然存在这些文件，彻底瘦身需改写历史（未执行，需单独决定） |

**历史文档里的旧路径不改**：`docs/project/LOG.md` 按 append-only 约定保留原样，
所以这里保留跳转说明以便追溯。长期结论沉淀在 `.trellis/spec/`，证据随任务归档。
