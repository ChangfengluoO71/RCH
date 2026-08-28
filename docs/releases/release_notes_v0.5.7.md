# RCH v0.5.7

## Improved

- 显著缩短 Android 冷启动关键路径：首页首帧不再等待目录快照、资料库树、同步管理器、AI 队列恢复和自动同步/刮削流程完成。
- `LibraryStore` 启动加载不再等待 JSON 备份的 800 ms 防抖保存；备份仍保留并在首帧后异步触发。
- 自动同步、刮削和 AI 恢复功能语义保持不变，只调整到首帧后执行，其中 `AutomationCoordinator` 最后启动，避免重任务阻塞进入首页。
- Android 正式发布版采用新的单调递增 `versionCode` 规则：`100000 + (major × 10000 + minor × 100 + patch)`；v0.5.7 对应 `100507`，可直接覆盖此前用于真机测试的正式签名 `2507` 构建。

## Verification

同一 Android 真机、同一应用数据环境，使用 `adb shell am force-stop` + `adb shell am start -W` 各进行 10 次进程冷启动：

- 冷启动 `WaitTime` 平均：约 **10093.7 ms → 349.3 ms**（约 **-96.5%**）
- 冷启动中位数：**10090 ms → 331 ms**
- 冷启动 P95：**10141 ms → 518 ms**
- 暖启动 `WaitTime` 平均：**92.1 ms → 28.4 ms**
- 优化版冷启动 `TotalTime`：平均 **341.3 ms**，中位数 **323 ms**，P95 **506 ms**

合并后主干 CI：Rust Test、Flutter Analyze、Android Build、Windows Build 全部通过。

完整证据见 `docs/project/STARTUP_PERFORMANCE_2026-08-28.md`。

## Android upgrade compatibility

- 历史真机基线：`versionCode=2506`
- 启动性能正式签名测试包：`versionCode=2507`
- v0.5.7 正式包：`versionCode=100507`

因此 v0.5.7 可以直接覆盖安装在上述测试包之上，后续版本也会沿同一规则继续递增。
