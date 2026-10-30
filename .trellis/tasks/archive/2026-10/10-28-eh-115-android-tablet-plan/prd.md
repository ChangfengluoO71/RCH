# EH → 115 云下载、Android 每日工作流与平板模式

状态：已按用户确认的实施方案完成实现并验收。实现与验证记录见 `design.md`、`implement.md`。

## 目标

1. 让 EH 收集到的有效种子最终成为 115 云下载 BT 任务，并能看清每一步及失败原因。
2. 让 Android 手机和平板可以开启自动每日完整工作流，且清楚说明系统对后台执行的限制。
3. 增加独立“平板模式”，使 Android 平板既能使用适合触屏的大屏布局，也能选择该平台可实现的材质效果。

## 背景与已确认事实

- 当前批处理不是只有上传：它设计为上传 `.torrent` → 调 115 解析 → 提交 BT；解析未通过时不会进入提交。真实服务此次究竟停在哪一步尚未验证。`app/rust/src/source/cloud115_offline.rs:1039`、`app/lib/store/cloud115_bt_batch.dart:989`。
- 现有预检已能从本地种子取得 Infohash 和文件索引，Cookie BT 提交函数本身只使用本地解析结果，不要求此前已调用上传；上层批处理目前强制等待远端解析才调用它。跳过上传能否在 115 实际解析并下载给定种子，尚未实测。`app/rust/src/source/cloud115_offline.rs:619`、`:1107`。
- Rust 解析器严格要求远端 Infohash、文件数量、顺序、路径和大小与本地相同；部分异常被压成“已上传，解析待恢复”。BT 提交的业务拒绝也常被压成通用 `REJECTED`。`app/rust/src/source/cloud115_offline.rs:350`、`:1039`、`:1180`。
- BT 提交会将用户选择的 115 目录 ID 放入 `wp_path_id`；但远端任务查询只匹配 Infohash，并将请求的目标目录 ID 原样返回，未核实任务实际目录。开启本地种子清理时，其他目录中的同哈希任务也可能触发删除。`app/rust/src/source/cloud115_offline.rs:1148`、`:1391`、`:1442`、`:660`。
- 汇总将 `rejected` 算作跳过，但判断处理是否结束时没有检查 `skipped`；因此所有条目都被拒绝时也可能显示“本轮提交处理已结束”。`app/lib/store/cloud115_bt_daily_workflow.dart:185`。
- Dart 的运行记录有 `partial` 和 `cloud_stage`，Rust 的 `EhAutomationRun` 没有云阶段字段，且 `update_daily_run` 不接受 `partial`；手动扫描结束后再补写完整工作流终态也不符合现有终态迁移规则。以上三点已用当前 Rust FFI 和隔离临时规则文件验证。`app/lib/store/eh_daily_coordinator.dart:91`、`app/rust/src/eh_subscription.rs:138`、`:1702`、`app/lib/ui/eh_subscription_panel.dart:438`。
- 自动 Daily 在入口、调度器、UI 上均限于 Windows，且依靠进程内定时器。Android 非 Windows 的 EH collection lease 当前是空实现；不能简单移除平台判断就启动后台任务。`app/lib/main.dart:142`、`app/lib/store/eh_daily_coordinator.dart:260`、`app/lib/ui/eh_subscription_panel.dart:2257`、`app/rust/src/eh_subscription.rs:1559`。
- Android 凭据通道目前由 `MainActivity` 注册，后台 headless engine 不能假定也已注册；保险库的 `@Synchronized` 只保护单个实例，且共用 `.part` 文件，跨引擎读改写需要额外协调。`app/android/app/src/main/kotlin/com/rch/reader/MainActivity.kt:20`、`CredentialVault.kt:31`、`:98`。
- `tabletLayout` 只有 auto/desktop/mobile；宽度低于 600dp 永远紧凑，否则 auto/desktop 用同一桌面侧栏。Windows 云母和 Flutter 弹窗亚克力都被 Windows/Mica 能力判断限制。`app/lib/ui/common.dart:9`、`app/lib/ui/home_page.dart:2053`、`:2697`、`app/lib/ui/rch_overlay.dart:91`。
- 阅读器还会缓存打开时的紧凑布局并在退出时恢复相应方向；平板窄窗口可能因此被锁竖屏。`app/lib/ui/reader_page.dart:298`、`:2134`。
- 历史 P0 研究未证明当前 Cookie 写接口的完整线上契约；2026-10-17 用户确认曾在应用中试用后，代码级 P0 Daily 门禁已移除。不能把历史记录误当成当前运行时门禁，也不能把历史测试视为本次实测。`.trellis/tasks/10-11-eh-115-p0-p1/implement.md:111`。

