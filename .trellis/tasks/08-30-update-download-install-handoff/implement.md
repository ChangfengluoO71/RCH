# 应用内更新下载与安装交接：实施计划

## 同批执行边界

本任务与 `08-02-cover-loading-perf` 组成 `cover-and-update` 实施批次。两项可以使用同一工作分支、同一全量验证轮次，但不共享状态机、平台通道、测试替身或提交；本任务的下载互斥、平台安装交接和 Windows/Android 冒烟仍必须独立完成。

## 步骤 1：锁定状态机与测试替身

- [ ] 为 `UpdateManager` 的下载互斥、完成后一次性安装交接、失败恢复写单测。
- [ ] 提取 Windows/Android 安装启动器接口，使用 fake 记录调用次数、路径和返回结果。
- [ ] 覆盖 UAC/启动失败、Android 未知来源权限和用户关闭进度视图的状态恢复。

## 步骤 2：下载互斥与平台交接

- [ ] 让 `download()` 或用户确认入口持有同一进行中 Future/操作标识，确保并发点击不会重复写文件。
- [ ] 下载和大小校验成功后，由经用户确认的链路调用一次安装启动器。
- [ ] Windows 使用安全参数化启动；Android 保留 APK 并将系统确认结果展示为可重试状态。

## 步骤 3：连续的更新体验

- [ ] 将更新确认弹窗转为/导航到可见下载进度视图，而非关闭后要求用户寻找设置页。
- [ ] 让下载、镜像切换、失败、已下载和交接中状态在该视图与 `UpdatePanel` 中一致。
- [ ] 保持“稍后”、静默检查、GitHub Releases 兜底和镜像选择语义不变。

## 验证命令

```powershell
cd app
flutter test test/update_manager_test.dart test/update_mirror_test.dart
flutter test
flutter analyze
```

## Execution update (2026-09-11)

- [x] Update downloads stream to a `.part` file and atomically commit after completion; an existing verified package is preserved for retry and is not destructively re-downloaded.
- [x] Windows handoff uses parameterized process arguments; Android handoff preserves the APK and exposes a retryable failure state.
- [x] Concurrent requests share one download future and the verified package is handed off at most once per confirmation.
- [x] Automated verification: `flutter test --no-pub test/update_handoff_test.dart` passed (9 tests); `flutter analyze --no-pub` passed.
- [ ] Real Windows installer/UAC and Android unknown-source/system-installer smoke remain pending; keep platform completion unchecked.

## 平台关卡与回滚

- [ ] Windows：用测试安装包验证确认下载后出现进度、校验成功后只启动一次安装器、取消 UAC 后可重试。
- [ ] Android：验证确认下载后出现进度，完成后只拉起系统安装确认；拒绝未知来源权限后可回到已下载状态重试。
- [ ] 将 UI/状态机提交与平台通道提交保持可回滚边界；出现平台回归时先关闭自动交接入口，保留手动安装。
