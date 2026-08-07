# P3 原生格式 PDF/RAR 安卓适配

## Goal

让 PDF 与 RAR / CBR 两种依赖原生库的格式在安卓上可解码、可渲染、可翻页。

## Confirmed Facts

- `pdfium-render` 当前按 进程工作目录 → exe 所在目录 → PATH → 系统目录 链式加载 pdfium 动态库(2026-08-04 修复);Windows 需 pdfium.dll;Android 需按 ABI 打包 `libpdfium.so`,并通过 nativeLibraryDir 显式传入 Rust(不能依赖 exe 目录链)。
- `unrar` 0.5.8 经 `unrar_sys` 用 `cc` 编译 unrar C++ 源码;Android NDK 交叉编译需验证。

## Requirements

- PDF:为 arm64-v8a(及后续 ABI)打包 libpdfium.so(来源待定:pdfium-android 制品或自编译),Rust `pdf.rs` 加载路径适配 Android(应用 nativeLibraryDir 或随包资源路径)。
- RAR / CBR:验证 unrar / unrar_sys 在 NDK 下可编译;失败则评估备选(static 特性 / 纯 Rust 方案),记录结论。
- 两种格式在阅读器内与现有 Document trait 无缝集成。

## Acceptance Criteria

- [ ] 真机打开 PDF 与 CBR 各一本,可渲染页面并翻页。
- [ ] 失败预案已执行:若 NDK 编译不可行,结论写入研究记录,并回退为"首版暂不支持该格式",同时明确告知用户。

## Dependencies

- 前置:p0-android-buildchain。
- 可与 p1-local-reader 并行;合入后由 p1 验收全格式阅读闭环。