## 需求与验收

### 115

- 提交结果必须可区分：本地校验、已上传、115 已解析、BT 已受理、排队/下载中、下载完成、失败、结果未知；上传文件本身不是创建 BT 任务的证据。
- 优先研究和验证直接提交 Infohash/磁力任务，避免仅为提交任务而把 `.torrent` 保存在目标目录；如果 115 不能据此获得元数据，保留原始种子解析的可解释备选路径。
- 给出可理解的阶段、计数和有限的脱敏错误；全拒绝或存在失败/未知时不能显示完整成功。
- 已上传但尚未解析的条目可安全恢复解析；写入结果未知时必须先只读核对，不盲目重传或重复提交；继续保留现有账号、Infohash、目标目录账本语义。
- 直接 BT 或磁力任务沿用用户选择的 115 目标目录；必须区分“请求中指定目录”和“远端任务/文件已实际落入该目录”。没有同账号、同哈希、真实目标目录证据时，不得因目录已确认而清理本地种子。
- 用一个可控的真实种子验证当前选中账户：115 云下载任务列表出现匹配任务，完成后目标内容可见并可通过 RCH 正常读取；mock 成功不能替代此验证。

### Android Daily

- 默认关闭，用户开启后依照选中的本地种子或 115 模式执行；前台和后台共用每日领取、状态与账本。
- 同一设备同一本地日期最多新建一轮自动运行；中断后可以安全续跑同一轮，不能重复发送结果未知的写请求。自动运行检查点不能被后续手动运行覆盖。
- 暴露等待网络、配置/目录不可用、需重新认证、中断、部分完成和最近运行结果；不承诺手机关机、被强行停止、系统限制期间按固定时刻运行。
- 前台手动运行与后台运行互斥；Android 真机需覆盖关闭界面、重启、断网、进程被回收和权限/会话失效。

### 平板模式

- 设置中提供独立 `tablet` 模式，和已有自动、桌面、手机选项共存；旧配置语义不丢失，能手动切换并跨重启保存。
- 大屏优先侧栏、触摸适配和可旋转；窗口变窄时允许布局自适应，不能仅因宽度变化丢失用户选择或错误锁定平板方向。
- Android 平板提供适用的平台无关材质选项，视觉效果有清晰标识和预览；不冒称 Windows 系统云母。高对比度和设备性能不适合时回退标准材质。
- Windows 原有云母、亚克力、主题、阅读画布及既有配置不回归。

## 范围与限制

- 第一阶段以仓库当前发布平台 Windows / Android 为目标；iOS 自动调度不包含在本方案里。
- 不绕过 115 认证、WAF 或限流；不自动清除远端种子或任务，也不重置既有账本。既有用户选择的本地清理策略继续独立处理。
- 修复 Cookie 路线前需要核实其 Web 协议和当前真实失败阶段的脱敏证据；如另选 Open API 适配器，再核实其当前官方文档及授权范围。不得把第三方 SDK 的接口模型当作官方承诺。
- 实施顺序建议：先修 115 状态和完整任务链，再接 Android Daily，最后实现平板模式；详见 `design.md`、`implement.md`。
