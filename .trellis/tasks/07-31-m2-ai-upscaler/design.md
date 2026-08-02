# M2 AI 超分 — 技术设计 (design.md)

## 1. 整体架构

```
┌─────────────────────────────────────────────────┐
│ Dart UI（reader_page.dart）                       │
│   右键菜单 → super_resolve(bytes, 2)             │
│   返回超分 bytes → 替换当前页显示                   │
├─────────────────────────────────────────────────┤
│ FRB 桥接                                         │
│   api/ai.rs: super_resolve(bytes,scale)->bytes   │
├─────────────────────────────────────────────────┤
│ Rust ai/ 模块                                    │
│   ┌──────────────────────────────────────────┐  │
│   │ super_resolve(bytes, scale)              │  │
│   │   ├─ sha256(bytes) → 查 ai/ 缓存 → 命中  │  │
│   │   ├─ image::load(bytes) → RGB 解码       │  │
│   │   ├─ 写入 temp/ 临时 PNG                  │  │
│   │   ├─ Command::new(exe_path)              │  │
│   │   │    -i temp.png -o temp_out.png       │  │
│   │   │    -s 2 -n realesr-animevideov3      │  │
│   │   ├─ 等待 exit + 超时检测 (60s)          │  │
│   │   ├─ image::open(temp_out.png) → JPEG    │  │
│   │   ├─ 写入 ai/ 缓存                       │  │
│   │   └─ 清理 temp/ 临时文件                  │  │
│   └──────────────────────────────────────────┘  │
└─────────────────────────────────────────────────┘
```

## 2. 数据流

```
用户右键 → showMenu(['阅读设置','AI 超分 (2x)'])
   │
   ├─ "阅读设置" → 现有 _showSettings()
   │
   └─ "AI 超分 (2x)"
        │
        ├─ 取当前页原始字节: _bytes[_page]  (Uint8List = 压缩图片)
        │
        ├─ 调 Rust: superResolve(pageBytes: Uint8List, scale: 2) → Future<Uint8List>
        │   │
        │   ├─ Rust super_resolve(bytes, 2):
        │   │   1. 计算 hash = sha256(bytes)
        │   │   2. 查 ai/ 缓存: read(ai/{hash}_realesr-animevideov3_2x.ai)
        │   │      → 命中则直接返回 (跳过推理)
        │   │   3. 解码 bytes → DynamicImage (image crate)
        │   │   4. 写入 temp/ 临时 PNG: rch_temp_ai_input_{rand}.png
        │   │   5. 确定 exe 路径: current_exe.parent / "data" / "ai" / "realesrgan-ncnn-vulkan.exe"
        │   │   6. spawn: Command::new(exe)
        │   │        .args(["-i", input_png, "-o", output_png, "-s", "2", "-n", "realesr-animevideov3"])
        │   │        .timeout(Duration::from_secs(60))
        │   │   7. 检查 exit code == 0 → 成功
        │   │   8. 读 output_png → image::open → 编码 JPEG quality 90
        │   │   9. 写 ai/ 缓存
        │   │   10. 删除 temp/ 临时文件
        │   │   11. 返回 JPEG bytes
        │   │
        │   └─ 错误处理:
        │      - exe 不存在 → Err("AI 超分引擎未安装，请确认...")
        │      - 超时 60s → kill 进程 → Err("AI 超分超时")
        │      - exit code != 0 → Err("AI 超分失败，退出码: {code}")
        │
        ├─ Dart 侧 _doAiSuperResolve():
        │   1. 显示 SnackBar: "AI 超分处理中..."
        │   2. await superResolve(bytes, 2)
        │   3. 成功: _bytes[_page] = 返回值 + setState → 页面刷新
        │      显示 SnackBar: "AI 超分完成 ✓"
        │   4. 失败: 显示 SnackBar: "AI 超分失败: {error}"
        │   5. finally: 恢复 UI 交互
        │
        └─ 完成: 阅读器显示超分后的高清图片
```

## 3. 文件结构

```
新增/修改文件:
├─ app/rust/src/ai/
│   └─ mod.rs              # 新文件: 超分编排 (~120 行)
├─ app/rust/src/api/ai.rs  # 新文件: FRB 桥接 (~50 行)
├─ app/rust/src/lib.rs     # 编辑: +pub mod ai;
├─ app/rust/src/api/mod.rs # 编辑: +pub mod ai;
├─ app/rust/Cargo.toml     # 编辑: +sha2, +base64
├─ app/lib/ui/reader_page.dart # 编辑: 右键菜单 + 超分逻辑 (~50 行)
├─ app/windows/ai/         # 新目录: 存放 exe + models/
│   ├─ realesrgan-ncnn-vulkan.exe
│   └─ models/
│       ├─ realesr-animevideov3-x2.bin
│       └─ realesr-animevideov3-x2.param
└─ app/windows/CMakeLists.txt # 编辑: 复制 ai/ 到输出目录
```

## 4. 关键设计决策

### 4.1 exe 路径定位
```
std::env::current_exe()
  → 在开发时: app/build/windows/x64/runner/Release/RCH.exe
  → 安装后: C:/Program Files/RCH/RCH.exe

exe 路径 = current_exe.parent / "data" / "ai" / "realesrgan-ncnn-vulkan.exe"
模型路径 = current_exe.parent / "data" / "ai" / "models"
```
因为 `setup.iss` 的 `[Files]` 用 `recursesubdirs` 递归复制，`data/ai/` 会出现在 Release 目录下。

### 4.2 输入格式策略
CLI 接受 jpg/png/webp。用户漫画页可能是这几种格式。Rust `image` crate 自动识别格式，统一解码 → 写 PNG 临时文件 → 送 CLI → 读 PNG 结果 → 编码 JPEG 返回。

选择 JPEG 返回的原因：体积可控，漫画阅读器性能平衡。

### 4.3 缓存策略
- 缓存 key = `sha256(原始压缩字节)_模型名_倍率.ai`
- 用 sha256 而非路径 hash：同一张图跨场景复用更可靠
- 价值：如果用户在阅读器中反复开关超分，第二次直接命中缓存，不调 CLI

### 4.4 进程超时与清理
- 单次推理超时 60s（通常 2-5s）
- 超时时 kill 进程 + 清理临时文件
- `finally` 块确保 temp/ 不泄漏

## 5. 风险点

| 风险 | 缓解 |
|---|---|
| Vulkan 不兼容用户 GPU | exe 启动失败 → 返回友好错误消息 |
| 超大图片 OOM | 当前不限制，后续可加尺寸上限（如 4096×4096） |
| 临时文件磁盘占用 | `finally` 清理 + CacheDir::Temp 已支持独立清空 |
| exe 被杀毒软件拦截 | 分发前用 VirusTotal 扫描 |
| 用户修改安装目录 | 用 `current_exe()` 相对路径，不硬编码 |

## 6. CMake 集成

```cmake
# 在 app/windows/CMakeLists.txt 中添加:
# 复制 AI 引擎文件到 build 输出目录
add_custom_command(TARGET ${BINARY_NAME} POST_BUILD
  COMMAND ${CMAKE_COMMAND} -E copy_directory
  "${CMAKE_CURRENT_SOURCE_DIR}/ai"
  "$<TARGET_FILE_DIR:${BINARY_NAME}>/data/ai"
)
```

这样 `flutter build windows` 后 exe 同级的 `data/ai/` 下就有引擎文件了。
