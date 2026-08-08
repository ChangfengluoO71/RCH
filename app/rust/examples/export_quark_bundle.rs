//! 命令行工具：从 library.json 提取带凭据的远程书源，加密导出为"书源凭据包"。
//! 用法: cargo run --example export_quark_bundle -- <library.json> <out.txt> <passphrase>

use rust_lib_app::rchpkg::{encrypt_source_bundle, SourceCredentialEntry};
use serde_json::Value;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("用法: export_quark_bundle <library.json> <out.txt> <passphrase>");
        std::process::exit(2);
    }
    let raw = std::fs::read_to_string(&args[1]).expect("读取 library.json 失败");
    let j: Value = serde_json::from_str(&raw).expect("解析 library.json 失败");

    let mut entries = Vec::new();
    if let Some(sources) = j.get("sources").and_then(|v| v.as_array()) {
        for s in sources {
            let typ = s.get("type").and_then(|v| v.as_str()).unwrap_or("");
            let has_cred = s.get("cookie").is_some()
                || s.get("password").is_some()
                || s.get("refreshToken").is_some()
                || s.get("clientSecret").is_some();
            if typ.is_empty() || typ == "local" || !has_cred {
                continue;
            }
            let str_field = |k: &str| s.get(k).and_then(|v| v.as_str()).map(String::from);
            entries.push(SourceCredentialEntry {
                id: str_field("id"),
                fingerprint: String::new(),
                r#type: typ.to_string(),
                name: str_field("name"),
                path: str_field("path"),
                url: str_field("url"),
                username: str_field("username"),
                port: s.get("port").and_then(|v| v.as_i64()),
                client_id: str_field("clientId"),
                root_id: str_field("rootId"),
                password: str_field("password"),
                refresh_token: str_field("refreshToken"),
                client_secret: str_field("clientSecret"),
                cookie: str_field("cookie"),
                note: str_field("note").unwrap_or_default(),
            });
        }
    }
    if entries.is_empty() {
        eprintln!("library.json 中没有带凭据的远程书源");
        std::process::exit(1);
    }
    let data = encrypt_source_bundle(&args[3], &entries).expect("加密失败");
    std::fs::write(&args[2], data).expect("写出失败");
    println!("已加密导出 {} 个书源 -> {}", entries.len(), args[2]);
}
