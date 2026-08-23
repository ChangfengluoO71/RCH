# TODO.md — 任务看板

> 状态看板:Backlog(待办)/ Doing(进行中)/ Waiting(待验证)/ Done(已完成)。
> 里程碑验收标准见 SPEC.md。

---

## 0. 工作流程(每轮闭环)

1. **读文档对齐** — 读 README(当前状态)+ TODO(看板)+ SPEC(目标)
2. **判断类型** — Bug修复 / 功能开发 / 重构 / 架构调整。架构级先征求用户确认
3. **小步开发** — 最小修改、不动已有功能。改 Rust API 后 generate 桥接
4. **验证** — `cargo test/check` + `flutter analyze` + `flutter run` 实测
5. **立即记档** — 更新 LOG、LOG-INDEX(起止行+HASH)、TODO(状态流转)
6. **交用户验证** — 说清改了什么、怎么测
7. **用户确认后** — 更新 README。若涉及 SPEC 变更再单独确认;TODO 归档

---

## Doing(进行中)

### 使用反馈修复（2026-08-17 长风落反馈，任务 08-17-usage-feedback）
- [x] 条漫模式滚动时页码实时跟随（AppBar 页码）
- [x] 条漫模式底部页码/进度栏（‹ 页码/总数 ›）+ 翻页/跳转
- [x] 书源界面顶栏与状态栏重叠（SafeArea 修复）
- [x] PC 端应用图标更换（紫底白字 RCH，生成多尺寸 ico + 安装器图标）
- [x] Windows Release 构建验证图标生效
- [x] MuMu 安装新版本验证
- [x] 发布 v0.5.1

### M2 AI 高清引擎
- [x] Phase 1: CLI 单次调用方案（临时文件传图）→ **第 39 轮已实施**
  - [x] 制作/配置 librealesrgan-ncnn-vulkan.exe + 模型文件（app/windows/ai/）
  - [x] Rust `ai/` 模块: 单次 Command 调用 + 超时 60s + sha256 缓存
  - [x] Rust `api/ai.rs`: 暴露 `super_resolve(page_bytes, scale) -> result_bytes`
  - [x] Dart: 阅读器右键菜单 → 触发超分 → SnackBar 进度 → 替换当前页显示
  - [x] 结果缓存: ai/ 目录 sha256 key 缓存，重复请求不走 CLI
  - [x] CMake 集成: install(DIRECTORY ai/) 随安装程序分发
- [x] Phase 2: CLI 目录批量模式 → **第 40 轮已实施**
  - [x] `super_resolve_batch()` 一次 CLI 调用处理整个目录
  - [x] ONNX 模型转换（pth → onnx，推理一致性验证通过）
  - [x] 评估 ort crate 在 FRB cdylib 环境中的可行性（结论：不可用，待稳定版）
- [ ] Phase 3: ONNX Runtime 直接推理（模型已转 ONNX，待 ort crate 稳定后切换 + Upscaler trait）

## Backlog(待办)

### 规划完成(待开工) — 2026-08-02 批量规划
- [ ] `08-02-m5-book-sources` — M5 书源扩展(SMB / SFTP)
- [ ] `08-02-tag-meta-hierarchy` — 元数据标签按作者/类别/系列/状态分层折叠
- [ ] `08-02-reader-zoom-pan-bug` — 修复缩放后移动区域只在第一页生效
- [ ] `08-02-extension-alias-dedup` — 后缀名变更识别(zip→cbz 视为同一本)
- [ ] `08-02-export-cbz` — 本地漫画转 CBZ(文件夹/ZIP 打包)
- [ ] `08-02-avif-support` — AVIF 格式支持
- [ ] `08-02-reader-page-rotate` — 阅读器页面旋转(M4 子集)
- 已删除:阅读器核心 backlog(双页拼接自动模式 / 滚轮缩放 / 自定义按键绑定),用户确认不做

### 后续里程碑
- [x] ADR-016: Repository 层实施（Tags 已完成，Books/History/Settings 待后续）（第26-29轮已实施 TagRepository）
- [x] ADR-017: 标签独立建模（Tag 实体 + BookTag 关联，解决标签补全 Bug）（第26-29轮已实施）
- [ ] M4 复杂场景(智能拼页 / 裁边;旋转已拆为 `08-02-reader-page-rotate`)
- [ ] M5 书源扩展(SMB / SFTP / 更多网盘) — 已建任务 `08-02-m5-book-sources`(SMB+SFTP)
- [ ] M6 Android 适配(手机 / 平板)
- [x] M7 标签筛选:按标签过滤书架(标签数据模型 + 跨书源搜索已落地)
- [ ] M8 Smart Scraping: catalog-only recognition + automatic sync integration (M8-M1 proposal slice frozen after real-sample validation; enrichment and canonical confirmation remain)
  - [ ] M8-A0 Automation Coordinator & Sync Integration: implemented; verify startup, debounce, periodic and sync-before-scrape behavior
  - [x] M8-M1 Catalog-Only Name & Role Extraction (`catalog-rules-v3`): frozen after after8 347-row local/Quark validation; proposal-only, zero remote book-source I/O
  - [ ] M8-M2 Canonical Identity & Migration: ordered DDL, works / external IDs / work links
  - [ ] M8-M3 Optional Provider Enrichment: independent AniList + Bangumi runtime
  - [ ] M8-M4 Candidate & Explainable Ranking
  - [ ] M8-M5 Review, Confirmation & Sync-Dirtiness
  - [ ] M8-M6 Corpus Validation: 100 real comics and role-confusion matrix
  - Later: M8.1 Provider Expansion; M8.2 Advanced Evidence; M8.3 Metadata Taxonomy; M8.4 Discovery; M8.5 Export & Interop

---

## Done(已完成)
- [x] 立项与总体规划(SPEC / LOG / DECISION / 文档体系建立)
- [x] ADR-016/017 标签系统重做 + 搜索系统统一（第27-29轮）
- [x] M9 缓存基础设施: 五级缓存 + 统一下载器 + SQLite 数据层（第30-32轮）
- [x] ADR-018 Repository 层扩展到 Book + Record（第36轮）
- [x] 核心阅读闭环: 书源 + ZIP/CBZ 流式 + 三模式 + 双页拼接 + WebDAV
- [x] 8种格式引擎: EPUB/Folder/CB7/CBT/PDF/CBR/MOBI
- [x] 标签系统: 补全/筛选/管理/批量/元数据标签/已读标记
- [x] 搜索系统: 内联补全 + Chip + 跨书源全局搜索
- [x] 缓存体系: 五级分类 + 独立管理面板 + 封面懒加载
- [x] 封面自定义: 选页 + 裁剪 + 质量可调
- [x] 数据层: SQLite 迁移 + Repository 层 + 封面磁盘缓存 + 并发加载限流（第36-38轮）
- [x] 文档体系: (LOG / LOG-INDEX / README / TODO / DECISION / SPEC)
- [x] 书源同步导出补全: 手动导出到文件 + 加密书源凭据包 + Android 导出降级（第42轮）
