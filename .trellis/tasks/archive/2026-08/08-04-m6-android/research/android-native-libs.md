# 安卓原生库调研(PDF / RAR)

## PDF:pdfium-render

- 现状:`app/rust/src/document/pdf.rs` 用 `Pdfium::new(Pdfium::bind_to_library(Pdfium::pdfium_platform_library_name_at_path("./")))`,失败再试系统库;Windows 依赖同目录 pdfium.dll。
- Android 方案:把 `libpdfium.so` 放入 `android/app/src/main/jniLibs/{abi}/`,运行时通过 `ApplicationInfo.nativeLibraryDir`(Dart 侧可经 method channel 或 context 获取)传入 Rust,用 `Pdfium::bind_to_library(PathBuf)` 指定绝对路径。
- libpdfium.so 来源候选:pdfium-android(Maven 制品,各 ABI 齐全)或自编译 pdfium。待 p3 子任务验证。

### PDF 验证结论(2026-08-08,真机通过)

- 采用 bblanchon/pdfium-binaries `chromium/7881`(与 pdfium-render 0.9.3 的 `pdfium_latest` 匹配)预编译 android-arm64 / android-x64 的 `libpdfium.so`,放入 `jniLibs/{arm64-v8a,x86_64}/`。
- 加载链路:`MainActivity` method channel 新增 `nativeLibraryDir` → Dart `nativeLibraryDir()` → Rust `set_native_lib_dir`(新 FRB API,`api/pdf.rs`)→ `pdf.rs` 加载时优先 `bind_to_library(nativeLibraryDir/libpdfium.so)`,Windows 原有链不变。
- 依赖注意:unrar 的 C++ 代码使 `librust_lib_app.so` 引用 libc++ 符号;必须同时打包 `libc++_shared.so` 且让 cdylib 带 `DT_NEEDED`(unrar_sys build.rs 对 Android 输出 `-lc++`),否则 dlopen 报 `cannot locate symbol _ZTISt12length_error`。
- 验证:MuMu(x86_64)真机 PDF 1/1 页渲染成功、进度记录正常。

## RAR:unrar

- 现状:`Cargo.toml` 依赖 `unrar = "0.5.8"`;Cargo.lock 显示其依赖 `unrar_sys 0.5.8`,通过 `cc` 编译 unrar C++ 源码(带 `winapi` 依赖,仅 Windows 侧)。
- Android 可行性:unrar_sys 用 cc + libc,NDK clang 交叉编译大概率可行,但需在 p3 子任务先做最小验证。
- 备选(若 NDK 编译失败):
  1. unrar crate 的静态编译特性(需确认 0.5.8 是否支持);
  2. 纯 Rust `rar` crate(仅 RAR4,分卷支持有限);
  3. 首版暂不支持 CBR(明确告知用户,后续再补)。

### RAR 验证结论(2026-08-08,已验证通过)

- P0 时为保构建绿,`unrar` 在 Android 目标被隔离(`cfg(not(target_os="android"))` + `document/mod.rs` 的 rar 分支 bail)。
- 放开隔离后交叉编译唯一报错:bionic 无 `lutimes`(unrar `os.hpp` 在 `__linux` 下定义 `USE_LUTIMES`,Android 也定义 `__linux`)。
- 修复:vendored `unrar_sys/vendor/unrar/os.hpp` 的 `USE_LUTIMES` 条件加 `&& !defined(__ANDROID__)`(阅读场景不提取符号链接,无需保留链接时间戳)。
- 验证:`cargo check --target aarch64-linux-android` 通过;`flutter build apk --debug` 4 个 ABI(armv7/aarch64/x86_64/i686)全部通过。
- 运行时:`RarBook` 全可移植(写临时文件→unrar 静态链接 libunrar.a→内存读页),无系统 DLL 依赖;待真机读一本 CBR/RAR 验收。

## AI 超分(不在首版)

- `app/rust/src/ai/mod.rs` 调用 `realesrgan-ncnn-vulkan.exe`(Windows 专属,`#[cfg(windows)]` 处理进程创建)。Android 上无此路径;M2 Phase 3(ONNX Runtime)未落地前,首版隐藏 AI 入口。
