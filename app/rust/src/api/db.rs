//! 应用数据持久化 API（FRB 桥接）。
//!
//! 所有 Dart 侧状态（书源/阅读记录/元数据/标签/设置）经此模块读写 SQLite。
//! 与 `api/book.rs`（阅读会话）、`api/cache.rs`（缓存管理）、`api/source.rs`（WebDAV 会话）并列。

use crate::db;
use anyhow::Result;

// ============================================================
// 迁移
// ============================================================

/// 数据是否已从 library.json 迁移到 SQLite。
pub fn data_is_migrated() -> bool {
    db::is_migrated()
}

/// 重开数据库连接（根目录切换后调用，使后续读写指向新根目录的数据库）。
pub fn reopen_data_db() -> Result<(), String> {
    db::reopen_data_db().map_err(|e| format!("{e}"))
}

/// 从 library.json 全量导入 SQLite。`json_path` 为 library.json 完整路径。
/// 幂等：已迁移过的数据不重复导入。
pub fn data_migrate_from_json(json_path: String) -> Result<(), String> {
    db::migrate_from_library_json(&json_path).map_err(|e| format!("{e}"))
}

// ============================================================
// BookSource DTO 与 CRUD
// ============================================================

/// 书源 DTO（扁平结构，对应 Dart BookSource）。
pub struct BookSourceDto {
    pub id: String,
    pub r#type: String,
    pub name: String,
    pub path: String,
    pub url: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
    pub port: Option<i64>,
    pub refresh_token: Option<String>,
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub root_id: Option<String>,
    pub cookie: Option<String>,
    pub note: String,
    pub capability_label: String,
    pub remote_only: bool,
    pub origin_device_id: Option<String>,
}

pub fn db_load_all_sources() -> Vec<BookSourceDto> {
    db::load_all_sources()
        .into_iter()
        .map(|r| BookSourceDto {
            id: r.id,
            r#type: r.r#type,
            name: r.name,
            path: r.path,
            url: r.url,
            username: r.username,
            password: r.password,
            port: r.port,
            refresh_token: r.refresh_token,
            client_id: r.client_id,
            client_secret: r.client_secret,
            root_id: r.root_id,
            cookie: r.cookie,
            note: r.note,
            capability_label: r.capability_label,
            remote_only: r.remote_only,
            origin_device_id: r.origin_device_id,
        })
        .collect()
}

pub fn db_upsert_source(source: BookSourceDto) -> Result<(), String> {
    db::upsert_source(&db::BookSourceRow {
        id: source.id,
        r#type: source.r#type,
        name: source.name,
        path: source.path,
        url: source.url,
        username: source.username,
        password: source.password,
        port: source.port,
        refresh_token: source.refresh_token,
        client_id: source.client_id,
        client_secret: source.client_secret,
        root_id: source.root_id,
        cookie: source.cookie,
        note: source.note,
        capability_label: source.capability_label,
        remote_only: source.remote_only,
        origin_device_id: source.origin_device_id,
    })
    .map_err(|e| format!("{e}"))
}

pub fn db_delete_source(id: String) -> Result<(), String> {
    db::delete_source(&id).map_err(|e| format!("{e}"))
}

/// 设备注册表条目（幽灵书源来源展示）。
pub struct DeviceDto {
    pub id: String,
    pub name: String,
}

pub fn db_list_devices() -> Vec<DeviceDto> {
    db::list_devices()
        .into_iter()
        .map(|d| DeviceDto { id: d.id, name: d.name })
        .collect()
}

// ============================================================
// ReadRecord DTO 与 CRUD
// ============================================================

/// 阅读记录 DTO。
pub struct ReadRecordDto {
    pub key: String,
    pub source_id: String,
    pub source_type: String,
    pub path: String,
    pub title: String,
    pub last_page: i32,
    pub read_count: i32,
    pub last_read_at: i64,
}

pub fn db_load_all_records() -> Vec<ReadRecordDto> {
    db::load_all_records()
        .into_iter()
        .map(|r| ReadRecordDto {
            key: r.key,
            source_id: r.source_id,
            source_type: r.source_type,
            path: r.path,
            title: r.title,
            last_page: r.last_page,
            read_count: r.read_count,
            last_read_at: r.last_read_at,
        })
        .collect()
}

