from __future__ import annotations

import argparse
import os
from pathlib import Path
import subprocess
import tempfile


def load_properties(path: Path) -> dict[str, str]:
    props: dict[str, str] = {}
    for raw in path.read_text(encoding="utf-8").splitlines():
        line = raw.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, value = line.split("=", 1)
        props[key.strip()] = value.strip()
    return props


def newest_build_tools(sdk: Path) -> Path:
    candidates = [
        p for p in (sdk / "build-tools").iterdir()
        if (p / "zipalign.exe").exists() and (p / "apksigner.bat").exists()
    ]
    if not candidates:
        raise RuntimeError("Android build-tools with zipalign/apksigner not found")
    return max(candidates, key=lambda p: tuple(int(x) if x.isdigit() else x for x in p.name.split(".")))


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("input_apk", type=Path)
    parser.add_argument("output_apk", type=Path)
    parser.add_argument(
        "--key-properties",
        type=Path,
        default=Path(r"D:/Projects/RCH-source/app/android/key.properties"),
    )
    parser.add_argument(
        "--sdk",
        type=Path,
        default=Path(r"C:/Users/cfl/AppData/Local/Android/Sdk"),
    )
    args = parser.parse_args()

    props = load_properties(args.key_properties)
    required = ["storeFile", "storePassword", "keyAlias", "keyPassword"]
    missing = [key for key in required if not props.get(key)]
    if missing:
        raise RuntimeError(f"missing signing properties: {', '.join(missing)}")

    store_file = (args.key_properties.parent / props["storeFile"]).resolve()
    build_tools = newest_build_tools(args.sdk)
    args.output_apk.parent.mkdir(parents=True, exist_ok=True)

    with tempfile.TemporaryDirectory(prefix="rch-hotfix-sign-") as tmp:
        aligned = Path(tmp) / "aligned.apk"
        subprocess.run(
            [str(build_tools / "zipalign.exe"), "-P", "16", "-f", "4", str(args.input_apk), str(aligned)],
            check=True,
        )

        env = os.environ.copy()
        env["RCH_KS_PASS"] = props["storePassword"]
        env["RCH_KEY_PASS"] = props["keyPassword"]
        subprocess.run(
            [
                "cmd.exe",
                "/d",
                "/c",
                str(build_tools / "apksigner.bat"),
                "sign",
                "--ks",
                str(store_file),
                "--ks-key-alias",
                props["keyAlias"],
                "--ks-pass",
                "env:RCH_KS_PASS",
                "--key-pass",
                "env:RCH_KEY_PASS",
                "--out",
                str(args.output_apk),
                str(aligned),
            ],
            check=True,
            env=env,
        )
        subprocess.run(
            ["cmd.exe", "/d", "/c", str(build_tools / "apksigner.bat"), "verify", "--verbose", str(args.output_apk)],
            check=True,
            env=env,
        )

    print(args.output_apk.resolve())
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
