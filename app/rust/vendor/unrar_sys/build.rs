fn main() {
    // `cfg!(windows)` 在 build script 里按宿主求值：Windows 主机交叉编译
    // Android 时也会注入 powrprof/shell32，导致 cdylib 链接失败。
    // 必须按目标平台判断（与下方 isnt.cpp 的处理一致）。
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "windows" {
        println!("cargo:rustc-flags=-lpowrprof");
        println!("cargo:rustc-link-lib=shell32");
        if cfg!(target_env = "gnu") {
            println!("cargo:rustc-link-lib=pthread");
        }
    } else if target_os == "android" {
        // Android 上 C++ 标准库走 libc++_shared.so（随 APK 打包进 jniLibs）。
        // 显式链接 -lc++，让最终 cdylib 带 DT_NEEDED libc++_shared.so，
        // 否则加载器不会加载它，std::length_error 等符号无法解析。
        println!("cargo:rustc-link-lib=c++");
    } else {
        // Android 的 bionic 将 pthread 并入 libc，没有独立的 libpthread；
        // 其余类 Unix 平台需要 -lpthread。
        println!("cargo:rustc-link-lib=pthread");
    }
    let mut files: Vec<String> = [
        "strlist",
        "strfn",
        "pathfn",
        "smallfn",
        "global",
        "file",
        "filefn",
        "filcreat",
        "archive",
        "arcread",
        "unicode",
        "system",
        "crypt",
        "crc",
        "rawread",
        "encname",
        "match",
        "timefn",
        "rdwrfn",
        "consio",
        "options",
        "errhnd",
        "rarvm",
        "secpassword",
        "rijndael",
        "getbits",
        "sha1",
        "sha256",
        "blake2s",
        "hash",
        "extinfo",
        "extract",
        "volume",
        "list",
        "find",
        "unpack",
        "headers",
        "threadpool",
        "rs16",
        "cmddata",
        "ui",
        "filestr",
        "scantree",
        "dll",
        "qopen",
    ].iter().map(|&s| format!("vendor/unrar/{s}.cpp")).collect();
    // The `isnt` source is Windows-only; `#[cfg(windows)]` in a build script
    // reflects the HOST, so it must be gated on the actual TARGET instead.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        files.push("vendor/unrar/isnt.cpp".to_string());
    }
    cc::Build::new()
        .cpp(true) // Switch to C++ library compilation.
        .opt_level(2)
        .std("c++14")
        // by default cc crate tries to link against dynamic stdlib, which causes problems on windows-gnu target
        .cpp_link_stdlib(None)
        .warnings(false)
        .extra_warnings(false)
        .flag_if_supported("-stdlib=libc++")
        .flag_if_supported("-fPIC")
        .flag_if_supported("-Wno-switch")
        .flag_if_supported("-Wno-parentheses")
        .flag_if_supported("-Wno-macro-redefined")
        .flag_if_supported("-Wno-dangling-else")
        .flag_if_supported("-Wno-logical-op-parentheses")
        .flag_if_supported("-Wno-unused-parameter")
        .flag_if_supported("-Wno-unused-variable")
        .flag_if_supported("-Wno-unused-function")
        .flag_if_supported("-Wno-missing-braces")
        .flag_if_supported("-Wno-unknown-pragmas")
        .flag_if_supported("-Wno-deprecated-declarations")
        .define("_FILE_OFFSET_BITS", Some("64"))
        .define("_LARGEFILE_SOURCE", None)
        .define("RAR_SMP", None)
        .define("RARDLL", None)
        .files(&files)
        .compile("libunrar.a");
}
