# Component Guidelines（Flutter 组件约定）

> 2026-09-21 补实（替换原模板）。每条都能指向真实组件。

## Overview

本项目没有组件库/设计系统层，组件就是 `lib/ui/` 下的 widget：
- 页面：`*_page.dart`（`BookDetailPage` / `ReaderPage` / `CoverEditorPage` / `HomePage`）；
- 可复用控件：同目录的普通文件（`ComicCover` / `RemoteScanStatusPanel` / `Cloud115CookieQrScanDialog`）。

约定：**无状态优先**；需要异步或订阅时用 `StatelessWidget` + `FutureBuilder` / `ValueListenableBuilder`，
只有真正持有可变状态（页码、裁剪框）才用 `StatefulWidget`。

## Component Structure（一个组件文件的固定骨架）

```dart
class XxxPanel extends StatelessWidget {
  const XxxPanel({
    super.key,
    required this.sourceName,        // 必需数据：required + 具体类型
    this.onRetry,                    // 可选回调：可空 + 命名
    this.stateListenable,            // 订阅源：ValueListenable 而不是裸值
  });

  final String sourceName;
  final Future<void> Function()? onRetry;
  final ValueListenable<RemoteScanViewState?>? stateListenable;

  @override
  Widget build(BuildContext context) { /* 只做布局与订阅，不做 I/O */ }
}
```

- **构造参数即 props**：全部命名参数；必需项 `required`；类型写具体（不用 `dynamic` / 裸 `Map`）；
- **可测性**：需要 I/O 或平台能力的组件必须暴露**可注入边界**，例
  `ComicCover(legacyLocalCoverReader:, legacyRemoteCoverLoader:, coverRepository:)`
  —— 生产默认走 Rust FRB，测试注入假实现（见 `test/comic_cover_legacy_fallback_test.dart`）。

## Props Conventions

- 不传 `BuildContext` 之外的环境对象；不把 store/单例当参数传（直接 `LibraryStore.instance`）；
- 回调命名 `onXxx`，返回 `Future<void>` 的用 `Future<void> Function()?`（便于 `await` 与测试断言）；
- 尺寸约束交给父级（`SizedBox(width: 220, height: 310, child: ComicCover(...))`），组件内部不写死布局尺寸。

## Styling Patterns

- **颜色/文字样式一律取主题**：`Theme.of(context).colorScheme.onSurfaceVariant`、`textTheme`；
  **禁止**硬编码 `Colors.grey` 之类（对比度问题曾因此返工）；
- 提示/无障碍：可点击图标要有 `tooltip`（中文，如 `'重试远程扫描'`）；
- 文案面向用户、中文、可被测试断言（`find.text('获取失败')` / `find.byTooltip('重试远程扫描')`）；
- 空态/失败态要有明确文案（`等待扫描` / `获取失败` / `暂不支持`），不要留空白容器。

## 文档中的 UI 路径必须可校验（2026-09-21）

- 文档（README、用户手册）里出现的 `设置 → …` 必须与 UI 中的**真实分组 / 小节 / 入口名**一致。
  路径分段取自 `app/lib/ui/home_page.dart` 的分组标题与 fontSize 16 / w600 的小节标题，
  以及各面板标题（`cache_manager.dart`、`update_panel.dart`、`backup_panel.dart` …）。
- 由测试固化：`app/test/doc_settings_paths_test.dart` = 词表白名单 + **漂移守卫**
  （词表里的标签若在代码中消失即失败）。CI 的 analyze job 会运行它。
- 分组归属不确定时，**只写用户可见的标签**，不要编造层级路径（例：`自动转 CBZ` 只写开关名）。
- 教训：曾出现 `设置 → 刮削`（实际在「书源与网络」分组下）与 `设置 → 同步`（实际分组为
  「同步与备份」）共 **8 处失效路径**，用户按文档找不到入口。

## 反模式

- 组件内部起定时器/轮询推进数据（订阅 notifier 或 revision 唤醒）；
- 在 `build` 内发起网络/DB/原生调用；
- 直接在 UI 里 new 出 repository/网络客户端（必须可注入）；
- 一个文件塞多个页面（设置项过多时按类别折叠，见 `home_page.dart`）。
