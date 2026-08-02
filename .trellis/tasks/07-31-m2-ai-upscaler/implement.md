# M2 AI 超分 — 实施计划 (implement.md)

## 遗留 BUG 修复（2026-08-01 已完成）

- [x] 进程超时：`run_cli()` 60s 超时 + kill（管道后台排空防阻塞）
- [x] Windows 黑窗：`CREATE_NO_WINDOW`（cfg(windows)）
- [x] 并发临时文件冲突：文件名带全局递增序号；失败路径清理完整
- [x] 取消 AI 超分误删全部缓存 → 改为按页 hash 只清本书（`delete_ai_cache_for_page`）
- [x] 整本超分接入批量 API：每 20 页一次 `superResolveBatch`（内存有界、进度按块更新）
- [x] 批量结果与输入对齐（失败页返回空 Vec）；单页解码失败不再拖垮整批
- [x] 缓存写入原子化（tmp + rename）+ 损坏缓存读时自愈删除
- [x] JPEG 质量指定 90
- [x] 缓存 key 使用 MODEL_NAME（换模型不串缓存）
- [x] 文档/注释 4x → 2x 一致性（README、api/ai.rs）
- [x] 移除打包中的 ONNX 残留（x4.onnx / .data）
- [x] 阅读器右键超分防重入（_aiProcessing）

> 关联：`ea0916b` 标签归一化修复了 AI 超分后的"数据保存失败"；`70a5b5a` 缓存根迁移修复与本任务无冲突。

## 步骤概览

按顺序执行，每步完成后验证。

---

### 第 1 步：获取 AI 引擎文件

从 GitHub Release 下载 `realesrgan-ncnn-vulkan` Windows 预编译包 + 模型文件。

```bash
# 下载 exe + models (v0.2.5.0)
# 需要手动从以下链接下载:
# https://github.com/xinntao/Real-ESRGAN/releases/download/v0.2.5.0/realesrgan-ncnn-vulkan-20220424-windows.zip
```

解压后放入 `app/windows/ai/`:
```
app/windows/ai/
├── realesrgan-ncnn-vulkan.exe
└── models/
    ├── realesr-animevideov3-x2.bin
    └── realesr-animevideov3-x2.param
```

验证：在终端运行 `realesrgan-ncnn-vulkan.exe -i test.png -o test_out.png -s 2 -n realesr-animevideov3` 确认输出正常。

---

### 第 2 步：添加 Rust 依赖

编辑 `app/rust/Cargo.toml`，添加 `sha2` 依赖：

```toml
sha2 = "0.10"
```

然后运行 `cargo check` 确认依赖解析正常。

---

### 第 3 步：实现 Rust `ai/mod.rs` — 超分核心

新文件 `app/rust/src/ai/mod.rs`：

1. `ai_exe_path()` — 定位 exe 路径
2. `super_resolve(bytes: &[u8], scale: u32) -> Result<Vec<u8>>`
   - sha256(bytes) → 构造缓存 key
   - 查 `CacheDir::Ai` 缓存 → 命中直接返回
   - 解码 bytes → 写 temp/ 临时 PNG
   - `std::process::Command` 调 CLI
   - 等待 exit，超时 kill（60s）
   - 读结果 PNG → 解码 → 编码 JPEG (quality 85)
   - 写 `CacheDir::Ai` 缓存
   - 清理临时文件
   - 返回 JPEG bytes
3. 辅助函数：`compute_hash(bytes) -> String`

验证：`cargo check --lib` ✅

---

### 第 4 步：注册 ai 模块

编辑 `app/rust/src/lib.rs`：
```rust
pub mod ai;
```

编辑 `app/rust/src/api/mod.rs`：
```rust
pub mod ai;
```

验证：`cargo check` ✅

---

### 第 5 步：实现 `api/ai.rs` — FRB 桥接

新文件 `app/rust/src/api/ai.rs`：

```rust
#[flutter_rust_bridge::frb(sync)]
pub fn super_resolve(page_bytes: Vec<u8>, scale: u32) -> Result<Vec<u8>> {
    crate::ai::super_resolve(&page_bytes, scale)
        .map_err(|e| anyhow::anyhow!("{:#}", e))
}
```

注意：FRB 需要 `pub fn` + 适当的返回类型。如果 FRB 不支持同步返回 `Result`，可能需要 `Tokio::spawn_blocking` 包装。

---

### 第 6 步：运行 FRB codegen

```bash
cd app
flutter_rust_bridge_codegen generate
```

验证：
- `app/lib/src/rust/api/ai.dart` 生成成功
- `flutter analyze` ✅ 0 issues

---

### 第 7 步：Dart 侧 — 阅读器右键菜单

编辑 `app/lib/ui/reader_page.dart`：

