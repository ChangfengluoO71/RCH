# 云端封面与流式读取：Range、调度与取消调研（2026-08-30）

## 问题

默认加载未缓存云端漫画封面，但 Range 不可用时不能隐式下载整本；阅读当前页不能被海报墙请求拖慢；离屏请求不能形成风暴。

## 一手资料结论

| 结论 | 对本任务的影响 | 来源 |
| --- | --- | --- |
| `Accept-Ranges` 只是建议；客户端即使收到了它，也不得假定后续请求一定得到部分响应。Range 请求可被服务器忽略。 | 不以 HEAD/`Accept-Ranges` 决策。以真实、小范围 GET 的 `206 + Content-Range`（且范围一致）作为唯一成功判定；`200`、`416`、缺失/不匹配 `Content-Range` 均视为不能安全部分读取。 | [RFC 9110 §14](https://www.rfc-editor.org/rfc/rfc9110.html#section-14) |
| `If-Range` 验证器不匹配时，服务端会忽略 Range 并传输完整表示。 | 海报墙的 `remotePartialOnly` 不使用 `If-Range` 作为探测手段，否则可能违反“不能下载整本”。可将 ETag/mtime/size 仅用于版本化缓存与失效。 | [RFC 9110 §13.1.5](https://www.rfc-editor.org/rfc/rfc9110.html#section-13.1.5) |
| Tokio `Semaphore` 是公平 FIFO，不提供优先级；前方 `acquire_many` 还能阻塞后面的小请求。 | 不能用单一 Semaphore 声称“当前页优先”。实现具有三个队列的自定义请求预算，并保留前台许可；同优先级内 FIFO，避免低优先级永久饥饿。 | [Tokio Semaphore 文档](https://docs.rs/tokio/latest/tokio/sync/struct.Semaphore.html) |
| Tokio 的 `spawn_blocking` 已启动后不能被 `abort`。`CancellationToken` 只能在任务合作检查取消时生效。 | MVP 仅取消**尚未启动**的封面任务，已启动的同步 I/O 只丢弃 UI 订阅结果；不承诺强制中止网络。真正可取消 I/O 留作将来改成异步/合作式客户端后的独立优化。 | [Tokio JoinHandle 文档](https://docs.rs/tokio/latest/tokio/task/struct.JoinHandle.html)、[CancellationToken 文档](https://docs.rs/tokio-util/latest/tokio_util/sync/struct.CancellationToken.html) |
| singleflight 的核心模式是“同 key 的请求只执行一次，其他调用者共享结果”。 | `_CoverLoadQueue` 应变为稳定 key 的共享 Future/订阅者模型，而不是每个卡片都排一个任务。 | [Go `singleflight` 文档](https://pkg.go.dev/golang.org/x/sync/singleflight) |
| 429 表示限流，响应可携带 `Retry-After`。 | 解析并优先遵从 `Retry-After`；缺失时按书源做带抖动的指数退避和短期负缓存，禁止滚动/rebuild 立即重试。 | [RFC 6585 §4](https://www.rfc-editor.org/rfc/rfc6585#section-4) |
| `reqwest::blocking::Response` 是按 `Read` 消费的响应；读取完整 body 需要显式调用 `bytes()`、`text()` 等。 | 安全探测只检查 status/header，遇到 `200` 全量响应立即丢弃 Response，不把 body 送入解析或缓存。 | [reqwest blocking Response 文档](https://docs.rs/reqwest/latest/reqwest/blocking/struct.Response.html) |

## 代码验证

- RCH 已用 `Range: bytes=0-0` 探测部分来源，但有的实现仅检查 `206`，没有系统验证 `Content-Range`；需要统一为安全探测合同。
- `baidu_cover`、`cloud115_cover`、`quark_cover` 和 115 Cookie 封面 API 在 Range 不可用时会调用 `download_to_raw_cache`；这与海报墙默认加载封面的产品边界冲突。
- `Reader` 会为预取直接创建线程，封面调用也通过 `spawn_blocking`；现有 `_CoverLoadQueue` 只能取消未开始的 Dart 队列项。
- `CoverEditorPage` 当前按全局 `BookOpenStrategy` 打开远程书。`auto` 和部分 `stream` 路径可能直接整本下载，因此不能直接复用为“Range 不可用时先问用户”的自定义封面流程。

## 采纳的方案

1. 海报墙：`cacheOnly` 或 `remotePartialOnly`。后者只接受经实际响应验证的局部读取；失败显示既有“未缓存”占位，并对同一书源在本次浏览会话提示一次，不逐卡弹窗。
2. 自定义封面：先走本地缓存与 `remotePartialOnly`；结果为 `rangeUnavailable` 时展示确认框（来源、文件名、大小或“大小未知”、下载/取消）。仅点“下载整本”后调用显式下载路径。
3. 调度：Dart 侧 singleflight + pending 取消；Rust 侧同步优先级预算（当前页 > 预取 > 封面），并为当前页保留许可。已开始的 blocking I/O 不强制取消。
4. 失败控制：按资源版本的 Range 负能力缓存、按书源的 429/超时冷却，并在目录刷新、文件 size/mtime 改变或 TTL 到期时重新探测。

## 刻意不在本次 MVP 引入

- 为所有提供方把同步 HTTP 客户端重写为 async `reqwest`，以实现真正的中途取消；这是跨来源协议重构，应以本次基线数据决定是否另立任务。
- 多 Range 合并请求；标准明确提示客户端不要发出比一个连续范围更低效的多范围请求，现有 `SourceReader` 的连续读放大更适合保留。
