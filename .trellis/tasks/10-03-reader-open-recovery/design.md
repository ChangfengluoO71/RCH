# 阅读器打开与失败恢复设计

## 现状与决定

`openBook` 在 `Navigator.push` 前获取远程 session 和检查图片目录 manifest，所以等待这一步时读者看不到阅读器的加载阶段。`ReaderPage._open` 会调用各来源的 Rust 打开 API，但错误页面只有文本；`_ensure` 的失败处理只移除 loading 标记，后续 build 又能发起同一请求。`onPageChanged` 同步 dispose 离屏 PhotoView 控制器，可能早于 `PhotoView` 子树卸载。

将来源 session 获取和打开请求集中到 store 层的 `BookOpenCoordinator`，`openBook` 先推入 `ReaderPage`，阅读器通过 coordinator 订阅阶段/下载进度。Coordinator 持有 provider 选择、session 解析、远程图片目录判定和 Rust open API 调用；页面只管理展示、首屏/page 状态和用户动作。

## 打开数据流

1. `openBook` 保留阅读记录和初始页语义，立即推入 `ReaderPage`。页面打开期间显示明确阶段，不再等 session 创建后才导航。
2. `BookOpenCoordinator.open` 解析对应来源 session，报告连接完成，再检查远程图片目录 manifest，调用匹配的 Rust open API。
3. 现有各来源下载百分比进入同一进度回调；stream 模式保持不显示下载条。成功返回 handle 后，ReaderPage 进入首屏阶段并请求初始页。
4. 初始页成功显示后转为就绪。session、manifest、book open 或首屏读取失败时保留错误阶段，并提供明确的重试动作。

Coordinator 的边界不持有 widget/context。ReaderPage 以操作 generation 防止重试或退出后的旧结果覆盖新状态；若已卸载时 Rust open 晚到成功，立即调用 `closeBook` 释放 handle。页面退出会停止 Dart 侧进度订阅和状态更新。当前 provider 的同步阻塞调用没有统一中断协议，退出不会承诺立刻中止网络 I/O。

## 单页恢复

- 每个 page index 有独立的 loading/success/failure/attempt 状态。
- 自动最多 3 次请求（首次 + 2 次重试），短暂退避后重试；仅当前可见页的失败需要立即展示错误卡片，预取页失败保留可重试状态。
- 最终错误态阻止 build 自动重新请求。显式点击“重试”才重置该页尝试计数并再次加载。
- 一次读取的结果只能写回与发起时相同的 book handle 和页面 generation；切换漫画/AI 版本/卸载后忽略旧结果。

## PhotoView 生命周期

- 页面控制器窗口仍保留当前页及相邻页。
- 页面切换后先更新窗口，使用 `addPostFrameCallback` 延迟清理；回调执行时再次检查页窗口和 controller identity，只释放依然离屏的控制器。
- ReaderPage 最终 dispose 时统一释放尚存控制器。不得在 PageView 仍持有 PhotoView 子树期间先 dispose 外部传入的控制器。

## 诊断

- 对 Quark 流式失败，在进入回退下载前先冻结 stream attempt elapsed；download duration 独立计时；`total_ms` 在完整 open operation 返回后记录。
- Flutter reader diagnostic 为 session acquisition、book open 和 first page 单独记录毫秒数，包含来源类型、阶段和结果类别，不记录完整路径、书名、URL 或凭据。
- 使用现有 `reader_diag.log` `key=value` 事件格式；新增字段只追加，不改变旧事件字段含义。

## 备选方案与取舍

1. 只给当前 ReaderPage 的 spinner 加文字和按钮：改动最小，但 ReaderPage 尚未创建时的远程连接仍然没有阶段反馈，也会继续在 UI 内分散 provider 分支。
2. 在 `openBook` 里显示 dialog：能覆盖连接阶段，但对 route pop、重试、Progress owner 和迟到 handle 的生命周期管理容易变成第二套加载状态。
3. 先导航、再由 store 层 coordinator 管理一次打开操作：各阶段由一个 owner 负责，页面只消费状态；跨文件改动较多，但能统一连接、打开、进度和退出时序。

