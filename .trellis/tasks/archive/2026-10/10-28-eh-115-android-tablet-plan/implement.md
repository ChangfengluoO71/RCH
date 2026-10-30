# 实施与验证状态

已完成代码层实现：115 任务直接提交及目标目录核验、Android 可选每日工作流和平板独立模式。Rust 离线检查、Rust 格式检查、Flutter 静态分析、Windows release 代码生成构建、Android APK 构建及实现阶段的针对性测试已通过。Android release 首启时发现 R8 裁掉 WorkManager Room 构造器，已在 `app/android/app/proguard-rules.pro` 增加 keep 规则；版本 `0.6.7+104604` 已覆盖安装到手机，WorkManager 初始化不再崩溃。

外部验收已完成：用户确认 115 工作流与设备验收验证成功。MuMuPlayer 15 / Android 15 已完成 release 安装（`0.6.7+104604`），已启用“平板风格”，并在重启后确认设置持久化与横屏双栏布局。实现与运行检查记录如下。

## 1. 明确 115 故障阶段并修复状态

1. 核对用户实际运行的构建版本和 115 选中书源类型。读取现有 UI/账本中的脱敏状态，定位是否 `upload_parse_pending`、`upload_outcome_unknown`、`rejected` 或 `unknown_remote_state`；不重新上传已有未知结果。
   同时优先验证本地预检文件索引 + 现有 `bt_submit_once` 的直接提交可行性：检查 115 是否在从未上传对应 Torrent 的情况下创建任务、获取元数据并按预期下载。真实可行前不宣称直接路线已实现；不确定的写入不自动切换到上传路线重发。
2. 在 `app/rust/src/source/cloud115_offline.rs` 将上传、解析、提交的已知业务失败细分，保留受限安全错误码；针对真实脱敏响应形态补合同测试。逐一核对 Web Cookie 请求方法、响应字段与加密封装，不把 Open API 响应直接套用到 Web 接口。
3. 在 `app/lib/store/cloud115_bt_batch.dart` 和 `cloud115_bt_daily_workflow.dart` 区分受理、失败、未知和已处理；全拒绝与部分失败必须是非成功结果，恢复不重放未知写入，保留原账本兼容。
4. 在 Rust `EhAutomationRun` 与 Dart `EhDailyRunRecord` 之间建立一致、向后兼容的阶段/状态 schema；修复手动与自动工作流的终态迁移。用真实 Rust FFI 测试 `partial`、`cloud_stage`、重启读回、旧 JSON 兼容和手动云阶段追加；不要只用注入式假 writer。
5. 在 `app/lib/ui/eh_subscription_panel.dart` 分别显示上传、解析、BT 已受理、115 下载状态和 RCH 发现状态；包含失败/待核对数量及安全下一步。
6. 用现有授权会话、一个可控 Torrent 和选定目录验证：本地校验 → 上传/复用 → 115 解析 → 任务列表匹配 → 115 下载完成 → 目录内容 → RCH 读取。记录脱敏证据；如接口拒绝或权限不具备，保留明确未完成状态。
   同时验证直接 BT 路线的请求含所选 `wp_path_id`、实际内容位于所选目录或其子目录。修正 `read_task_status` 只按 Infohash 匹配并回填本地目标 ID 的问题；针对“同哈希但不同目录”“远端目录缺失/未知”测试不能误判或删除本地种子。
7. 如果评估官方开放平台适配器，先核对 115 当前官方文档和授权范围；以独立 OAuth 书源接入并测试，禁止在用户不知情的情况下从 Cookie 切换账号/协议。避免为第三方 SDK 字段差异臆测 Web 修复。

## 2. Android 自动每日工作流

1. 抽取不依赖 Widget 的共享工作流执行入口，处理规则路径、清单、云端适配器和状态存取；前台和后台都调用它。评估 headless Flutter engine 与原生 Worker 桥接；选定当前支持的工作管理依赖并核对 Android 版本。
2. 修复 `app/rust/src/eh_subscription.rs` 非 Windows 空 collection lease，以真实的跨进程/跨 isolate 互斥保护一整轮；建立独立于最近手动结果的自动 run ID、每日新运行资格、阶段检查点和死亡恢复语义。验证同进程多 isolate、不同进程、前后台同时启动、中断后恢复，以及等待→手动运行→冷启动→续跑和跨日本地日期处理。
3. Android 注册唯一周期 WorkManager，建议 6 小时只读资格检查，按本地日历最多创建一轮；应用启动/恢复时也触发同一资格检查。关闭开关取消未来调度；重启、时区变化、回拨、错过多日以及网络和电量限制都要有明确行为。
4. 初始化 Rust/FRB、所需插件和数据路径，复用无 UI 启动逻辑先恢复自定义缓存根、再打开数据库和书源，并安全恢复既有书源会话；不能依赖前台 isolate 内的会话 map。将 `MainActivity.kt` 的凭据及后台所需非交互存储通道改为 headless engine 也能注册，并修复 `CredentialVault.kt` 多实例共用 `.part` 的跨引擎/进程读改写竞争、书源状态更新冲突。测试自定义缓存根下的前台/后台冷启动及并发凭据更新；处理凭据不可读/过期、目录授权失效、网络不可用；有限重试必须复用同一轮与现有账本，不重复未知写入。
5. 加入必要的阶段检查点和停止信号；超过普通 Worker 期限时分段处理或遵循 Android 前台服务通知/类型/限制。将调度、等待、失败和最近结果接入设置页。
6. 真机覆盖：首次开启、前台/后台、应用被系统回收、设备重启、强行停止后重新打开、断网/恢复、切换时区、目录权限失效、115 会话过期和前后台竞争；验证不会产生重复 BT 请求。

## 3. 独立平板模式

1. 扩展 `app/lib/store/models.dart` 中布局与平板材质持久化字段，兼容旧值与未知值，保留 Windows 独立配置。
2. 重构 `app/lib/ui/common.dart`、`home_page.dart` 和 `reader_page.dart` 的有效模式、断点及方向判断；统一首页与阅读器退出的方向协调，新增 `tablet` 选择，窄窗口可紧凑显示但不覆盖模式或误锁方向。
3. 解耦 `app/lib/ui/rch_overlay.dart` 的 Flutter 模糊能力与 Windows 云母能力；Android 平板提供标准/磨砂的有效材质选项与预览；限制模糊区域、保证可读性和高对比度回退。
4. 覆盖 Android 平板横竖屏/分屏、窄窗口进入并退出阅读器、手机、桌面窗口、旧配置迁移、重启持久化、动态配色、深浅主题、Windows 云母/亚克力回归和真实设备性能。

## 4. 质量门槛

- Rust：现有 115 合同测试，加上真实响应的脱敏 fixture、状态迁移、Android 互斥和 FFI 集成测试；执行相应 `cargo test`、`cargo check` 和格式检查。
- Flutter：相关 batch、daily、panel、tablet、material 测试和静态分析；若 Rust 对外 API 变化，按项目流程重新生成 FRB 并检查生成差异。
- Android：构建对应 APK、至少一台手机和一台平板/等效宽屏真机执行生命周期测试；系统调度仅验证可观察的尽力运行和状态，不宣称精确时间。
- 单独报告 mock、FFI、真实 115 账号、Android 真机和 RCH 读取结果；任何一项没做就标明未验证。
