# 远程图片文件夹阅读器：执行计划

**Canonical plan:** `../09-14-remote-cloud-scan/research/2026-09-14-remote-cloud-scan.md` Task 3。

- [ ] 为 `RemoteFolderBook` 写页数、自然排序、隐藏过滤、越界、取消和单页大小上限测试。
- [ ] 在 `document/remote_folder.rs` 实现按页 adapter 读取；Range/随机读取优先，非 Range 只允许受限单图片响应。
- [ ] 在 `api/source.rs` 的五类 provider 打开路径中优先识别已提交 `image_folder` 清单；压缩包分支保持不变。
- [ ] 将 `source_browser.dart` 图片文件夹卡片接入正常 Reader，不生成 credential-less source；更新 `reader_page.dart` 的读记录和页面状态。
- [ ] 让 `remote_cache_cleanup.dart` 使用同一 BookKey 清理 page/raw 并保留封面。
- [ ] 运行 `cargo test remote_folder reader:: -- --test-threads=1` 与 Flutter Reader/cache 测试。
