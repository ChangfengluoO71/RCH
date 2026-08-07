//! 命令行工具：把本地数据库的标准包导出为 `.rchpkg`，并附带加密书源凭据。
//! 用法: cargo run --example export_creds -- <database.db> <out.rchpkg> <passphrase>

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("用法: export_creds <database.db> <out.rchpkg> <passphrase>");
        std::process::exit(2);
    }
    rust_lib_app::db::open_at(&args[1]).expect("打开数据库失败");
    let info = rust_lib_app::rchpkg::export_package_with_credentials_to_file(
        &args[2],
        false,
        &args[3],
    )
    .expect("导出失败");
    println!(
        "导出完成: sources={} metas={} tags={} records={} settings={}",
        info.sources, info.metas, info.tags, info.records, info.settings
    );
}
