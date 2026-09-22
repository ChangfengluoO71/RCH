# WebDAV 封面：索引 size 无法落库（扫描发布阶段）

> 2026-09-22 建立。用户可见症状：海报墙**能显示封面** ✓，但状态行统计 **失败 231** ✗（`cover_size_missing` 212 + `malformed` 19）。
> 关联提交：`52c2e19`（WebDAV 会话修复）、`409bc53`（size 通路 Dart 侧）、`b6a0330`/`51d0c7d`（后台自愈）、
> `67723ed`/`891b0cd`/`ce5d26c`（临时探针）。

## 已证实的事实（每条都有实测证据）

1. 服务器：`PROPFIND Depth:1` 对某目录返回 **240/240 文件带非零 `getcontentlength`** ✓；
   `Range bytes=0-0` → 206 且 `Content-Range: bytes 0-0/<total>` ✓；`PROPFIND` 7 种变体全 207 ✓。
2. 应用解析：`webdav.rs` 用 `local_name` 解析、`list()` 即 Depth:1 ✓；provider 列表探针
   `list_dir_probe entries=257 files=257 with_size=257` ✓ **全带 size**。
3. 暂存：`remote_scan_listing_stage.entries_json` **含 `"size"`** ✓（316 行）。
4. 自愈：`heal_probe path=/dav/comic matched=1 null_any_source=240` ✓ —— 判定成立、会下钻、会重新列目录 ✓。
5. `stage_directory` 的两道门（代际/纪元闸门、两次持久化）**都没有报错** ✓（`stage_cancelled`/`stage_failed` 均未出现）。

## 仍未确定（下一步的唯一问题）

`library_index.size` 对 WebDAV 资产仍为 **NULL** ✗（244 行），而 `remote_scan_preview` 里
**只有 115 与 quark 的行** ✗（无 webdav）⇒ 需要确认：
**是 preview 写入的条件把 WebDAV 排除，还是 preview 行在发布阶段被清理后、`library_index` 的写入丢了 size？**

## 下一步（按顺序）

1. 在 `persistence::materialize_preview_listing` 的 INSERT 前后各记一条计数诊断（entries / written / source / generation），
   确认 WebDAV 是否真的写入了 preview（用 `SELECT COUNT(*)` 而非 LIKE 前缀猜 source_id 形态 ✗）。
2. 若 preview 有行 ⇒ 查 `catalog.rs` 的发布事务（`load_preview_rows` → `library_index`）为何丢 size；
   若 preview 无行 ⇒ 查 INSERT 的约束（`session_epoch` / `source_fingerprint` / 主键冲突）。
3. 修复后：确认 `library_index.size > 0` ⇒ `cover_size_missing` 归零 ⇒ **后台自愈从此生效**（无需手动点离线索引）。
4. **删除全部临时探针**：`heal_probe`、`list_dir_probe`、`stage_cancelled`、`stage_failed`（代码注释中均已标注）。

## 复现与验证命令

```bash
cd app/rust
RUSTFLAGS="-D warnings" cargo check --locked --all-targets
cargo test --locked -- --test-threads=1
# 真机诊断（读本地库、凭据只在内存）
cargo test --locked --lib webdav_session_probe -- --ignored --nocapture --test-threads=1

# 数据库侧核对（桌面端）
sqlite3 -readonly D:/Documents/RCH/database.db \
  "SELECT (CASE WHEN size IS NULL THEN 'NULL' ELSE '>0' END), COUNT(*) FROM library_index WHERE source_id LIKE 'webdav%' GROUP BY 1;"
```
