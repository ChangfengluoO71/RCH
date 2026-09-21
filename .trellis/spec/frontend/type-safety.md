# Type Safety（Dart 侧类型与边界约定）

> 2026-09-21 补实（替换原模板）。

## FRB 边界

- Rust 类型经 `flutter_rust_bridge` 生成为 `lib/src/rust/**`（**生成物，勿手改**）；
  业务代码通过 `package:app/src/rust/api/*.dart` 使用，不直接引用 `frb_generated.dart` 内部符号；
- 64 位整数用 `BigInt`（如 session）；测试构造用 `PlatformInt64Util.from(0)`；
- 可空性显式处理（`BigInt? session`、`String? errorCode`），**不要**用 `!` 硬断言跨边界数据。

## 领域模型

- 领域数据一律用 `store/models.dart` 的类（`BookSource` / `AppSettings` / `RenderWidth` …），
  不把 `Map<String, dynamic>` 直接传到 UI；
- JSON 解析走 `fromJson` 并对**缺失/未知值**给显式回退：例 `AppSettings.renderWidth`
  用 `orElse: RenderWidth.standard`（未知档位回落标准档，不抛异常）；
- 纯映射写成**顶层纯函数**并单测：例 `renderWidthPixels(mode, {screenWidth, devicePixelRatio})`
  （标准档返回 `null` ⇒ 保持历史页缓存路径不变）。

## 常量与魔法数

- 尺寸/阈值要有名字与出处：`CACHE_CAP`、`PDF_RENDER_TARGET_WIDTH`、`RenderWidth.dataSaver(1080, …)`；
- 单位写进名字（`Ms` / `Bytes` / `Width`），禁止裸数字散落在 UI 里。
