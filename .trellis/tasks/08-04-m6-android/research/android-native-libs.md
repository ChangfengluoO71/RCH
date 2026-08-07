# 安卓原生库调研(PDF / RAR)

## PDF:pdfium-render

- 现状:`app/rust/src/document/pdf.rs` 用 `Pdfium::new(Pdfium::bind_to_library(Pdfium::pdfium_platform_library_name_at_path("./")))`,失败再试系统库;Windows 依赖同目录 pdfium.dll。
- Android 方案:把 `libpdfium.so` 放入 `android/app/src/main/jniLibs/{abi}/`,运行时通过 `ApplicationInfo.nativeLibraryDir`(Dart 侧可经 method channel 或 context 获取)传入 Rust,用 `Pdfium::bind_to_library(PathBuf)` 指定绝对路径。
- libpdfium.so 来源候选:pdfium-android(Maven 制品,各 ABI 齐全)或自编译 pdfium。待 p3 子任务验证。

## RAR:unrar

- 现状:`Cargo.toml` 依赖 `unrar = "0.5.8"`;Cargo.lock 显示其依赖 `unrar_sys 0.5.8`,通过 `cc` 编译 unrar C++ 源码(带 `winapi` 依赖,仅 Windows 侧)。
- Android 可行性:unrar_sys 用 cc + libc,NDK clang 交叉编译大概率可行,但需在 p3 子任务先做最小验证。
- 备选(若 NDK 编译失败):
  1. unrar crate 的静态编译特性(需确认 0.5.8 是否支持);
  2. 纯 Rust `rar` crate(仅 RAR4,分卷支持有限);
  3. 首版暂不支持 CBR(明确告知用户,后续再补)。

## AI 超分(不在首版)

- `app/rust/src/ai/mod.rs` 调用 `realesrgan-ncnn-vulkan.exe`(Windows 专属,`#[cfg(windows)]` 处理进程创建)。Android 上无此路径;M2 Phase 3(ONNX Runtime)未落地前,首版隐藏 AI 入口。