pub fn db_upsert_record(record: ReadRecordDto) -> Result<(), String> {
    db::upsert_record(&db::ReadRecordRow {
        key: record.key,
        source_id: record.source_id,
        source_type: record.source_type,
        path: record.path,
        title: record.title,
        last_page: record.last_page,
        read_count: record.read_count,
        last_read_at: record.last_read_at,
    })
    .map_err(|e| format!("{e}"))
}

pub fn db_delete_record(key: String) -> Result<(), String> {
    db::delete_record(&key).map_err(|e| format!("{e}"))
}

pub fn db_delete_records_by_source_prefix(prefix: String) -> Result<u32, String> {
    db::delete_records_by_source_prefix(&prefix).map_err(|e| format!("{e}"))
}

// ============================================================
// BookMeta DTO 与 CRUD
// ============================================================

/// 漫画元数据 DTO。
pub struct BookMetaDto {
    pub key: String,
    pub cover_page: i32,
    pub crop_x: Option<f64>,
    pub crop_y: Option<f64>,
    pub crop_w: Option<f64>,
    pub crop_h: Option<f64>,
    pub author: String,
    pub genre: String,
    pub series: String,
    pub title: String,
    pub chinese_title: String,
    pub summary: String,
    pub comment: String,
    /// 每页旋转（JSON 文本，如 {"0":90}）。
    pub rotations: String,
}

pub fn db_load_all_metas() -> Vec<BookMetaDto> {
    db::load_all_metas()
        .into_iter()
        .map(|m| BookMetaDto {
            key: m.key,
            cover_page: m.cover_page,
            crop_x: m.crop_x,
            crop_y: m.crop_y,
            crop_w: m.crop_w,
            crop_h: m.crop_h,
            author: m.author,
            genre: m.genre,
            series: m.series,
            title: m.title,
            chinese_title: m.chinese_title,
            summary: m.summary,
            comment: m.comment,
            rotations: m.rotations,
        })
        .collect()
}

pub fn db_upsert_meta(meta: BookMetaDto) -> Result<(), String> {
    db::upsert_meta(&db::BookMetaRow {
        key: meta.key,
        cover_page: meta.cover_page,
        crop_x: meta.crop_x,
        crop_y: meta.crop_y,
        crop_w: meta.crop_w,
        crop_h: meta.crop_h,
        author: meta.author,
        genre: meta.genre,
        series: meta.series,
        title: meta.title,
        chinese_title: meta.chinese_title,
        summary: meta.summary,
        comment: meta.comment,
        rotations: meta.rotations,
    })
    .map_err(|e| format!("{e}"))
}

pub fn db_delete_meta(key: String) -> Result<(), String> {
    db::delete_meta(&key).map_err(|e| format!("{e}"))
}

pub fn db_delete_metas_by_source_prefix(prefix: String) -> Result<u32, String> {
    db::delete_metas_by_source_prefix(&prefix).map_err(|e| format!("{e}"))
}

// ============================================================
// Tag DTO 与 CRUD
// ============================================================

/// 标签实体 DTO。
pub struct TagDto {
    pub id: String,
    pub name: String,
    pub created_at: i64,
}

/// 漫画-标签关联 DTO。
pub struct BookTagDto {
    pub book_key: String,
    pub tag_id: String,
}

pub fn db_load_all_tags() -> Vec<TagDto> {
    db::load_all_tags()
        .into_iter()
        .map(|t| TagDto {
            id: t.id,
            name: t.name,
            created_at: t.created_at,
        })
        .collect()
}

pub fn db_load_all_book_tags() -> Vec<BookTagDto> {
    db::load_all_book_tags()
        .into_iter()
        .map(|bt| BookTagDto {
            book_key: bt.book_key,
            tag_id: bt.tag_id,
        })
        .collect()
}

