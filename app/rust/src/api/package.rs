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
    pub tombstones: i64,
    pub ghosts: i64,
    pub skipped: i64,
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

/// 合并/导入标准包。`force=true` 恢复（包覆盖，凭据保留）；`false` 拉取合并（LWW + 墓碑）。
pub fn rchpkg_import(path: String, force: bool) -> Result<SyncImportStats, String> {
    let stats = rchpkg::merge_package_from_file(&path, force).map_err(|e| e.to_string())?;
    Ok(SyncImportStats {
        schema_version: stats.schema_version,
        tags: stats.tags as i64,
        book_tags: stats.book_tags as i64,
        metas: stats.metas as i64,
        records: stats.records as i64,
        sources: stats.sources as i64,
        settings: stats.settings as i64,
        tombstones: stats.tombstones as i64,
        ghosts: stats.ghosts as i64,
        skipped: stats.skipped as i64,
    })
}

/// 导出标准包并附带加密凭据分块（凭据 AES-256-GCM + 口令派生，按 fingerprint 匹配）。
pub fn rchpkg_export_with_credentials(
    path: String,
    incremental: bool,
    passphrase: String,
) -> Result<SyncExportInfo, String> {
    let info = rchpkg::export_package_with_credentials_to_file(&path, incremental, &passphrase)
        .map_err(|e| e.to_string())?;
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

/// 导入标准包并应用加密凭据分块（需口令，口令错误则整体中止）。
pub fn rchpkg_import_with_credentials(
    path: String,
    passphrase: String,
) -> Result<SyncImportStats, String> {
    let stats = rchpkg::import_package_with_credentials_from_file(&path, &passphrase)
        .map_err(|e| e.to_string())?;
    Ok(SyncImportStats {
        schema_version: stats.schema_version,
        tags: stats.tags as i64,
        book_tags: stats.book_tags as i64,
        metas: stats.metas as i64,
        records: stats.records as i64,
        sources: stats.sources as i64,
        settings: stats.settings as i64,
        tombstones: stats.tombstones as i64,
        ghosts: stats.ghosts as i64,
        skipped: stats.skipped as i64,
    })
}

/// 默认同步目录约定：`<root>/RCH/sync`。
pub fn rchpkg_default_sync_dir(root: String) -> String {
    rchpkg::default_sync_dir(std::path::Path::new(&root))
        .to_string_lossy()
        .into_owned()
}

/// 书源凭据包条目（加密导入用）。
pub struct SourceBundleDto {
    pub id: String,
    pub r#type: String,
    pub name: String,
    pub path: String,
    pub root_id: Option<String>,
    pub password: Option<String>,
    pub refresh_token: Option<String>,
    pub client_secret: Option<String>,
    pub cookie: Option<String>,
}

impl From<rchpkg::SourceCredentialEntry> for SourceBundleDto {
    fn from(e: rchpkg::SourceCredentialEntry) -> Self {
        SourceBundleDto {
            id: e.id.unwrap_or_default(),
            r#type: e.r#type,
            name: e.name.unwrap_or_default(),
            path: e.path.unwrap_or_default(),
            root_id: e.root_id,
            password: e.password,
            refresh_token: e.refresh_token,
            client_secret: e.client_secret,
            cookie: e.cookie,
        }
    }
}

impl From<SourceBundleDto> for rchpkg::SourceCredentialEntry {
    fn from(d: SourceBundleDto) -> Self {
        rchpkg::SourceCredentialEntry {
            id: Some(d.id),
            fingerprint: String::new(),
            r#type: d.r#type,
            name: Some(d.name),
            path: Some(d.path),
            root_id: d.root_id,
            password: d.password,
            refresh_token: d.refresh_token,
            client_secret: d.client_secret,
            cookie: d.cookie,
        }
    }
}

/// 加密导出"书源凭据包"：返回 JSON 文本（AES-256-GCM + 口令派生）。
pub fn source_bundle_encrypt(
    passphrase: String,
    sources: Vec<SourceBundleDto>,
) -> Result<String, String> {
    let entries: Vec<rchpkg::SourceCredentialEntry> =
        sources.into_iter().map(Into::into).collect();
    rchpkg::encrypt_source_bundle(&passphrase, &entries).map_err(|e| e.to_string())
}

/// 解密"书源凭据包"：口令错误或数据损坏会报错。
pub fn source_bundle_decrypt(
    passphrase: String,
    data: String,
) -> Result<Vec<SourceBundleDto>, String> {
    let entries = rchpkg::decrypt_source_bundle(&passphrase, &data).map_err(|e| e.to_string())?;
    Ok(entries.into_iter().map(Into::into).collect())
}
