# 缓存目录切换重构 — 实施计划（方向反转版）

## 前置

- 基线：`8a99f95` + `ea0916b`（标签归一化），master 干净。

## 步骤（按序，每步验证）

1. **Rust 迁移 API**（cache.rs + api/cache.rs + Cargo.toml 加 fs2）：`migrate_cache_root(from, to, support_dir)`、`migration_progress`、`available_space`、`delete_root_items`、`migration_pending`、`clear_migration_marker`；单元测试（排除支持目录、路径安全、删源范围、幂等）；`cargo test`。
2. **FRB 重新生成**：`flutter_rust_bridge_codegen generate`；`flutter analyze` 通过。
3. **启动链路（main.dart）**：读 `cache_root.txt` → JSON 兜底 → `setCacheRootPath` → 数据愈合（搬 database.db 到当前根）→ 迁移/加载；pending 标记检测 + 恢复对话框（复用生命周期 flush 容器）。
4. **Dart 切换流程（cache_manager）**：file_selector（pubspec 加依赖）+ 校验 + 空间检查 + 确认 + 进度 + 切换 + 写标记 + 删源；恢复默认路径。
5. **验证**：
   - `cargo test`、`flutter analyze` 0 issues；
   - 手工：切换 → 重启 → 书源/标签/记录/AI 缓存完整且数据库在新根；来回切换不丢；目标选支持目录被拒；空间不足被拒；迁移中杀进程 → 重启提示重试；升级首启：`D:\Documents\TEST\database.db` 自动搬入。
   - 完整 `flutter build windows`：延后由用户本地跑（静态门通过即可）。
6. **收尾**：勾选 PRD 验收项；提交；归档流程。

## 回滚点

- 步骤 1-2 后：删除迁移 API 文件即可（无 schema 变更）。
- 步骤 3 后：恢复 main.dart 旧启动顺序。
- 步骤 4 后：恢复旧切换对话框。

## 风险文件

- `app/rust/src/cache.rs`、`app/rust/src/api/cache.rs`
- `app/lib/main.dart`、`app/lib/ui/cache_manager.dart`
- `app/pubspec.yaml`（file_selector）、FRB 生成文件
