# Android 启动性能验证 — 2026-08-28

## 结论

Android 启动优化通过真机验收。相同设备、相同应用数据环境下，进程冷启动 `WaitTime` 平均值从约 **10093.7 ms** 降至 **349.3 ms**，下降约 **96.5%**；中位数从 **10090 ms** 降至 **331 ms**。

本轮优化目标是缩短“点击图标 → 首页可进入”的关键路径，不改变同步、刮削、AI 恢复或启动备份的功能语义，只改变它们相对首帧的执行时机。

## 被测版本

- 基线：设备原已安装版本，`versionCode=2506`
- 优化代码：`master` commit `0b498fdfcac99d44ff9bdb559c8bbc4842ea567e`
- 真机覆盖测试包：正式签名临时构建，`versionCode=2507`
- 包名：`com.rch.reader`
- Activity：`com.rch.reader/.MainActivity`

## 测试方法

进程冷启动每轮执行：

```text
adb shell am force-stop com.rch.reader
adb shell am start -W -n com.rch.reader/.MainActivity
```

暖启动每轮将应用送回 Home 后重新拉起 Activity。冷启动和暖启动各重复 10 次。

注意：基线冷启动 10 次中有 9 次未返回 `TotalTime`，但 `WaitTime` 均约为 10 秒，因此跨版本主比较指标采用 `WaitTime`。优化版 10 次冷启动均成功返回 `TotalTime`。

## 原始结果

### 基线 — 冷启动 WaitTime

```text
10093, 10135, 10080, 10116, 10141, 10086, 10021, 10096, 10087, 10082 ms
```

- 平均：10093.7 ms
- 中位数：10090 ms
- P95：10141 ms

### 优化版 — 冷启动 TotalTime

```text
506, 322, 321, 312, 314, 326, 328, 318, 324, 342 ms
```

- 平均：341.3 ms
- 中位数：323 ms
- P95：506 ms

### 优化版 — 冷启动 WaitTime

```text
518, 332, 327, 322, 320, 331, 331, 322, 338, 352 ms
```

- 平均：349.3 ms
- 中位数：331 ms
- P95：518 ms

### 暖启动 WaitTime

基线：

```text
94, 87, 121, 85, 80, 91, 84, 93, 95, 91 ms
```

- 平均：92.1 ms

优化版：

```text
28, 27, 27, 23, 31, 30, 34, 29, 28, 27 ms
```

- 平均：28.4 ms

## 对比

| 指标 | 基线 | 优化后 | 改善 |
|---|---:|---:|---:|
| 冷启动 WaitTime 平均 | 10093.7 ms | 349.3 ms | -96.5% |
| 冷启动 WaitTime 中位数 | 10090 ms | 331 ms | -96.7% |
| 冷启动 WaitTime P95 | 10141 ms | 518 ms | -94.9% |
| 暖启动 WaitTime 平均 | 92.1 ms | 28.4 ms | -69.2% |

按冷启动平均 WaitTime 计算，启动约提升 **28.9 倍**。

## 实现边界

首帧前继续保留：

- Rust 初始化
- Android PDF native 配置
- cache root / DB heal 与 migration
- 首页/主题所需 `LibraryStore` 核心加载
- pending migration 检查

首帧后执行：

- `FolderSnapshotStore.instance.load()`
- `LibraryCatalogStore.instance.loadTree()`
- `SyncManager.instance.init()`
- `AiUpscaleManager.instance.init()`
- `AutomationCoordinator.instance.init()`（最后执行）

`LibraryStore` 启动时改为 `load(persist: false)`；JSON 备份仍会触发，但不再阻塞首帧等待 800 ms debounce。

## 自动化验证

合并后 `master` CI run `33157894335`：

- Rust Test：success
- Flutter Analyze：success
- Android Build：success
- Windows Build：success

同时启动顺序 source contract 已完成 RED → GREEN 验证。

## 验收结论

**PASS。** 当前 Android 冷启动已稳定进入约 0.3–0.5 秒量级，不建议在没有新的真机 profiling 证据前继续拆分 `LibraryStore` / SQLite 首帧关键路径，以免用较高竞态风险换取很小的边际收益。

## 发布版本号注意事项

本次临时正式签名测试包使用 `versionCode=2507`，仅用于覆盖设备上的 `2506` 做无损对比测试。

当前发布 workflow 对 `v0.5.7` 的公式仍会计算出 `versionCode=507`。至少对这台已安装 `2507` 的测试设备而言，`507` 无法直接覆盖安装。正式打 `v0.5.7` tag 前必须先明确 Android `versionCode` 的单调递增策略，并保证正式发布包的 versionCode 高于已分发/已安装的相关构建。不要把临时 `2507` 直接视为正式版本规则。