选择第 3 项。单页重试作为 ReaderPage 的局部状态实现，不扩展到全局缓存或远程扫描 worker。

## 远程索引打开身份

资料库与远程扫描使用 `library_index.path` 作为稳定逻辑路径；Quark、百度、115 的文件 API 需要 `remote_asset_route.provider_file_id`。阅读协调器在发起文件打开前，用 `source_id + logical_path` 优先查询当前扫描代际中通过 fingerprint/session epoch 校验的 preview 路由，再查当前书源 fingerprint 下仍对应有效索引条目的已发布路由。来源浏览器直接传入 provider 标识时，若数据库中没有相应逻辑路径映射，则原样保留；索引路径缺少有效路由时显示可读错误，不把整条逻辑路径发给云盘 API。

打开结果额外保留本次实际传给 provider 的路径，供 raw 包清理与使用同一缓存键的本地工具使用。阅读记录、元数据、远程目录 manifest 和 use lease 继续使用原逻辑路径。统一的解析入口也供封面读取、封面编辑和 AI 后台处理复用。

AI 阅读状态由 ReaderPage 以唯一 owner 注册。卸载时在 post-frame 按 owner 清理；旧页迟到的清理不会覆盖新页的注册，也不会在树锁定期间同步通知监听器。

## 条漫远距离跳页与有界页面读取

`ListView.builder` 的 child builder 可能在远距离像素跳转时被调用多次，以便布局目标之前的可变高度子项。这个布局过程不能等同于用户预读：读取请求仅由一个以显式跳转目标或当前视口页为中心、半径由 `webtoonPageLoadRadius` 统一定义的窗口触发。窗口外页面用占位项完成布局，不调用 Rust `book_page`。

`WebtoonNavigationModel` 的未知页估值同时作为 `_scrollWebtoonToPage` 的偏移输入和未加载子项的占位高度。`measure` 只接收实际图片高度；占位框尺寸仍可进入锚点守护的渲染高度记录，但不得污染未知页均值。显式跳转在第一帧布局完成前保留目标 generation，期间忽略程序化滚动自身产生的结束通知。用户手势或后续跳转使旧 generation 失效。

Rust 的 `BlockingRequestGovernor` 仍保留有界队列作为跨来源背压；UI 不通过放大队列容量来掩盖过度请求。

## 可见页优先级与渐进首显

Paged 阅读只向 Flutter–Rust bridge 提交当前可见页/跨页。条漫显式跳转时先提交目标页；目标页字节返回并完成布局后，再排入附近有限窗口。邻页使用独立 `book_page_prefetch` API，沿用 `RequestPriority::Prefetch`，并且不在 Rust 侧递归展开下一圈预取。目标及当前跨页使用现有 `book_page` 前台 API。

Rust 预取在取得 governor 许可前不认领页面 `inflight`。这样同页低优先级任务仍在 governor 等待时，后来到达的前台目标可先取得许可；已经开始的同步网络读取不可取消，仍由同页等待者共享结果。Rust 保留有限 governor 和跨优先级请求门控，不增加总并发上限。

Flutter 在当前页面进入视口时，若已有同页 Dart 预取任务正在等待，则提升该页的 request generation 并排入前台请求。旧预取结果因 generation 过期而忽略。Dart 常规页读取最多 3 个；槽位已被旧页面占用时，至多允许 1 个最新可见目标越过 Dart 预算进入 Rust 优先级队列，Dart 总读取调用上限为 4。邻页预取受常规并发预算约束；Rust 远程读取仍遵守现有 governor 上限。

页面主字节返回后先提交原图并完成首屏就绪；AI 缓存查询在后台 best-effort 执行，命中后才替换显示字节。页内加载标记仅关注当前单页或当前跨页。诊断只为前台页记录 `page_load` 毫秒数与成功/失败结果，不记录页码和资源身份。

## 验证

- Rust compile/FRB codegen/Flutter analyze 检查数据流接口和生成绑定。
- 按验收标准检查本地打开、远程 stream/download/fallback、失败重试、晚到成功句柄清理、首屏和普通分页失败、快速翻页控制器释放。
- 本轮不新增或运行自动化测试，除非用户另行要求；验证执行遵从当前代理指令。
