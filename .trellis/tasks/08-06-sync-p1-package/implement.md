# P1 实施清单

## 本阶段（Rust 内核）

1. `db/mod.rs`：六组 `load_*_for_sync(since)`（含同步列）+ 六个 `apply_sync_*`（sources 凭据 COALESCE 保留）
2. `lib.rs` 注册 `pub mod rchpkg`
3. `rchpkg/mod.rs`：
   - 常量：`FORMAT="rchpkg"`、`SCHEMA_VERSION=1`、包内路径
   - manifest / 分块 serde 结构
   - `export_package(path, incremental)`、`import_package(path) -> ImportStats`
   - `default_sync_dir(root)`（R5 目录约定）
4. 测试：
   - 导出→空库导入 round-trip 数据一致
   - sources 分块不含 password/refresh_token/client_secret/cookie
   - schema 版本不匹配拒绝
   - 增量导出仅含 since 后变更；游标推进
5. `cargo test` 全绿

## 后续步骤（P1 后半段 / P2 前置）

- FRB：`api/package.rs` 暴露 export/import，跑 codegen.ps1 重建绑定
- Dart 侧：备份/恢复入口、同步目录选择（P2）
- P3 合并引擎接入 `apply_sync_*` 的 LWW 语义

## 验证

```bash
cd app/rust && cargo test
```

## 风险与回滚

- zip 依赖 deflate 特性已具备（Cargo.toml）；若写包需要额外 feature 再调整
- 只新增模块与 db 层函数，不触碰现有 CRUD；回滚删除 rchpkg 模块与对应 db 函数即可
