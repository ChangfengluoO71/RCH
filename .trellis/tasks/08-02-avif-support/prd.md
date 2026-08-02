# AVIF 格式支持
## Goal

让阅读器与封面支持 AVIF（.avif / .avifs）图片，补齐 SPEC 格式矩阵中标注「未来」的一项。

## Background（已确认事实）

- Rust image crate 0.25 当前 features 仅 jpeg/png/webp/gif [app/rust/Cargo.toml]
- Flutter 端 `MemoryImage`/`ResizeImage` 不支持 AVIF，因此 AVIF 页面字节不能直接交给 Flutter 解码（与 JPEG/PNG 管线不同）
- 现有管线：Rust 返回原始页面字节 → Flutter imageProvider 解码（ADR-004）；封面由 Rust `decode_cover` 解码缩放
- Rust 侧按扩展名分发格式引擎，需将 avif/avifs 加入识别列表
- ADR-011 已预研：AVIF 按需扩展，候选 `avif-decode`（纯 Rust）或 libavif 绑定

## Requirements

- **R1** 扩展名识别 `.avif` / `.avifs`，加入 document 分发与文件夹检测支持。
- **R2** Rust 侧解码 AVIF 并转码为 PNG/JPEG 字节后再返回 Flutter（仅 AVIF 走转码例外；JPEG/PNG 仍走原管线）。
- **R3** `decode_cover` 封面管线支持 AVIF 输入。
- **R4** 缓存策略：转码结果按页缓存，避免每次打开重复解码；缓存 key 含转码格式，换质量设置不串缓存。
- **R5** 依赖选型：优先验证纯 Rust `avif-decode` crate（Windows 构建成本低）；若性能/兼容不足，再评估 image crate 的 avif feature（dav1d 绑定）。
- **R6** 测试 fixture：仓库内置 1 张小型 AVIF 测试图，覆盖页面解码与封面解码单测。

## Acceptance Criteria

- [ ] 打开含 AVIF 页的 ZIP/文件夹漫画，页面正常显示且翻页流畅
- [ ] AVIF 封面在书源浏览页正常显示
- [ ] 二次打开同一页命中缓存，不重复解码
- [ ] `cargo test --lib` 通过（avif fixture 单测）；`flutter analyze` 0 issues
- [ ] Release 构建成功（新增依赖无 Windows 构建失败）

## Out of Scope

- AVIF 编码/导出
- HDR / 10-bit / 动画 AVIF
- 移动端（M6 暂缓）

## Open Questions

- 无阻塞问题。依赖选型（avif-decode vs image avif feature）在实现第 1 步验证构建与解码正确性后确定（R5）。
