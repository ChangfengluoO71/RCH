# 审阅记录与资料

审阅日期：2026-10-28 至 2026-10-29；仅本地审阅和无真实服务写入的定向测试。

## 115

- [115 官方 Linux 客户端发布说明](https://115.com/115/T979690.html?reverse=0) 明确区分 BT 种子解析/云下载与普通文件上传；它证明产品能力，不证明第三方 API 权限或字段。
- [OpenListTeam 115 SDK offline.go](https://github.com/OpenListTeam/115-sdk-go/blob/main/offline.go) 是该 SDK 作者的实现：Open API 包含 `ParseTorrent` 和 `AddOfflineTaskBT`，参数各自包括 `torrent_sha1`/`pick_code` 与 `info_hash`/`wanted` 等；[SDK const.go](https://github.com/OpenListTeam/115-sdk-go/blob/main/const.go) 列出对应路径。这是第三方实现证据，不是 115 官方接口文档或授权保证；它使用 Open API，与项目当前 Cookie Web 路线不同。
- SDK 所链的 115 官方语雀页：[种子解析](https://www.yuque.com/115yun/open/evez3u50cemoict1)、[BT 任务](https://www.yuque.com/115yun/open/svfe4unlhayvluly) 在本次研究环境无法打开，因此本次不能核实权限、详细字段和最新契约。
- [BitTorrent BEP 3](https://www.bittorrent.org/beps/bep_0003.html) 定义 v1 Infohash 为种子原始 `info` 编码的 SHA-1，和整个 Torrent 文件的 SHA-1 不同。
- 2026-10-29 补充：[p115client 的客户端接口资料](https://p115client.readthedocs.io/en/latest/reference/module/client.html) 描述 Cookie/Web 的 `clouddownload_task_add_bt` 可按 Infohash 提交，也可使用 `wanted` 选择文件；它将此描述为与按该 Infohash 提交磁力链接相近。该资料是第三方实现说明，不证明 115 对任意新种子都能获取元数据。[BEP 9](https://www.bittorrent.org/beps/bep_0009.html) 的磁力链接格式允许包含 Tracker，但不能据此推断 115 一定使用它。
- 2026-10-29 目录核对：上述 p115client 文档将 `wp_path_id` 描述为目标目录 ID，`savepath` 为其下的相对路径。项目 BT 提交在 `cloud115_offline.rs:1148` 传入已选择的目标 ID 和空 `savepath`，`ssp_bt_payload_uses_current_encrypted_contract_fields` 定向测试通过；但查询仅按 Infohash 匹配，返回的目标 ID 来自入参而非远端结果。定向执行 `cargo test --locked --offline --lib source::cloud115::offline::tests::local_torrent_is_deleted_only_after_same_account_remote_task_is_confirmed -- --exact --test-threads=1`：1 passed；对应 mock 的远端任务不含目录字段，测试仍删除了隔离临时种子，说明现有清理判断未证明目标目录。此次未连接真实 115。
- 本地 `p0-gate.md` 的 2026-10-13 记录曾观察到一个 115 BT 任务完成，另一次提交被拒绝且只读核对未能确定任务存在；不是本次用户运行的证据。`implement.md` 的 2026-10-17 条目确认用户曾要求移除代码级 Daily P0 gate，旧 checkpoint 已过时。

## Android 与 Flutter

- [Android WorkManager 调度](https://developer.android.com/develop/background-work/background-tasks/persistent)：持久调度支持约束与重启，但执行时间受系统控制。
- [Android 周期工作](https://developer.android.com/develop/background-work/background-tasks/persistent/getting-started/define-work)：周期运行是弹性时间窗，不等价于固定时刻执行。
- [Android 长运行 Worker](https://developer.android.com/develop/background-work/background-tasks/persistent/how-to/long-running)：长任务涉及前台通知，Android 16 也有任务配额问题；[前台服务超时](https://developer.android.com/develop/background-work/services/fgs/timeout) 说明 Android 15 的相应限制。
- [Flutter 后台处理](https://docs.flutter.dev/packages-and-plugins/background-processes) 和 [isolate 文档](https://docs.flutter.dev/perf/isolates) 指出后台执行和插件初始化的边界；Dart isolate 不共享全局变量。

## 本次执行的定向验证

- `flutter test --no-pub -r expanded test/cloud115_bt_batch_test.dart test/cloud115_bt_daily_workflow_test.dart test/eh_daily_coordinator_test.dart test/eh_subscription_daily_store_test.dart test/tablet_layout_test.dart`：49 passed。
- `cargo test --locked --offline --lib source::cloud115::offline::tests -- --test-threads=1`：32 passed，518 filtered out。
- `cargo build --locked --offline --lib` 后，用一次性 Flutter FFI 测试读取当前 Rust DLL 和隔离临时规则文件：1 passed；确认 `cloud_stage` 往返时丢失、`partial` 更新被拒绝、`completed` 后第二次终态更新被拒绝。测试脚本和临时规则文件已移除。
- 其余测试主要为隔离数据与模拟响应；没有连接实际 115/EH、没有发送真实写入，也没有 Android 设备验收。这些结果不能证明用户此次 115 失败的具体阶段或真实接口兼容性。
