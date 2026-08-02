# 缓存目录切换重构 — 资源管理器选择 + 整个根目录自动迁移（数据留在用户所选目录）

> **【回滚记录 2026-08-01】** 第一版实现已全部回滚（`git reset --hard 8a99f95`，撤销 `be5a1f2`）：当时把"数据/缓存分离"落实为"数据固定系统支持目录"，与用户预期冲突，且实施期间出现"数据保存失败"。
>
> **【方向调整 2026-08-01】** 用户确认新方向：**不把数据搬到系统支持目录**。数据库 `database.db` 与缓存（`cache/`、`download/`）都留在用户自己选择的根目录（如 `D:\Documents\TEST`），同一根目录下分文件夹管理；切换目录时**整个根目录一起迁移**。"数据保存失败"根因已定位并修复（`ea0916b` 标签 hash ID 归一化，与本任务无关）。

## Goal

缓存目录（应用根目录）切换改为：资源管理器选择目标文件夹 → 应用自动迁移整个根目录（数据库 + 缓存）→ 切换并持久化 → 重启后仍生效。**数据始终留在用户选择的目录**，彻底修复"手动切换/复制后书源全部丢失"。

## Background（已确认的代码事实与根因）

1. **自定义缓存根目录重启后从不恢复**：`setCacheRootPath` 只改 Rust 内存 static（[api/cache.rs](C:/Users/cfl/Desktop/RCH/app/rust/src/api/cache.rs:101)）；`settings.cacheDir` 只写不读（[cache_manager.dart](C:/Users/cfl/Desktop/RCH/app/lib/ui/cache_manager.dart:110)、[models.dart](C:/Users/cfl/Desktop/RCH/app/lib/store/models.dart:205)），全仓搜索无启动恢复逻辑。
2. **SQLite 数据库位于缓存根目录**：`db_path() = cache_root()/database.db`（[db/mod.rs](C:/Users/cfl/Desktop/RCH/app/rust/src/db/mod.rs:26)）→ 切换根目录 = 数据库"换位"，旧库留在原地。**本方案保留此布局**（数据库跟随用户所选根目录），只补自动迁移与启动恢复。
3. **library.json 在应用支持目录**（`%APPDATA%\RCH\RCH\library.json`，与默认根 `%APPDATA%\RCH` 嵌套）——其位置归属见 O-JSON。
4. 当前切换 UI 为纯 TextField 手动输入路径，并明示"应用不会自动迁移"（[cache_manager.dart](C:/Users/cfl/Desktop/RCH/app/lib/ui/cache_manager.dart:52)），无迁移、无校验、无恢复。
5. 用户实测：手动复制到新目录后重启 → 书源全部丢失。根因链：新目录重启后不被读取（自定义根未恢复）+ 旧默认根被移动/清空 → 数据不可用 → 空库重建。
6. `cache_root()` 的全部使用点：纯缓存（`cache/page|raw|cover|thumb|ai`、`download/`）与 **`database.db`**——根目录里唯一的"数据"就是数据库；用户习惯"我的库 = 我选的目录"。
7. 当前数据状态：真实 `database.db`（含书源/标签/记录）在 `%APPDATA%\RCH\database.db`（及 `%APPDATA%\RCH\RCH\` 备份副本）；用户所选目录 `D:\Documents\TEST` 目前只有 `cache/` + `download/`——新方案落地后需把数据库**放回用户所选根目录**（R5 愈合）。
8. 标签 hash ID 归一化 bug 已修复（`ea0916b`），本任务基于该修复进行。

## Requirements

- **R1** 目录选择：弹出系统文件夹选择器（而非手输路径），支持选择/新建。
- **R2** 自动迁移**整个根目录**（O2 方向反转，数据留在用户所选根目录）：把旧根 `database.db` + `cache/` + `download/` + 其他根文件迁移到新根；**排除嵌套的应用支持目录**（默认根 `%APPDATA%\RCH` 下的 `RCH/` 子目录，library.json 所在）；显示进度；迁移失败/目标盘空间不足 → 中止并提示，不切换。
- **R3** 迁移完成后：切换内存根 + 持久化 `cacheDir`；**启动时在打开数据库之前恢复自定义根**（解决"设置存在数据库里、数据库在自定义根里"的先后依赖）：支持目录写 `cache_root.txt` 标记（切换时同步写、启动时先读），library.json 的 `settings.cacheDir` 作兜底。
- **R4** 提供"恢复默认缓存目录"路径（反向迁移整个根）。
- **R5** 修复书源丢失：迁移/恢复后重启，书源、标签、阅读记录、AI 缓存全部保留；**当前状态愈合**：启动时若用户所选根目录缺 `database.db` 而旧位置有 → 自动搬入。
- **R6**（O1-A 移动语义）迁移 = 复制 + 校验（数量/大小）+ 成功后删源；校验失败保留源并回滚，不切换。
- **R7**（O3）中断恢复：整体重试 + `migration.partial` 标记（含 from/to），启动检测到残留 → 提示继续迁移/稍后。
- **R8** 基线保障：`ea0916b`（标签归一化）已提交；本任务不得回退该修复。

## Acceptance Criteria

- [ ] 切换目录后立即重启，书源/标签/阅读记录/AI 缓存完整保留（数据库在新根目录里）
- [ ] 目标盘空间不足或复制中断：不切换、不丢数据、可重试
- [ ] 默认 ↔ 自定义来回切换，数据不丢
- [ ] 迁移过程有进度反馈，大目录不卡 UI（后台迁移）
- [ ] 升级后首次启动：`D:\Documents\TEST\database.db` 被自动搬入（从旧位置愈合），书源/标签/记录完整
- [ ] 重启后自定义根被恢复（数据库与缓存都指向所选目录）
- [ ] 迁移目标若包含应用支持目录（或其父/子）→ 拒绝
- [x] `flutter analyze` 0 issues（2026-08-01）
- [x] `cargo test --lib` 通过（27 passed，含迁移范围/排除支持目录/路径安全测试）

> 手工验证项（切换/重启/中断/空间不足/首启愈合）与完整 `flutter build windows` 联调构建需在桌面端执行。

## Open Questions（开工前确认）

- ~~**O2** 数据目录与缓存目录是否分离~~ → **方向反转（2026-08-01）**：不分离。数据库与缓存在同一用户所选根目录下分文件夹管理；切换目录时整个根一起迁移；`db_path()` 保持 `cache_root()/database.db`；默认根仍为 `%APPDATA%\RCH`。
- ~~**O1** 缓存迁移语义~~ → 已定：A 移动（复制 + 校验 + 成功后删源）
- ~~**O3** 迁移中断恢复~~ → 已定：整体重试 + `migration.partial` 残留标记（R7）
- ~~**O-JSON** library.json 位置~~ → 已定：**A 留在应用支持目录**（固定不迁移，作为最后一道备份；`filePath()` 不改）

> 所有阻塞问题已关闭；技术细节见 design.md / implement.md。