/// 确保标签存在（幂等），返回标签 DTO。若标签已存在直接返回，否则创建。
pub fn db_ensure_tag(name: String) -> Result<TagDto, String> {
    db::ensure_tag(&name)
        .map(|t| TagDto {
            id: t.id,
            name: t.name,
            created_at: t.created_at,
        })
        .map_err(|e| format!("{e}"))
}

/// 重命名标签（关联自动迁移）。
pub fn db_rename_tag(old_name: String, new_name: String) -> Result<(), String> {
    db::rename_tag(&old_name, &new_name).map_err(|e| format!("{e}"))
}

/// 删除标签及所有关联。
pub fn db_delete_tag(name: String) -> Result<(), String> {
    db::delete_tag(&name).map_err(|e| format!("{e}"))
}

/// 将标签关联到一本书（幂等）。
pub fn db_link_tag(book_key: String, tag_name: String) -> Result<(), String> {
    db::link_tag(&book_key, &tag_name).map_err(|e| format!("{e}"))
}

/// 将标签从一本书移除。
pub fn db_unlink_tag(book_key: String, tag_name: String) -> Result<(), String> {
    db::unlink_tag(&book_key, &tag_name).map_err(|e| format!("{e}"))
}

/// 设置一本书的标签集（全量替换）。
pub fn db_set_book_tags(book_key: String, tag_names: Vec<String>) -> Result<(), String> {
    db::set_book_tags(&book_key, &tag_names).map_err(|e| format!("{e}"))
}

// ============================================================
// 设置 CRUD
// ============================================================

/// 设置条目 DTO。
pub struct SettingEntryDto {
    pub key: String,
    pub value: String,
}

pub fn db_load_all_settings() -> Vec<SettingEntryDto> {
    db::load_all_settings()
        .into_iter()
        .map(|s| SettingEntryDto {
            key: s.key,
            value: s.value,
        })
        .collect()
}

pub fn db_save_setting(key: String, value: String) -> Result<(), String> {
    db::save_setting(&key, &value).map_err(|e| format!("{e}"))
}

/// AI 超分后台任务 DTO。
pub struct AiTaskDto {
    pub id: String,
    pub book_key: String,
    pub source_type: String,
    pub source_id: String,
    pub path: String,
    pub title: String,
    pub scale: i64,
    pub total: i64,
    pub done: i64,
    pub status: String,
    pub sort_order: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

/// 写入/更新 AI 超分任务。
pub fn db_upsert_ai_task(task: AiTaskDto) -> Result<(), String> {
    db::upsert_ai_task(&db::AiTaskRow {
        id: task.id,
        book_key: task.book_key,
        source_type: task.source_type,
        source_id: task.source_id,
        path: task.path,
        title: task.title,
        scale: task.scale,
        total: task.total,
        done: task.done,
        status: task.status,
        sort_order: task.sort_order,
        created_at: task.created_at,
        updated_at: task.updated_at,
    })
    .map_err(|e| format!("{e}"))
}

/// 加载全部 AI 超分任务。
pub fn db_load_all_ai_tasks() -> Vec<AiTaskDto> {
    db::load_all_ai_tasks()
        .into_iter()
        .map(|t| AiTaskDto {
            id: t.id,
            book_key: t.book_key,
            source_type: t.source_type,
            source_id: t.source_id,
            path: t.path,
            title: t.title,
            scale: t.scale,
            total: t.total,
            done: t.done,
            status: t.status,
            sort_order: t.sort_order,
            created_at: t.created_at,
            updated_at: t.updated_at,
        })
        .collect()
}

/// 按给定顺序重排排队任务的 sort_order（1..N）。调用方应只传排队中任务的 id。
pub fn db_reorder_ai_tasks(ids: Vec<String>) -> Result<(), String> {
    db::reorder_ai_tasks(&ids).map_err(|e| format!("{e}"))
}

/// 删除 AI 超分任务。
pub fn db_delete_ai_task(id: String) -> Result<(), String> {
    db::delete_ai_task(&id).map_err(|e| format!("{e}"))
}

pub fn db_delete_setting(key: String) -> Result<(), String> {
    db::delete_setting(&key).map_err(|e| format!("{e}"))
}
