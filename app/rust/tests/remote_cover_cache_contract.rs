use rust_lib_app::cache;
use rust_lib_app::db;
use rust_lib_app::remote_scan::cover_model::CoverJobKey;
use rust_lib_app::remote_scan::cover_service;

#[test]
fn versioned_remote_cover_cache_is_source_scoped_and_offline_readable() {
    let root = std::env::temp_dir().join(format!(
        "rch_remote_cover_cache_contract_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    cache::set_custom_cache_root(root.to_str().unwrap());

    let rgba = vec![1, 2, 3, 4];
    cache::remote_cover_cache_write(
        "source-a", "asset-a", "rev-1", "sel-1", "200x300", 1, 1, &rgba,
    )
    .unwrap();
    assert_eq!(
        cache::remote_cover_cache_read("source-a", "asset-a", "rev-1", "sel-1", "200x300"),
        Some((rgba.clone(), 1, 1))
    );
    assert_eq!(
        cache::remote_cover_cache_read("source-b", "asset-a", "rev-1", "sel-1", "200x300"),
        None
    );

    cache::set_custom_cache_root("");
    let _ = std::fs::remove_dir_all(root);
}

/// 第 82 轮补（D4）：**durable 行被清了，但磁盘字节还在**时也必须读得出封面。
///
/// 真机形状（2026-09-21 实测）：`remote_cover_variant = 0`，而
/// `remote_cover_blob = remote_cover_ref = 1061`（1.2 GB）——第 80 轮换档 purge 只删
/// job/variant，blob 文件被 ref 留着。于是卡片读状态拿不到 ready、直接抛异常显示占位，
/// 尽管 `.cover-v2` 就在本地。
///
/// 修法：`read_cached_cover` 在**没有任何 durable ready 行**时，用
/// `remote_cover_ref.owner_key`（=`CoverJobKey::encode()`，含 content_revision）反推
/// 缓存文件名并**纯只读**地取出同一份字节。
#[test]
fn ref_backed_read_serves_bytes_when_durable_rows_were_purged() {
    let root = std::env::temp_dir().join(format!("rch_cover_ref_backed_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    cache::set_custom_cache_root(root.to_str().unwrap());

    let rgba = vec![9, 8, 7, 6];
    cache::remote_cover_cache_write("src", "asset", "rev-1", "default", "170x240@1", 1, 1, &rgba)
        .unwrap();

    {
        let conn = db::get().lock().unwrap();
        rust_lib_app::remote_scan::persistence::migrate(&conn).unwrap();
        rust_lib_app::remote_scan::cover_store::migrate(&conn).unwrap();
        // 复现 purge 后的形状：job/variant 行全无，只剩 ref + 磁盘字节。
        for table in ["remote_cover_job", "remote_cover_variant", "remote_cover_ref"] {
            conn.execute(&format!("DELETE FROM {table} WHERE source_id='src'"), [])
                .unwrap();
        }
        let owner_key = CoverJobKey {
            source_id: "src".into(),
            asset_id: "asset".into(),
            content_revision: "rev-1".into(),
            selection_revision: "default".into(),
            profile: "170x240@1".into(),
        }
        .encode();
        conn.execute(
            "INSERT INTO remote_cover_ref(owner_key,blob_key,role,source_id,asset_id,dependency_revision)
             VALUES(?1,'blob-1','variant','src','asset','rev-1')",
            rusqlite::params![owner_key],
        )
        .unwrap();
    }

    let got = cover_service::read_cached_cover("src", "asset", "default", "170x240@1").unwrap();
    assert_eq!(
        got,
        Some((rgba.clone(), 1, 1)),
        "只剩 ref+字节时也必须读得出封面（D4）"
    );

    // 反例：别的档位不得被这条回退误命中（回退只认完全一致的 selection+profile）。
    assert_eq!(
        cover_service::read_cached_cover("src", "asset", "default", "340x480@1").unwrap(),
        None
    );

    cache::set_custom_cache_root("");
    let _ = std::fs::remove_dir_all(root);
}
