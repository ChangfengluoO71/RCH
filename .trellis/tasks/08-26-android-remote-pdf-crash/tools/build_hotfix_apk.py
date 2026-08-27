from __future__ import annotations

import argparse
import os
from pathlib import Path
import subprocess
import sys
import time
import zipfile

import requests


BASE_APK_URL = "https://github.com/ChangfengluoO71/RCH/releases/download/v0.5.5/app-arm64-v8a-release.apk"
TARGET = "aarch64-linux-android"
EXPECTED_BASE_APK_SIZE = 42_235_635


def load_vs_environment(temp_dir: Path) -> dict[str, str]:
    vsdevcmd = Path(r"C:/Program Files (x86)/Microsoft Visual Studio/2022/BuildTools/Common7/Tools/VsDevCmd.bat")
    launcher = temp_dir / "vs-env.cmd"
    launcher.write_text(
        "@echo off\r\n"
        f'call "{vsdevcmd}" -arch=x64 -host_arch=x64 >nul\r\n'
        "set\r\n",
        encoding="utf-8",
    )
    raw = subprocess.check_output(["cmd.exe", "/d", "/c", str(launcher)])
    env = os.environ.copy()
    for line in raw.decode("mbcs", "replace").splitlines():
        if "=" in line:
            key, value = line.split("=", 1)
            env[key] = value
    return env


def build_rust(root: Path, out_dir: Path) -> Path:
    sdk = Path(r"C:/Users/cfl/AppData/Local/Android/Sdk")
    ndk_bin = sdk / "ndk" / "28.2.13676358" / "toolchains" / "llvm" / "prebuilt" / "windows-x86_64" / "bin"
    rust_dir = root / "app" / "rust"
    env = load_vs_environment(out_dir)
    env["TEMP"] = str(out_dir)
    env["TMP"] = str(out_dir)

    libgcc = out_dir / "libgcc"
    libgcc.mkdir(exist_ok=True)
    (libgcc / "libgcc.a").write_text("INPUT(-lunwind)", encoding="ascii")

    env[f"AR_{TARGET}"] = str(ndk_bin / "llvm-ar.exe")
    env[f"CC_{TARGET}"] = str(ndk_bin / "aarch64-linux-android24-clang.cmd")
    env[f"CXX_{TARGET}"] = str(ndk_bin / "aarch64-linux-android24-clang++.cmd")
    env[f"RANLIB_{TARGET}"] = str(ndk_bin / "llvm-ranlib.exe")
    env["CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER"] = str(ndk_bin / "aarch64-linux-android24-clang.cmd")
    env["CARGO_ENCODED_RUSTFLAGS"] = f"-L\x1f{libgcc}\x1f-Clinker-flavor=gcc"

    target_dir = out_dir / "target"
    cmd = [
        "rustup", "run", "stable", "cargo", "build",
        "--release", "--offline", "--locked",
        "--manifest-path", str(rust_dir / "Cargo.toml"),
        "-p", "rust_lib_app",
        "--target", TARGET,
        "--target-dir", str(target_dir),
    ]
    subprocess.run(cmd, cwd=rust_dir, env=env, check=True)
    so = target_dir / TARGET / "release" / "librust_lib_app.so"
    if not so.exists():
        raise RuntimeError("release Rust library was not produced")
    print(f"RUST_SO={so}", flush=True)
    return so


def download_base_apk(path: Path) -> None:
    headers = {"User-Agent": "Mozilla/5.0 RCH-hotfix-builder/3"}
    last_error: Exception | None = None
    for attempt in range(1, 6):
        try:
            if path.exists():
                path.unlink()
            print(f"BASE_APK_DOWNLOAD_ATTEMPT={attempt}", flush=True)
            with requests.get(
                BASE_APK_URL,
                headers=headers,
                stream=True,
                timeout=(20, 60),
                allow_redirects=True,
            ) as response:
                response.raise_for_status()
                with path.open("wb") as output:
                    for chunk in response.iter_content(1024 * 1024):
                        if chunk:
                            output.write(chunk)
            size = path.stat().st_size
            print(f"BASE_APK_SIZE={size}", flush=True)
            if size != EXPECTED_BASE_APK_SIZE:
                raise RuntimeError(
                    f"unexpected base APK size: {size} != {EXPECTED_BASE_APK_SIZE}"
                )
            return
        except Exception as exc:
            last_error = exc
            print(f"BASE_APK_DOWNLOAD_ERR attempt={attempt} err={exc}", flush=True)
            if path.exists():
                path.unlink()
            if attempt < 5:
                time.sleep(attempt * 2)
    raise RuntimeError("failed to download v0.5.5 arm64 base APK after 5 attempts") from last_error


def patch_apk(base_apk: Path, rust_so: Path, unsigned_apk: Path) -> None:
    target_entry = "lib/arm64-v8a/librust_lib_app.so"
    replaced = False
    with zipfile.ZipFile(base_apk, "r") as zin, zipfile.ZipFile(unsigned_apk, "w", allowZip64=True) as zout:
        for info in zin.infolist():
            upper = info.filename.upper()
            if upper.startswith("META-INF/") and upper.endswith((".RSA", ".DSA", ".EC", ".SF", ".MF")):
                continue
            if info.filename == target_entry:
                zout.writestr(info, rust_so.read_bytes())
                replaced = True
            else:
                zout.writestr(info, zin.read(info))
    if not replaced:
        raise RuntimeError(f"{target_entry} not found in base APK")
    print(f"UNSIGNED_APK={unsigned_apk}", flush=True)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--keepalive-seconds", type=int, default=0)
    args = parser.parse_args()

    root = Path(__file__).resolve().parents[4]
    out_dir = root / ".cwapi-hotfix"
    out_dir.mkdir(exist_ok=True)
    base_apk = out_dir / "app-arm64-v8a-v0.5.5.apk"
    unsigned_apk = out_dir / "RCH-pdf-diag-unsigned.apk"
    signed_apk = out_dir / "RCH-pdf-diag-signed.apk"

    # Network first: do not spend ~1-2 minutes compiling if GitHub download is unavailable.
    download_base_apk(base_apk)
    rust_so = build_rust(root, out_dir)
    patch_apk(base_apk, rust_so, unsigned_apk)

    signer = Path(__file__).with_name("sign_hotfix.py")
    subprocess.run([sys.executable, str(signer), str(unsigned_apk), str(signed_apk)], check=True)
    print(f"SIGNED_APK={signed_apk.resolve()}", flush=True)
    print(f"SIGNED_APK_SIZE={signed_apk.stat().st_size}", flush=True)
    print("HOTFIX_READY", flush=True)

    if args.keepalive_seconds > 0:
        print(f"KEEPALIVE_SECONDS={args.keepalive_seconds}", flush=True)
        time.sleep(args.keepalive_seconds)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
