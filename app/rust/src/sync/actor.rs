//! 同步参与者身份（ADR-026 / Phase 4.6）。
//!
//! - device_id：首次运行生成 UUID v4，永久稳定（改名只改 device_name）。
//! - 本地注册表 `sync_devices`；远端 `devices/<device_id>.json`。
//! - writer 是 revision 元数据，绝不参与业务合并（无 LWW）。

use anyhow::Result;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::db;

/// 远端设备文件（devices/<device_id>.json）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceFile {
    pub device_id: String,
    pub name: String,
    pub platform: String,
    pub last_seen_at: i64,
}

pub(crate) fn upsert_device_on(
    conn: &Connection,
    device_id: &str,
    device_name: &str,
    platform: &str,
    last_seen_at: i64,
    last_revision: i64,
) -> Result<()> {
    conn.execute(
        "INSERT INTO sync_devices (device_id, device_name, platform, created_at, last_seen_at, last_revision)
         VALUES (?1, ?2, ?3, ?4, ?4, ?5)
         ON CONFLICT(device_id) DO UPDATE SET
            device_name=excluded.device_name, platform=excluded.platform,
            last_seen_at=excluded.last_seen_at, last_revision=excluded.last_revision",
        params![device_id, device_name, platform, last_seen_at, last_revision],
    )?;
    Ok(())
}

pub fn upsert_device(
    device_id: &str,
    device_name: &str,
    platform: &str,
    last_seen_at: i64,
    last_revision: i64,
) -> Result<()> {
    let conn = db::get().lock().unwrap();
    upsert_device_on(&conn, device_id, device_name, platform, last_seen_at, last_revision)
}

#[derive(Debug, Clone)]
pub struct SyncDeviceRow {
    pub device_id: String,
    pub device_name: String,
    pub platform: String,
    pub created_at: i64,
    pub last_seen_at: i64,
    pub last_revision: i64,
}

pub(crate) fn list_devices_on(conn: &Connection) -> Vec<SyncDeviceRow> {
    let mut stmt = conn
        .prepare(
            "SELECT device_id, device_name, platform, created_at, last_seen_at, last_revision
             FROM sync_devices ORDER BY last_seen_at DESC",
        )
        .unwrap();
    stmt.query_map([], |r| {
        Ok(SyncDeviceRow {
            device_id: r.get(0)?,
            device_name: r.get(1)?,
            platform: r.get(2)?,
            created_at: r.get(3)?,
            last_seen_at: r.get(4)?,
            last_revision: r.get(5)?,
        })
    })
    .unwrap()
    .filter_map(|r| r.ok())
    .collect()
}

pub fn list_devices() -> Vec<SyncDeviceRow> {
    let conn = db::get().lock().unwrap();
    list_devices_on(&conn)
}

/// 注册本机/远端设备（last_seen 最新者覆盖名称；这是身份元数据，非业务合并）。
pub(crate) fn register_devices_on(
    conn: &Connection,
    own: &DeviceFile,
    remotes: &[DeviceFile],
    last_revision: i64,
) -> Result<()> {
    upsert_device_on(
        conn,
        &own.device_id,
        &own.name,
        &own.platform,
        own.last_seen_at,
        last_revision,
    )?;
    for r in remotes {
        upsert_device_on(
            conn,
            &r.device_id,
            &r.name,
            &r.platform,
            r.last_seen_at,
            last_revision,
        )?;
    }
    Ok(())
}

/// 本机设备身份（id 永久稳定；name 来自设置或默认）。
pub fn own_device(conn: &Connection, platform: &str) -> Result<DeviceFile> {
    let device_id = db::get_or_create_device_id_on(conn)?;
    let name = db::load_setting_on(conn, "sync_device_name")
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| {
            std::env::var("COMPUTERNAME")
                .or_else(|_| std::env::var("HOSTNAME"))
                .unwrap_or_else(|_| "本机".to_string())
        });
    Ok(DeviceFile {
        device_id,
        name,
        platform: platform.to_string(),
        last_seen_at: db::now_ms(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn schema_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        db::init_tables(&conn).unwrap();
        conn
    }

    #[test]
    fn device_id_is_stable_uuid() {
        let conn = schema_conn();
        let a = db::get_or_create_device_id_on(&conn).unwrap();
        let b = db::get_or_create_device_id_on(&conn).unwrap();
        assert_eq!(a, b);
        assert_eq!(a.len(), 36);
        assert_eq!(a.matches('-').count(), 4);
        assert_eq!(&a[14..15], "4"); // version 4
    }

    #[test]
    fn device_registry_upsert_and_list() {
        let conn = schema_conn();
        upsert_device_on(&conn, "a", "主电脑", "Windows", 100, 1).unwrap();
        upsert_device_on(&conn, "b", "安卓", "Android", 200, 2).unwrap();
        upsert_device_on(&conn, "a", "主电脑(改名)", "Windows", 300, 3).unwrap();
        let rows = list_devices_on(&conn);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].device_id, "a");
        assert_eq!(rows[0].device_name, "主电脑(改名)");
        assert_eq!(rows[0].last_revision, 3);
    }
}
