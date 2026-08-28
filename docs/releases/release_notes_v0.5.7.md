# RCH v0.5.7 — Draft

> 状态：待发布。正式打 tag 前必须先解决 Android versionCode 单调递增策略。

## Improved

- 显著缩短 Android 冷启动关键路径：首页首帧不再等待目录快照、资料库树、同步管理器、AI 队列恢复和自动同步/刮削流程完成。
- `LibraryStore` 启动加载不再等待 JSON 备份的 800 ms 防抖保存；备份仍保留并在首帧后异步触发。
- 自动同步、刮削和 AI 恢复功能语义保持不变，只调整到首帧后执行，其中 `AutomationCoordinator` 最后启动，避免重任务阻塞进入首页。

## Verification

同一 Android 真机、同一应用数据环境，使用 `adb shell am force-stop` + `adb shell am start -W` 各进行 10 次进程冷启动：

- 冷启动 `WaitTime` 平均：约 **10093.7 ms → 349.3 ms**（约 **-96.5%**）
- 冷启动中位数：**10090 ms → 331 ms**
- 冷启动 P95：**10141 ms → 518 ms**
- 暖启动 `WaitTime` 平均：**92.1 ms → 28.4 ms**
- 优化版冷启动 `TotalTime`：平均 **341.3 ms**，中位数 **323 ms**，P95 **506 ms**

合并后主干 CI：Rust Test、Flutter Analyze、Android Build、Windows Build 全部通过。

完整证据见 `docs/project/STARTUP_PERFORMANCE_2026-08-28.md`。

## Release blocker — Android versionCode

真机基线安装版本为 `versionCode=2506`，本次无损覆盖测试临时使用正式签名 `versionCode=2507`。当前 release workflow 对 `v0.5.7` 会按版本号公式生成 `507`，至少无法覆盖已经安装 `2507` 的测试设备。

正式发布 v0.5.7 前需要先冻结新的 versionCode 规则，确保后续正式 APK 始终单调递增；在此之前不要创建 v0.5.7 tag。
