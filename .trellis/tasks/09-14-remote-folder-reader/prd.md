# 远程图片文件夹阅读器

## Goal

让所有云端 provider 的直接图片文件夹既能生成封面，也能通过现有 Reader 按页流式/受限读取，不创建整本文件夹 raw。

## Requirements

- 新增 `RemoteFolderBook` `Document`，输入为已提交的图片子清单和 provider session，页数、自然排序和首图稳定。
- Range/随机读取可用时逐页读取；不可用时只允许受 `max_page_bytes` 限制的单图片响应。
- 使用现有 `BookKey`、Reader governor、L1/page cache、预取和 read record；压缩包 stream/download/auto 语义不变。
- SourceBrowser 远程图片文件夹卡片必须走正常 Reader 路径，不得重建 credential-less source。
- 阅读完成只清理 page/raw，保留封面、元数据、凭据、标签、历史、完成状态和自定义封面参数。

## Acceptance Criteria

- [ ] 远程图片文件夹 page count、自然排序、隐藏过滤、越界和单页大小上限测试通过。
- [ ] 五个 provider 均可通过统一 adapter 打开图片文件夹；Range 不可用不会整本下载。
- [ ] 前台当前页优先于文件夹预取和扫描；页缓存命中不产生额外远程请求。
- [ ] 阅读完成后的 page/raw 清理保留 cover 和所有关键状态。
- [ ] `cargo test remote_folder reader:: -- --test-threads=1` 与 Flutter Reader/cache 测试通过。

## Notes

实现细节和文件边界见父任务计划 Task 3；依赖 manifest 中的 `image_folder` 子清单。
