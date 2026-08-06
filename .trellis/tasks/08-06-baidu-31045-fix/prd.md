# 百度网盘 31045 修复：dlink 拼接 access_token + 下载 403 强制刷新 + 书源删除 SQLite 持久化

## Goal

百度网盘源远程下载报 31045（access_token 验证未通过）。根因：filemetas 返回的 dlink 不含 access_token，下载请求未拼接；同 AppKey 多书源互相轮换 token；书源删除未同步 SQLite。修复：下载统一拼接当前 access_token、403 强制刷新重试、删除书源/清理失效记录同步删 SQLite 行。

## Requirements

1. 下载 dlink 时必须拼接当前有效 `access_token`（官方要求），并携带 `User-Agent: pan.baidu.com`。
2. 下载遇到 403（31045）时，强制刷新 token 后重取 dlink 重试一次。
3. API 请求遇到 errno -6/110/31045 自动刷新重试一次。
4. 拦截 200 + JSON 错误体，避免把错误内容当文件写入缓存；错误信息附带响应体片段便于定位。
5. 删除书源 / 清理失效阅读记录时，同步删除 SQLite 行（`dbDeleteSource` / `dbDeleteRecordsBySourcePrefix` / `dbDeleteMetasBySourcePrefix` / `dbDeleteRecord`），避免重启后复活。

## Acceptance Criteria

- [x] filemetas 返回的 dlink 在下载请求中带当前 `access_token`，URL 重建不改变原参数（百分号编码往返稳定）。
- [x] 下载 403/31045 时自动刷新 token 并重试；实测 dlink + token + UA → 302 → 200，拿到真实 PDF。
- [x] 同 AppKey 多书源互相轮换 token 导致 31045 的已知坑记录进 `netdisk-source.md`。
- [x] `removeSourceWithCleanup` / `purgeStaleRecords` 同步删 SQLite 行，删除的书源重启后不再复活。
- [x] `cargo check`、8 个百度模块单测、`flutter analyze` 全部通过。

## Notes

- Keep `prd.md` focused on requirements, constraints, and acceptance criteria.
- Lightweight tasks can remain PRD-only.
- For complex tasks, add `design.md` for technical design and `implement.md` for execution planning before `task.py start`.
