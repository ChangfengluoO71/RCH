# WebDAV 同步 URL 基础路径修复

## Goal

坚果云等带基础路径的 WebDAV（如 `https://dav.jianguoyun.com/dav/`）同步推送失败：MKCOL 请求丢失 `/dav/` 前缀，服务器返回 410 Gone。

## Requirements

- `WebDavClient` 保留 URL 携带的基础路径，构造请求 URL 时对不包含该前缀的相对路径自动补全。
- 已含基础前缀的路径（如 PROPFIND 返回的 href）不得重复拼接；无基础路径的服务器行为保持不变。

## Acceptance Criteria

- [x] 同步目录 MKCOL/PUT/GET 均发往带基础前缀的正确路径。
- [x] `cargo test --lib` 全绿（87 passed，含 3 个新增 URL 构造用例）。
- [x] 坚果云实测推送成功。

## Notes

- Keep `prd.md` focused on requirements, constraints, and acceptance criteria.
- Lightweight tasks can remain PRD-only.
- For complex tasks, add `design.md` for technical design and `implement.md` for execution planning before `task.py start`.
