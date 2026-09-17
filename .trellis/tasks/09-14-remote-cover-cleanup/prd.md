# 封面依赖与失效缓存清理

## Goal

把远程封面、别名、图片依赖和 page/raw 清理接入已验证目录 tombstone，避免远程删除后旧封面残留，同时保护阅读完成时的封面和关键状态。

## Requirements

- 封面依赖以逻辑漫画键、依赖路径、依赖指纹和 profile 持久化。
- 只有完整分页、认证有效且 generation 匹配的目录刷新才能产生 tombstone 和 stale cleanup。
- 文件夹删除按路径前缀与依赖表清理子漫画封面、别名和内容缓存；首图删除/改名使图片文件夹封面重新排队。
- 认证/权限/网络/截断/取消和暂时 404 必须保留旧清单、封面、元数据、历史和自定义封面参数。
- 阅读完成的 `purge_remote_book_content_cache` 与已验证删除的 `purge_verified_remote_asset` 必须是两个明确路径。

## Acceptance Criteria

- [ ] 完整删除能清理 folder cover、aliases、依赖图片和 page/raw；无关路径不受影响。
- [ ] 失败/部分刷新不会清理任何旧封面或关键记录。
- [ ] 活跃书籍阅读完成只清 page/raw，cover/custom metadata/history 保留。
- [ ] 同路径新指纹不会复用旧 cover；新 cover 成功后旧 revision 可按策略回收。
- [ ] Rust cache/db 与 Flutter LibraryStore/cleanup 回归测试通过。

## Notes

实现细节和文件边界见父任务计划 Task 4；依赖 manifest 的 proof-carrying listing 状态。
