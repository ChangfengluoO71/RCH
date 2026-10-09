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
- Widget `dispose` 中不得同步触发被其他活跃页面订阅的 `notifyListeners()`；需要延后清理时用 post-frame callback，并用 owner/token 校验，避免旧页面清掉新页面注册的状态。
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

## Reader Navigation for Variable-Height Streams

### 1. Scope / Trigger

Use this contract when a continuous reader can jump to a page whose image and preceding page extents have not all been laid out yet. Lazy image loading means page height is not known at the time a distant jump is requested.

### 2. Signatures

```dart
WebtoonNavigationModel({
  required int pageCount,
  int initialPage = 0,
  double estimatedHeight = 240,
});

bool measure(int page, double height);
double heightFor(int page);
double offsetFor(int page);
WebtoonNavigationIntent requestTarget(int page);
```

`ReaderPage` supplies `webtoonPlaceholderHeight` (currently 200 px) as the initial estimate.

### 3. Contracts

- `offsetFor(target)` is the sum of `heightFor(i)` for every page before `target`; unknown pages must contribute a non-zero estimate.
- `heightFor(page)` returns a valid measured height when available, otherwise the active unknown-page estimate. Before navigation starts that estimate may learn from measured images; once the user scrolls or jumps, freeze it for the rest of the book session.
- Ignore measurements that are non-finite, non-positive, or outside the current page range. Re-measuring a page replaces its previous value in the mean.
- Programmatic navigation intents are generation-scoped. A newer target or user drag invalidates older completions. Commit stable reading progress after a scroll settles, not from transient estimated viewport positions.

### 4. Validation & Error Matrix

| Condition | Required behavior |
| --- | --- |
| Target page has not been laid out | Compute a non-zero offset from initial or measured-height estimates. |
| Some pages are measured | Use exact heights for those pages and the measured mean for unknown pages. |
| Invalid image height | Ignore it; keep the prior measurement or fallback estimate. |
| New target or user drag supersedes an animation | Reject stale generation callbacks and clear the pending target on drag. |
| Scroll is still moving | Update viewport observation without committing stable progress until settle. |

### 5. Good / Base / Bad Cases

- Good: jumping to page 50 before layout uses 50 estimated extents. The estimate can learn from the opening viewport, then stays fixed after navigation so newly measured pages do not resize every unknown page at once.
- Base: jumping within already measured pages uses their exact extents.
- Bad: sum only measured heights while treating every unmeasured page as zero; distant page jumps land too near the beginning.

### 6. Tests Required

In `app/test/webtoon_navigation_test.dart`, assert the initial distant-jump offset, the offset after representative pages are measured, exact measured-height replacement, monotonic offsets, stale-intent rejection, gesture cancellation, and stable-page commit only after settling.

### 7. Wrong vs Correct

Wrong:

```dart
final offset = measuredHeights.take(target).fold<double>(0, (sum, h) => sum + h);
```

Correct:

```dart
final offset = model.offsetFor(target); // includes an estimate for every unknown page
```

### 8. Bounded Loading During Distant Jumps

- `ListView.builder` may build intermediate offscreen children while resolving a
  distant pixel offset. Its builder must not start a page read for every child
  it is asked to lay out.
- Keep page reads inside the shared `webtoonPageLoadRadius` around the explicit
  jump target or current viewport page. Intermediate children outside that
  window render estimated placeholders only.
- Keep ReaderPage's ordinary page-read budget at three. A current jump target
  may use one urgent fourth slot when stale requests already occupy that budget;
  the Rust governor still bounds actual remote work. Prioritize the target,
  discard queued reads outside the active window, and stop retries after a page
  leaves that window.
- Only rendered image heights are measurements. Loading placeholders must not
  enter the measured-height average, and each placeholder must use the same
  `WebtoonNavigationModel.heightFor(page)` estimate used to calculate jump
  offsets.
- Freeze the unknown-page estimate at the first user scroll or programmatic
  jump. Updating one image's measurement must not resize all unmeasured pages
  above the current viewport.
- Keep a programmatic target active until its first layout completes. Ignore
  scroll-end notifications generated by programmatic jumps; a user drag or a
  newer target invalidates the old intent.
- If the target is requested before the ScrollController attaches, retain it
  and execute the jump after the webtoon ListView attaches; updating only the
  logical page number is insufficient.

| Condition | Required behavior |
| --- | --- |
| Distant jump causes intermediate children to build | Do not start reads outside the target window. |
| Placeholder is measured before its image loads | Track its rendered size for anchor correction, but do not add it to navigation measurements. |
| Rapid scrolling leaves queued page reads behind | Keep the normal budget at three, allow at most one urgent target beyond it, and discard queued requests outside the new target window. |
| A measured page changes while the viewport is stable | Apply anchor correction and refresh the viewport-to-page mapping. |
| Jump runs before the controller attaches | Defer execution until the webtoon list has attached. |

### 9. E-Hentai Tag Visibility

- The `源:e站` row is a presentation toggle for imported tags. Hiding tags must
  not unlink them from the book or persist a destructive change.
- Keep the toggle visible while the book still has E-Hentai imported tags;
  showing the tags again must be possible from that same row.
| Programmatic jump emits a scroll-end before layout | Keep the target intent until the first post-layout callback. |
| New target or user drag supersedes an old jump | Ignore old completion and move the read window to the new target/viewport. |

Good: jumping to page 228 lays out estimated placeholders for intervening pages,
while reads are limited to pages 225–231. Bad: invoking `_ensure(index)` for
every intermediate child and filling the Rust blocking-request queue.

### 10. Reader Page Priority and Progressive Reveal

- Send only the current page/spread through the foreground `book_page` API.
  Bounded offscreen pages use `book_page_prefetch`, which must preserve the
  shared Rust `RequestPriority::Prefetch` priority and must not recursively
  start another prefetch window.
- Keep a queued low-priority page outside the `inflight` ownership set until it
  obtains a governor permit. This lets a later foreground target enter the
  priority queue first; active synchronous I/O remains shared and cannot be
  cancelled.
- If the user lands on a page with an active Dart prefetch call, advance its
  request generation and issue a foreground request. Ignore the older response.
- Show the original page bytes as soon as they arrive. AI cache lookup runs
  afterward and may replace the displayed image when it finds an enhancement.
- A page-load spinner represents only the current page or visible spread;
  loading offscreen neighbors must not keep it visible.
- `reader_diag.log` may record foreground page-load stage, source type, result,
  and elapsed milliseconds. Do not record a book path, title, page number, or
  provider credentials.

### 11. Compact Comic Display and Entry Settings

- Persist compact-layout poster density and comic-tap behavior in `AppSettings`
  through the existing settings JSON path. Missing values must default to two
  columns and detail-page taps; clamp the column count to `2..4` when loading.
- Use `comicPosterGridDelegate` for comic poster walls. It applies the saved
  fixed column count only when `isCompact(context)` is true and preserves each
  desktop grid's existing max-extent delegate.
- Route comic-item taps through `comicTapHandler`: compact layout opens the
  reader only when the preference is enabled and the item is readable; otherwise
  it opens details. Always provide the original desktop callback. Index-only and
  ghost entries remain detail-only, and folder navigation or selection actions
  stay outside this policy.
- Long-press detail prompts are compact-only and use
  `showComicDetailPrompt`; do not attach them to folders or selection-mode cards.
- `EhSubscriptionStore` canonicalizes the rule `host` to `e-hentai.org` after
  load and before save, probe, live import, or collection. Persist a legacy-host
  correction and keep the editable host field out of the settings UI.
