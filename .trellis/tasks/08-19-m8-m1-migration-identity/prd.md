# M8-M1 — Catalog-Only Name & Role Extraction

## Goal

交付一个完全不依赖漫画内容、远程书源和 Provider 的通用识别版本：只使用 SQLite 中已有的文件名与上级目录名，得到漫画名、可选作者、可选提供者/平台和卷章关系。

## Requirements

- 输入为已持久化的 `CatalogSnapshot`，默认比较文件名和向上 3 级祖先目录；不得刷新目录或请求远程字段。
- 先识别卷/章/版本结构，再识别 provider/platform、author、title；所有角色保留原文、层级、规则和置信度证据。
- 作者与 provider 互斥，title 与 chapter 互斥；冲突时报告冲突或留空，不能静默混淆。
- 作者或标题不存在是合法结果；proposal 必须可预览、可重跑、可解释，并仅保存到本地工作态。
- 不读取 `ByteSource`、文件字节、封面、内嵌 metadata，不调用 Downloader、Provider 或 sync transport。
- 通过 M8-A0 `catalog_scrape` 通道自动触发；调度器传入的是已持久化 snapshot，不得因为自动触发而刷新书源。

## Acceptance Criteria

- [ ] LocalFile、FullyCached、RemoteOnly catalog snapshot 都能运行；RemoteOnly 的远程书源请求数为 0。
- [ ] fixtures 覆盖 bracket group、`作者 - 标题`、平台/作者/标题多级目录、卷章数字、系列目录和缺失作者。
- [ ] 输出 `title?`、`authors[]`、`provider?`、`volume?`、`chapter?` 及逐项 evidence/conflict。
- [ ] 章节/卷标记不会进入 title；provider 不会进入 author；author 不会进入 provider。
- [ ] 同一 snapshot + rule version 重跑结果稳定，不写 canonical。
- [ ] 启动、catalog revision 或同步完成触发时可由协调器生成去重的 working proposal；同步/Provider 失败不影响本地 parser。
- [ ] parser/unit/integration tests 通过并可生成 100 本 corpus 的覆盖率与混淆报告。

## Dependencies / Out of Scope

- 依赖父任务已确认的 Catalog-Only 设计和 M8-A0 的 `CatalogSnapshot` 调度契约。
- 不实现 canonical migration（M8-M2）、Provider（M8-M3）、review UI 或 sync。
