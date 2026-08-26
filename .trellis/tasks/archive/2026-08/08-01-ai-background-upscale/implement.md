# AI 整本超分后台计划运行 — 实施计划

## 前置

- 基线：`a50eeef`（AI 遗留修复完成）；`70a5b5a`/`ea0916b` 已含。

## 步骤（按序，每步验证）

1. **Rust 数据层**：`ai_tasks` 表（init_tables）+ `upsert_ai_task`/`load_all_ai_tasks`/`delete_ai_task` + FRB 桥；单元测试（upsert/load/delete 往返）；`cargo test`。
2. **FRB 重新生成**：`flutter_rust_bridge_codegen generate`；`flutter analyze` 通过。
3. **AiTask 模型 + 管理器**：`ai_upscale_manager.dart`（模型、enqueue/去重/worker/取消/启动恢复/readingBookKey/forceAiVersion）；单元级逻辑自测（去重、状态流转）。
4. **全局悬浮小窗**：`main.dart` MaterialApp `builder` Stack + `AiFloatingProgress`（进行中/排队/取消/完成提示条）。
5. **完成提示**：`navigatorKey` + 阅读检测注册（ReaderPage init/dispose）+ 完成对话框 + forceAiVersion 通知。
6. **阅读界面版本切换**：ReaderPage `_useAiVersion` + 右键菜单/AppBar 入口 + 切换清页重载。
7. **详情页接入**：`_upscaleAll` 改 enqueue；按钮状态监听 manager；启动恢复续跑逻辑挂到 main()。
8. **验证**：
   - `cargo test`、`flutter analyze` 0 issues；
   - 手工：确认后立即后台执行不卡 UI；悬浮小窗进度实时；多书排队串行；重复入队忽略；取消后块边界停止且缓存保留；重启后续跑（已完成页秒过）；完成时阅读该书 → 弹切换提示；阅读器内原版/超分切换不丢页码。
   - 完整 `flutter build windows` 延后由用户本地跑。
9. **收尾**：勾选 PRD 验收项；提交；归档流程。

## 回滚点

- 步骤 1-2 后：删除 ai_tasks 相关函数即可（新表无破坏）。
- 步骤 3-7 后：`git revert` 相关文件；详情页回退为原前台循环。

## 风险文件

- `app/rust/src/db/mod.rs`、`app/rust/src/api/db.rs`
- `app/lib/ui/ai_upscale_manager.dart`（新增）、`app/lib/ui/ai_floating_progress.dart`（新增）
- `app/lib/ui/reader_page.dart`、`app/lib/ui/book_detail_page.dart`、`app/lib/main.dart`
