# Widget 生命周期约定（原模板的 "hook guidelines"）

> 2026-09-21 补实并**改写主题**：本项目**没有使用任何 hook 包**（无 flutter_hooks），
> 因此这里记录真实的 StatefulWidget 生命周期与资源释放约定。若将来引入 hook 包再按包模型增补，
> 不要沿用模板里的通用占位文字。

## 打开与加载

- 重 I/O 与原生调用放 `initState` 之后的异步路径（如 `_open()`），**不在 build 内发起**；
- 页面级缓存优先：先读本地缓存再决定是否走网络（阅读打开顺序 `raw-cache → stream → fallback-download`）；
- 异步返回后必须检查 `mounted` 再 `setState`。

## 释放（易漏，逐条对照）

- `dispose()` 必须：取消订阅（`removeListener`）、释放原生句柄（如 `closeBook(handle:)`）、
  清定时器、必要时还原系统设置（阅读页恢复 `SystemChrome` 方向）；
- 阅读器关闭时按设置处理整包：`deleteRawPackage(...)`（"阅读完成后自动删除整包"）；
  流式模式**不动任何缓存**；
- 全局单例在 dispose 里复位（如 AI 管理器的 `setReadingBook(null)`）。

## 反模式

- `dispose()` 之后触碰 `context`；
- 用静态/全局变量保存页面级状态；
- 忘记注销 `ValueNotifier` 监听（真机表现："返回后封面还在刷"）。
