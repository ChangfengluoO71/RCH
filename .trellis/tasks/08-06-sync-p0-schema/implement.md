# P0 实施清单

## 修改文件

- `app/rust/src/db/mod.rs`（唯一产品代码改动点）

## 顺序

1. init_tables：六张表 CREATE TABLE 追加新列；新增 devices / sync_state / source_alias 与索引
2. 新增 `ensure_columns` helper 并接入 init_tables 老库补列
3. 重写 upsert_source / upsert_record / upsert_meta / save_setting / link_tag / set_book_tags 为 ON CONFLICT 语义；`load_all_book_tags` 过滤 deleted=0
4. 新增 helper（device_id / sync_state / source_alias / fingerprint / stable_id）；delete_source 清理 source_alias
5. `#[cfg(test)]` 测试（内存库）

## 验证

```bash
cd app/rust && cargo check && cargo test
cd app && flutter analyze && flutter test
```

## 风险点与回滚

- `INSERT OR REPLACE` → `ON CONFLICT` 行为差异：主键冲突时不再删除重建，外键/级联副作用更小；回归重点为 upsert 后 load 数据一致
- 若 cargo test 受 FRB 生成代码编译影响，退化为 cargo check + flutter test 覆盖
- 回滚：还原 db/mod.rs 单一文件即可
