# State Management（Flutter 侧状态约定）

> 2026-09-21 补实（原为半成品）。

## 三类状态，各有归宿

| 类型 | 载体 | 例子 |
|---|---|---|
| 全局单例状态 | `LibraryStore.instance`（含 `AppSettings`） | 设置、资料库索引、同步状态 |
| 页面/子系统状态 | `ChangeNotifier` + `ValueNotifier` | `RemoteScanCoordinator.status`、阅读器显示宽度 |
| 一次性异步结果 | `Future` + `FutureBuilder` | 封面图、目录列举 |

- **UI 只订阅、不猜**：`ValueListenableBuilder` / `AnimatedBuilder` 监听 notifier；
  **不在 build 内做 I/O**，也不在 locked frame 内推 notifier（历史上有过 `widget tree was locked`）。
- **禁止轮询**：封面推进由 source-level revision 唤醒驱动
  （`RemoteScanCoordinator.coverRevisionFor` → `_onCoverRevisionChanged`）；30×350ms 轮询已删除，
  新增逻辑不得再引入定时轮询。

## 缓存与失效

- 卡片封面 L1：`ComicCover` 进程内 LRU（`CACHE_CAP`）；切换数据源/尺寸档时显式失效
  （`ComicCover.clear()`），不要依赖 GC；
- 阅读器页缓存按**渲染宽度**分目录（`page/<ns>/w<width>/`），标准档（width 0）与历史路径一致；
  切换宽度必须清 L1，避免新旧尺寸混用。

## 可测性（硬要求）

- 需要 I/O 或平台能力的类必须提供**可注入 loader/边界**，例如
  `RemoteCoverRepository(directoryLoader:, readLoader:, requestLoader:, stateLoader:, releaseLoader:)`
  与 `ComicCover(legacyLocalCoverReader:, legacyRemoteCoverLoader:)`；UI 不得直接 new 出网络/DB 调用；
- **不要对不存在的能力写测试**：spec 描述了但代码未落地的 seam，测试要锁"真实存在的 API"
  并在文件里写明缺口（见 backend/quality-guidelines 的实例）。

## 反模式

- 在 widget 里 `setState` 驱动订阅型数据（应由 notifier 驱动）；
- 用 `Future.delayed` / `Timer` 当同步手段；
- 页面自己保存可被多处修改的共享可变状态（应回收到 store/coordinator）。
