# 重新生成 FRB 绑定，并重建 Rust release 库。
#
# 背景：FRB 默认加载配置 ioDirectory: 'rust/target/release/'，flutter run 启动时优先
# 加载 rust/target/release/rust_lib_app.dll（相对 app 目录）。绑定重新生成后必须同步
# 重建该 DLL，否则启动时 content hash 校验报错。
# 以后修改 Rust API 后统一运行本脚本，而不是只跑 codegen。
$ErrorActionPreference = 'Stop'
$appDir = Split-Path -Parent $MyInvocation.MyCommand.Path

# RCH 正在运行时进程内仍持有旧版 Rust 代码，重新生成绑定后再热重启会触发
# content hash 校验失败，因此先提示关闭应用再执行。
if (Get-Process -Name RCH -ErrorAction SilentlyContinue) {
    Write-Warning '检测到 RCH 正在运行（占用 rust/target/release/rust_lib_app.dll），请先关闭应用再运行本脚本。'
    exit 1
}

Push-Location $appDir
try {
    flutter_rust_bridge_codegen generate
    if ($LASTEXITCODE -ne 0) { throw 'flutter_rust_bridge_codegen generate 失败' }

    Push-Location (Join-Path $appDir 'rust')
    try {
        cargo build --release
        if ($LASTEXITCODE -ne 0) { throw 'cargo build --release 失败' }
    } finally {
        Pop-Location
    }

    Write-Host '完成：绑定已重新生成，rust/target/release/rust_lib_app.dll 已重建。'
} finally {
    Pop-Location
}
