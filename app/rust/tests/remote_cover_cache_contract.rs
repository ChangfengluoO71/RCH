use rust_lib_app::cache;

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
