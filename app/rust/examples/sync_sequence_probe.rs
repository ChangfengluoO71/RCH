//! 卷/话在**同步载荷**与**整包备份**里的取证探针（**只对 DB 副本执行，绝不动真实库**；全程离线）。
//!
//! 用法：
//!   cargo run --example sync_sequence_probe -- <DB副本> <包临时路径> [<新库路径>]
//!
//! 输出：
//! [1] `.rchpkg` metas 分块的**数据源**（`db::load_metas_for_sync_on`，导出增量游标同一查询）里带号码的条数
//! [2] 同步快照（transport 真正推送的 `metas` 载荷，`snapshot::load_local_snapshots`）里带号码的条目数
//! [3] 导出整包统计（走真实备份入口 `export_snapshot_to_file`）
//! [3b] 包内自校验（直接读 `metadata/metas.json`：含两键条数 + 非空号码条数）
//! [4] 把整包导入到一个**全新空库**后，仍然带号码的书数（= 备份是否保住号码）
//!
//! 本探针不建会话、不联网、不构造 ByteSource/Downloader/同步传输句柄。

use anyhow::{Context, Result};
use rust_lib_app::{db, rchpkg, sync};
use std::{env, collections::HashMap};

fn main() -> Result<()> {
    let mut args = env::args().skip(1);
    let database = args.next().context("missing database path")?;
    let package = args.next().context("missing package path")?;
    let restored = args.next();

    db::open_at(&database)?;

    // [1] 同步增量 + `.rchpkg` metas 分块的共同数据源
    let rows = db::load_metas_for_sync(0);
    let volume = rows.iter().filter(|r| !r.volume.trim().is_empty()).count();
    let chapter = rows.iter().filter(|r| !r.chapter.trim().is_empty()).count();
    println!(
        "[1] 增量/整包 metas 数据源：rows={} volume={} chapter={}",
        rows.len(),
        volume,
        chapter
    );
    for sample in rows.iter().filter(|r| !r.chapter.trim().is_empty()).take(3) {
        println!(
            "    sample key={} | title={} | volume={} | chapter={}",
            sample.key, sample.title, sample.volume, sample.chapter
        );
    }

    // [2] 同步快照载荷（transport 推送内容）
    let snapshot = {
        let conn = db::get().lock().unwrap();
        sync::snapshot::load_local_snapshots(&conn)?
    };
    let empty = HashMap::new();
    let metas = snapshot.get(sync::base::ENTITY_METAS).unwrap_or(&empty);
    let has_value = |entry: &sync::merge::SyncEntry, field: &str| {
        entry
            .data
            .get(field)
            .and_then(|v| v.as_str())
            .map(|v| !v.trim().is_empty())
            .unwrap_or(false)
    };
    let snap_volume = metas.values().filter(|e| has_value(e, "volume")).count();
    let snap_chapter = metas.values().filter(|e| has_value(e, "chapter")).count();
    println!(
        "[2] 同步快照载荷：entries={} volume={} chapter={}",
        metas.len(),
        snap_volume,
        snap_chapter
    );

    // [3] 整包导出（走真实"备份/导出"入口，与 UI 同一条：`export_snapshot_to_file`）
    let info = rchpkg::export_snapshot_to_file(&package, None)?;
    println!(
        "[3] 导出整包：metas={} records={} library_index={} -> {}",
        info.metas, info.records, info.library_index, package
    );

    // [3b] 包内自校验：直接读包里的 metas 分块，统计"含两键"与"非空号码"
    let mut archive = zip::ZipArchive::new(std::fs::File::open(&package)?)?;
    let mut chunk = String::new();
    {
        use std::io::Read as _;
        archive
            .by_name("metadata/metas.json")?
            .read_to_string(&mut chunk)?;
    }
    let rows_in_package: Vec<serde_json::Value> = serde_json::from_str(&chunk)?;
    let with_keys = rows_in_package
        .iter()
        .filter(|r| r.get("volume").is_some() && r.get("chapter").is_some())
        .count();
    let non_empty = |field: &str| {
        rows_in_package
            .iter()
            .filter(|r| {
                r.get(field)
                    .and_then(|v| v.as_str())
                    .map(|v| !v.trim().is_empty())
                    .unwrap_or(false)
            })
            .count()
    };
    println!(
        "[3b] 包内自校验：rows={} 含两键={} volume非空={} chapter非空={}",
        rows_in_package.len(),
        with_keys,
        non_empty("volume"),
        non_empty("chapter")
    );

    // [4] 导入全新空库，核对号码是否恢复
    if let Some(restored) = restored {
        db::open_at(&restored)?;
        let stats = rchpkg::import_package_from_file(&package)?;
        let metas = db::load_all_metas();
        let volume = metas.iter().filter(|m| !m.volume.trim().is_empty()).count();
        let chapter = metas.iter().filter(|m| !m.chapter.trim().is_empty()).count();
        println!(
            "[4] 导入新库：导入 metas={}（跳过 {}）→ 恢复后 metas={} volume={} chapter={}",
            stats.metas,
            stats.ghosts,
            metas.len(),
            volume,
            chapter
        );
    }
    Ok(())
}
