# P1 设计 — 标准包格式（.rchpkg）

## 1. 包结构（zip）

```
manifest.json
chunks/tags.json
chunks/book_tags.json
chunks/metas.json
chunks/records.json
chunks/sources.json
chunks/settings.json
chunks/tombstones.json
```

## 2. manifest.json

```json
{
  "format": "rchpkg",
  "schemaVersion": 1,
  "deviceId": "dev_...",
  "deviceName": "本机",
  "createdAt": 1720000000000,
  "incremental": true,
  "since": 0,
  "chunks": ["tags","book_tags","metas","records","sources","settings","tombstones"]
}
```

- `format` 或 `schemaVersion` 不匹配当前版本 → 拒绝导入并给出可读提示。
- `incremental=true` 时 `since` 为增量起点游标；全量导出 `since=0`。

## 3. 分块字段（行字段 = DB 列，时间戳毫秒）

| 分块 | 字段 |
|---|---|
| tags | id / name / createdAt / updatedAt / deleted |
| book_tags | bookKey / tagId / updatedAt / deleted |
| metas | key / stableId / coverPage / cropX / cropY / cropW / cropH / author / genre / series / title / chineseTitle / summary / comment / rotations / updatedAt / deleted |
| records | key / stableId / sourceId / sourceType / path / title / lastPage / readCount / lastReadAt / updatedAt / deleted |
| sources | id / type / name / path / url / username / port / note / capabilityLabel / fingerprint / remoteOnly / originDeviceId / rootId / clientId / updatedAt / deleted —— **剔除 password / refresh_token / client_secret / cookie** |
| settings | key / value / updatedAt / deleted |
| tombstones | entity / key / updatedAt（导出 deleted=1 行，P3 起真正填充） |

## 4. 导出（export_package）

1. 读游标 `sync_state['cursor_export']`（默认 0）作为 `since`。
2. 六个 `load_*_for_sync(since)` 加载 `updated_at > since` 的行（含 deleted=1 墓碑行）。
3. 剔除 sources 敏感字段 → serde 序列化分块 → zip 写入。
4. 成功后 `set_sync_state('cursor_export', now_ms)`。

## 5. 导入（import_package）

1. 解包 → 校验 manifest（format / schemaVersion）→ 解析分块。
2. 按行 upsert：保留行内 `updated_at` / `deleted` 原值（导入即应用，合并语义归 P3）。
3. sources 凭据保留：`INSERT ... ON CONFLICT(id) DO UPDATE SET <非敏感列>..., password=COALESCE(book_sources.password, excluded.password)`——本地已有凭据不被覆盖，新书源凭据为 NULL（待填凭据状态）。
4. 返回各实体导入行数。

## 6. 目录约定（R5）

- 包名 `latest.rchpkg` + 归档 `archive/{yyyyMMdd_HHmmss}.rchpkg`，统一前缀 `RCH/sync/`。
- P1 提供路径常量与 `default_sync_dir(root)` helper；文件读写编排在 P2。

## 7. 模块边界

- `app/rust/src/rchpkg/mod.rs`：格式常量、manifest/分块结构、zip 读写、校验、导出/导入编排。
- `db/mod.rs`：新增 `*_for_sync` 加载函数与 `apply_sync_*` 导入函数（SQL 留在 db 层）。
- FRB/Dart 暴露（`api/package.rs` + codegen）与 UI 属于 P1 后半段，本阶段先交付 Rust 内核 + 测试。
