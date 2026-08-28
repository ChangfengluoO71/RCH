# RCH v0.5.6

## 修复

- 修复 Android 端打开远程多页 PDF 时，PDFium 并发访问可能触发原生 `SIGSEGV` 闪退的问题；所有 PDFium FFI 调用现通过进程级互斥门串行化。
- 修复阅读器 L1 缓存命中后触发预取时可能发生自锁，导致非 PDF 漫画出现大量页面同时转圈、后续页面无法继续加载的问题。
- 修复超长 PDF 页面渲染后超过 WebP 16383 像素单边限制、页面持续转圈的问题；普通页面仍保持 1600px 目标宽度，超长页面按比例缩放到安全尺寸。

## 验证

- 原始百度网盘 4 页 PDF 在同一 Android 设备上回归通过。
- 本地多页 PDF 回归通过。
- 触发过批量转圈的非 PDF 漫画回归通过。
- Rust 全量串行测试、`cargo check`、Android arm64 Release 核心构建与正式 APK 签名验证通过。

完整变更记录见 [CHANGELOG](../project/CHANGELOG.md)。
