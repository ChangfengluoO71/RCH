## Goal

在阅读器中接入端侧 AI 超分功能。用户浏览漫画时，右键当前页 → 选择 "AI 超分" → 显示进度 → 低分辨率图片放大 2x 后替换当前页显示。纯本地推理，不上传图片。

## Background

- SPEC ADR-009 原定 CLI 子进程方案；技术调研确认 `realesrgan-ncnn-vulkan` CLI 只支持文件路径传参（-i/-o），不支持 stdin/stdout 交互，Phase 1 采用单次调用方案，Phase 2 再做常驻 Worker。
- 五级缓存体系 `CacheDir::Ai` 已就绪（目录创建、大小计算、清理）。
- Reader 当前返回原始压缩字节（不解码），AI 超分需要 Rust 侧先解码为像素、超分后再编码输出。
- 阅读器右键当前触发设置面板，需改为 popup 菜单为 AI 超分预留入口。

## Confirmed Facts

| 事实 | 位置 |
|---|---|
| AI 占位 UI 在设置面板中，无实际功能 | `reader_page.dart` L214-215 |
| Rust 无 `ai/` 模块 | `lib.rs` |
| `CacheDir::Ai` 缓存目录已完整集成 | `cache.rs` L56-57 |
| `interprocess` 依赖未添加 | `Cargo.toml` |
| Reader 返回 `Arc<Vec<u8>>` 原始压缩字节 | `reader.rs` L100 |
| `realesrgan-ncnn-vulkan` 仅支持 `-i <path> -o <path>` | 官方 README |
| 推荐模型 `realesr-animevideov3` 专为动画/漫画优化，支持 2x/3x/4x | 官方文档 |
| exe + 模型将放入安装程序打包分发 | 用户确认方案 A |

## Requirements (Phase 1)

### R1: 文件资产
- 将 `realesrgan-ncnn-vulkan.exe` + `models/` 放入 `app/windows/ai/`
- CMake 构建时复制到 build 输出目录，随安装程序分发

### R2: Rust `ai/` 模块
- `ai/mod.rs`：超分编排——解码页面字节 → 写临时文件 → 调 CLI → 读结果 → 编码回 JPEG
- `api/ai.rs`：FRB 桥接层，暴露 `super_resolve(page_bytes: Vec<u8>, scale: u32) -> Result<Vec<u8>>`
- 进程超时 60s，超时自动 kill
- 运行时定位 exe：`std::env::current_exe()` 同级目录查找

### R3: 超分缓存
- 超分结果写入 `CacheDir::Ai` 缓存目录
- 缓存 key：`sha256(page_bytes)_<模型名>_<倍率>.ai`
- 重复请求命中缓存直接返回，不启动进程

### R4: Dart 侧右键菜单
- 右键阅读页面 → popup 菜单："阅读设置" / "AI 超分 (2x)"
- "AI 超分"触发：取当前页 `_bytes[_page]` → 调 `superResolve()` → 替换显示
- 显示进度 SnackBar："AI 超分处理中..." → 完成后刷新

## Acceptance Criteria

- [x] `realesrgan-ncnn-vulkan.exe` + 模型文件可独立运行并输出超分图片
- [x] `app/windows/ai/` 目录含 exe + models，CMake 构建后出现在 Release 目录
- [x] `super_resolve()` 端到端：输入 JPEG/PNG/WebP 字节 → 返回超分后 JPEG 字节
- [x] 进程超时或崩溃不拖崩主程序，返回错误信息
- [x] exe 未找到时返回明确错误消息
- [x] 相同输入二次请求走缓存，不重复启动进程
- [x] 阅读器右键菜单含"AI 超分 (2x)"选项
- [x] 超分期间显示 SnackBar 进度，完成后自动刷新页面
- [x] `cargo test --lib ai` 通过
- [x] `flutter analyze` 0 issues

## Out of Scope (Phase 1)

- 常驻 Worker 进程（Phase 2 — CLI 目录批量模式已完成）
- 共享内存传输（Phase 2 — 转为 CLI 目录批量模式）
- `Upscaler` trait 多模型切换（Phase 3）
- 批量超分 / 整本超分（Phase 2 已完成 `super_resolve_batch()`）
- ONNX Runtime 后端（ort crate 无法在 FRB cdylib 中编译，模型已转 ONNX 待 ort 稳定后切换）
- macOS / Android 支持

## Architecture Decisions

| 决策 | 结论 | 理由 |
|---|---|---|
| 通信方式 | `std::process::Command` 单次调用 | CLI 不支持 stdin/stdout，Phase1 接受每次 ~2s 模型加载开销 |
| 数据传递 | 临时文件写入 `CacheDir::Temp` | CLI 只接受文件路径 |
| 模型 | `realesr-animevideov3-x2` | 专为动画/漫画优化，2x 是漫画超分最佳倍率 |
| 分发 | 随安装程序打包 | 本地优先、离线可用 |
| 超时 | 60s | Vulkan GPU 推理通常 2-5s，60s 给予足够余量 |
