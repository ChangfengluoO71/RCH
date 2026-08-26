# AI 整本超分后台计划运行 — 技术设计

## 1. 架构总览

```
详情页"整本 AI 超分"确认
  → AiUpscaleManager.enqueue(book)   [去重 + 落库 ai_tasks]
  → worker 串行执行（一次一个任务，GPU 共享）
      openBook → 分块 superResolveBatch(20页/批) → 进度 → 完成
  → 完成：打 "AI超分" 标签 + saveToDisk
  → 通知：正在阅读该书 → 弹"是否全部加载为超分版本？"
          不在阅读 → 悬浮小窗"完成"提示条（数秒后消失）

全局悬浮小窗（MaterialApp builder Stack）：显示进行中/排队任务 + 取消
ReaderPage：挂载时注册 bookKey（阅读检测）；运行时原版/超分切换
```

## 2. 数据层

### 2.1 SQLite `ai_tasks` 表（init_tables 新增，幂等 CREATE）

```sql
CREATE TABLE IF NOT EXISTS ai_tasks (
    id TEXT PRIMARY KEY,
    book_key TEXT NOT NULL,
    source_type TEXT NOT NULL,
    source_id TEXT NOT NULL,
    path TEXT NOT NULL,
    title TEXT NOT NULL,
    scale INTEGER NOT NULL DEFAULT 2,
    total INTEGER NOT NULL DEFAULT 0,
    done INTEGER NOT NULL DEFAULT 0,
    status TEXT NOT NULL DEFAULT 'queued',  -- queued/running/canceled/done
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
```

### 2.2 FRB 桥（api/db.rs）

- `db_upsert_ai_task(task)` / `db_load_all_ai_tasks()` / `db_delete_ai_task(id)`
- 进度与状态的每次变更都 upsert（写入频率：每块一次，可接受）。

## 3. AiUpscaleManager（ChangeNotifier 单例）

- 状态：`List<AiTask> tasks`（启动时从 DB 加载）、`String? readingBookKey`。
- `enqueue(...)`：同一 `bookKey` 已有 queued/running → 忽略（R5）；否则建任务落库并通知，随后唤醒 worker。
- `_worker()`：串行循环——取第一个 queued → status=running → 执行：
  1. `openLocalBook`/`openWebdavBook`；
  2. 按 20 页分块 `superResolveBatch`（Rust 内部缓存命中直接返回）；
  3. 每块结束更新 `done` + upsert + notify；检查取消标记（canceled → 停止后续块，任务删除/保留为 canceled）；
  4. 全部完成 → status=done → 打 `AI超分` 标签 + `LibraryStore.saveToDisk()` → 触发完成通知 → 数秒后从 DB 删除。
- `cancel(id)`：置 canceled；进行中的 CLI 调用跑完当前块后停止。
- 启动恢复：main() 加载 DB 中的 queued/running 任务 → 重置为 queued → 唤醒 worker（已完成页靠缓存命中快速跳过）。

## 4. 全局悬浮小窗

- `main.dart` 的 `MaterialApp.builder`：`Stack[child, AiFloatingProgress()]`；
- `AiFloatingProgress` 监听 manager：进行中/排队任务显示在右上角小卡（书名 + `done/total` + 取消按钮）；无任务时隐藏；
- 完成且不在阅读 → 小窗变"完成"提示条，3 秒后消失（任务已从 DB 删除）。

## 5. 完成提示（R3）

- `MaterialApp.navigatorKey`（main.dart 新增全局 key）；
- 任务完成时若 `readingBookKey == task.bookKey` → `showDialog`（经 navigatorKey）：
  "该漫画已超分完毕，是否全部加载为超分版本？" → 确定：设置 `AiUpscaleManager.instance.forceAiVersionFor(bookKey)` 并 notify；
- ReaderPage 监听 manager：bookKey 匹配且 forceAiVersion → 切换为超分版（见 6）并清标记。

## 6. 阅读界面原版/超分切换（R4）

- ReaderPage 新增 `_useAiVersion = true` 状态；
- `_ensure` 按 `_useAiVersion` 决定是否查 AI 缓存（false = 读原版，等价现有 `skipAiCache`）；
- 切换入口：右键菜单加"原版 / 超分版本"项 + AppBar 小按钮；
- 切换时：清空当前视口页的 `_bytes` 并重新 `_ensure`（阅读页码不变，进度不丢）。
- 详情页"阅读未超分版本"（openBookNoAi）继续保留。

## 7. 详情页接入（R1）

- `book_detail_page._upscaleAll` 改为：确认对话框（保留）→ `AiUpscaleManager.enqueue(...)`；
- 原逐页循环逻辑移除（迁移到 worker）；按钮状态改为监听 manager（进行中显示进度）。

## 8. 风险

| 风险 | 缓解 |
|---|---|
| 批量调用中途取消粒度粗（当前块跑完） | 块间检查，20 页粒度可接受 |
| 任务持久化与 library 数据一致性 | ai_tasks 独立表，不影响书库；完成才打标签 |
| 重启续跑时书源不可用（本地路径删除） | 执行失败 → 任务置 canceled，提示 |
| 悬浮小窗遮挡阅读 | 右上角小卡 + 可收起；阅读页可用 hide 开关（后续优化） |
