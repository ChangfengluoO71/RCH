//! RCH 标准包（`.rchpkg`）导出/导入 — FRB 桥接（P1）。

use crate::rchpkg;

/// 导出结果统计。
pub struct SyncExportInfo {
    pub device_id: String,
    pub created_at: i64,
    pub since: i64,
    pub tags: i64,
    pub book_tags: i64,
    pub metas: i64,
    pub records: i64,
    pub sources: i64,
    pub settings: i64,
    pub tombstones: i64,
}

/// 导入结果统计。
pub struct SyncImportStats {
    pub schema_version: i64,
    pub tags: i64,
    pub book_tags: i64,
    pub metas: i64,
    pub records: i64,
    pub sources: i64,
    pub settings: i64,
}

/// 导出标准包到文件。`incremental=true` 时只导出自上次游标以来的变更。
pub fn rchpkg_export(path: String, incremental: bool) -> Result<SyncExportInfo, String> {
    let info = rchpkg::export_package_to_file(&path, incremental).map_err(|e| e.to_string())?;
    Ok(SyncExportInfo {
        device_id: info.device_id,
        created_at: info.created_at,
        since: info.since,
        tags: info.tags as i64,
        book_tags: info.book_tags as i64,
        metas: info.metas as i64,
        records: info.records as i64,
        sources: info.sources as i64,
        settings: info.settings as i64,
        tombstones: info.tombstones as i64,
    })
}

/// 从文件导入标准包（保留目标端书源凭据；schema 不兼容时拒绝）。
pub fn rchpkg_import(path: String) -> Result<SyncImportStats, String> {
    let stats = rchpkg::import_package_from_file(&path).map_err(|e| e.to_string())?;
    Ok(SyncImportStats {
        schema_version: stats.schema_version,
        tags: stats.tags as i64,
        book_tags: stats.book_tags as i64,
        metas: stats.metas as i64,
        records: stats.records as i64,
        sources: stats.sources as i64,
        settings: stats.settings as i64,
    })
}

/// 默认同步目录约定：`<root>/RCH/sync`。
pub fn rchpkg_default_sync_dir(root: String) -> String {
    rchpkg::default_sync_dir(std::path::Path::new(&root))
        .to_string_lossy()
        .into_owned()
}