1. 导入 `ai.dart`: `import 'package:app/src/rust/api/ai.dart';`
2. 添加 state 变量 `bool _aiProcessing = false;`
3. 修改 `onSecondaryTapUp` 从直接 `_showSettings()` 改为弹出 `showMenu`:
```dart
onSecondaryTapUp: (details) {
  showMenu(
    position: RelativeRect.fromLTRB(details.globalPosition.dx, details.globalPosition.dy, 0, 0),
    context: context,
    items: [
      PopupMenuItem(child: Text('阅读设置'), value: 'settings'),
      PopupMenuItem(child: Text('AI 超分 (2x)'), value: 'ai'),
    ],
  ).then((value) {
    if (value == 'settings') _showSettings();
    if (value == 'ai') _doAiSuperResolve();
  });
}
```
4. 实现 `_doAiSuperResolve()`:
```dart
Future<void> _doAiSuperResolve() async {
  final bytes = _bytes[_page];
  if (bytes == null) {
    ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text('当前页尚未加载')));
    return;
  }
  setState(() => _aiProcessing = true);
  ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text('AI 超分处理中...'), duration: Duration(seconds: 2)));
  try {
    final result = await superResolve(pageBytes: bytes, scale: 2);
    if (!mounted) return;
    setState(() { _bytes[_page] = result; });
    ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text('AI 超分完成 ✓')));
  } catch (e) {
    if (!mounted) return;
    ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text('AI 超分失败: $e'), duration: Duration(seconds: 3)));
  } finally {
    if (mounted) setState(() => _aiProcessing = false);
  }
}
```

5. 移除设置面板中的 AI 占位卡片 (L214-215)，替换为：
```dart
// Phase 1 已上线，右键菜单直接触发 AI 超分
```
或直接保留一行简洁说明。

---

### 第 8 步：CMake 集成 — 复制 AI 文件

编辑 `app/windows/CMakeLists.txt`，在 `install(FILES "${AOT_LIBRARY}" ...)` 之后、文件末尾之前，新增一段。实际文件中第 106-108 行 `install(FILES "${AOT_LIBRARY}"...)` 之后即为合适位置：

```cmake
# 复制 AI 超分引擎文件到数据目录
set(AI_SOURCE_DIR "${CMAKE_CURRENT_SOURCE_DIR}/ai")
if(EXISTS "${AI_SOURCE_DIR}")
  install(DIRECTORY "${AI_SOURCE_DIR}/"
    DESTINATION "${INSTALL_BUNDLE_DATA_DIR}/ai"
    COMPONENT Runtime
  )
endif()
```

因为 CMakeLists.txt 使用 `install()` 机制（非 `add_custom_command`），所以统一用 `install(DIRECTORY ...)` 把 `app/windows/ai/` 的内容安装到 `data/ai/`。Rust 侧定位为：`current_exe.parent / "data" / "ai" / "realesrgan-ncnn-vulkan.exe"`。

这样 `flutter build windows` 后 Release 目录自动包含 AI 引擎文件。

---

### 第 9 步：构建与验证

```bash
cd app

# 1. Rust 编译
cd rust && cargo check --lib
# 预期: 0 errors

# 2. 单元测试 (如果有测试页面)
cargo test --lib ai
# 预期: 通过

# 3. Flutter 静态分析
cd .. && flutter analyze
# 预期: 0 issues

# 4. 完整构建
flutter build windows
# 预期: 成功，data/ai/ 目录出现在 Release 目录

# 5. 启动验证
flutter run -d windows
# 打开一本漫画 → 右键 → 选 "AI 超分 (2x)" → 等待处理 → 页面刷新为高清
```

---

## 回滚点

| 步骤 | 回滚方式 |
|---|---|
| 第 2 步后 | `git checkout -- app/rust/Cargo.toml` |
| 第 3-5 步后 | 删除 `ai/` 目录，撤销 `lib.rs`/`mod.rs` 的 `pub mod ai` |
| 第 7 步后 | 恢复 `reader_page.dart` 的旧右键行为 |
| 第 8 步后 | 撤销 CMakeLists.txt 修改 |

## 文件变更清单

| 文件 | 变更类型 | 预估行数 |
|---|---|---|
| `app/rust/src/ai/mod.rs` | 新文件 | ~120 行 |
| `app/rust/src/api/ai.rs` | 新文件 | ~30 行 |
| `app/rust/src/lib.rs` | +1 行 | `pub mod ai;` |
| `app/rust/src/api/mod.rs` | +1 行 | `pub mod ai;` |
| `app/rust/Cargo.toml` | +1 行 | `sha2 = "0.10"` |
| `app/lib/ui/reader_page.dart` | 编辑 | ~60 行 |
| `app/windows/ai/` | 新目录 | exe + models |
| `app/windows/CMakeLists.txt` | 编辑 | ~10 行 |
