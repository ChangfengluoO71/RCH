# Bug 记录：Windows 桌面端启动黑屏（已定位并修复，commit e31a305）

## 现象

- 桌面端（Windows）应用启动后窗口存在、进程存活，但**黑屏加载不出来**（无任何界面）。
- 复现日志（flutter run -d windows / 直接运行 RCH.exe + stdout 重定向）：
  - Dart VM 已启动：`The Dart VM service is listening on http://127.0.0.1:.../`
  - main() 无任何后续输出（未到 runApp），无异常打印 → 疑似卡在某个启动 await（RustLib.init 或数据加载）。
- 同一份代码在安卓模拟器上正常渲染（rch_avd 411dp / MuMu 横竖屏均正常），宽屏布局（Windows/平板 AVD）必现。
- 用户真实数据位于 `C:\Users\cfl\AppData\Roaming\RCH\RCH\`（library.json + database.db，含夸克/百度源）。

## 已排查

- 不是旧 exe + 新 FRB 绑定不匹配：22:22 用户前台重建的 exe + 21:58 重建的 release DLL（同源绑定）。
- SyncManager.init / AiUpscaleManager.init 均为纯 DB 读取，无网络调用。
- 启动 await 全部是 Rust/DB/文件操作，理论上不应挂起；但主线程无输出，卡点未定位。
- 环境问题（次要）：本会话中 `flutter build windows` 后台隐藏方式多次完全冻结（CPU 0、无输出）；前台 verbose 曾正常推进到 ClCompile；用户自己的终端可正常构建。

## 根因（最终定位）

- 用户提供报错：`RenderFlex children have non-zero flex but incoming height constraints are unbounded.`，指向 home_page.dart `_buildSourceList()` 的 Column。
- Flutter `RenderFlex._constraintsForNonFlexChild`：**Column 的非 flex 子节点会被给予无限高度**（`BoxConstraints(maxWidth: ...)`，无 maxHeight）。
- P5 窄屏适配把侧栏书源列表抽成 `_buildSourceList()`（内部 Column+Expanded）后，它作为侧栏 Column 的**非 flex 子节点**被放入，内层 Expanded 收到 `h=Infinity` → 宽屏布局崩溃黑屏；手机端因包在 Expanded 内不受影响。
- 用 `flutter test` 最小探针复现：`Column([非flex..., Column([..., Expanded])])` 在宽布局下必现；`Expanded` 直接放外层则正常。

## 修复

- home_page.dart 侧栏：`_buildSourceList()` → `Expanded(child: _buildSourceList())`，使其成为有界 flex 子节点。
- 新增回归测试 `test/home_page_layout_test.dart`：宽屏 1200×800 与窄屏 400×800 的 HomePage 布局均无异常。
- 教训：共享 UI 重构（安卓适配）必须同时验证 Windows 宽屏布局；本会话 Windows 构建环境空转导致跳过桌面验证，属错误决策。
