# Android 远程 PDF 阅读闪退调查与修复

## Goal

修复 Android 端阅读远程 PDF 文件时可能发生的闪退，并证明根因与修复有效。该问题属于已发布 Android 版本的维护缺陷，不重新打开 M6 Android 里程碑。

## Current symptom

- 平台：Android。
- 场景：通过远程书源打开 PDF 并进入阅读。
- 现象：可能发生闪退。
- 当前尚未确认：是否只影响流式读取、是否与特定远程书源有关、是否与 PDFium/native 解码、临时文件生命周期、缓存路径、内存压力或文件完整性有关。

## Investigation requirements

- [ ] 记录稳定复现步骤：远程书源类型、打开策略（stream/download/auto）、PDF 大小/页数、设备/Android 版本。
- [ ] 捕获完整 Android 崩溃证据：`adb logcat` / tombstone / Flutter/Rust/native stack trace，不能只依据 UI 闪退现象猜测。
- [ ] 对照至少一个正常路径：本地 PDF、远程下载后 PDF、或另一种远程来源，定位故障边界。
- [ ] 沿调用链追踪远程文件 → 缓存/临时文件 → PDF 打开 → 页面渲染，确认数据与文件生命周期在哪一层失效。
- [ ] 检查近期涉及 Android PDF、远程流式读取、缓存/临时文件、PDFium/native library 的变更。

## Root-cause gate

在形成单一、可证伪的根因假设前，不提交修复。根因记录必须说明：为什么远程 PDF 会触发，而正常对照路径不会。

## Fix acceptance criteria

- [ ] 在修复前有一个能稳定失败的最小复现或自动化/集成测试。
- [ ] 只针对已确认根因实施最小修复，不顺带重构无关代码。
- [ ] 原始闪退场景在同一设备/同一 PDF/同一远程来源下不再崩溃。
- [ ] 本地 PDF 阅读回归通过。
- [ ] 远程 PDF 的 stream/download/auto 中与问题相关的策略完成回归。
- [ ] Rust 测试通过；Android 构建通过；若 Flutter 工具链可正常输出，再执行 `flutter analyze` / 相关测试。

## Notes

M6 Android 适配已按已发布里程碑关闭。今后的 Android 缺陷作为独立维护任务追踪。
