# 设计：外观设置与平台材质

本任务的设计以 [外观设置与平台材质设计规格](../../../docs/superpowers/specs/2026-10-09-rch-appearance-materials-design.md) 为准。该规格待用户审阅后，才能编写实现计划并进入实现阶段。

核心决策：

- 外观设置在现有入口内做局部整理，不重排整个设置页。
- 亮度、固定配色、Android 动态配色、字体和材质分开持久化。
- Android 动态配色通过 `dynamic_color` 获取，仅 Android 12+ 使用。
- Windows Mica 通过 Win32 DWM 应用于主窗口；Acrylic 通过 Flutter `BackdropFilter` 应用于对话框和模态底部面板。
- 新设置缺省为现有行为，平台不支持时回退到标准材质和固定 RCH 配色。
