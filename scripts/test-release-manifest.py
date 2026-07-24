#!/usr/bin/env python3
"""Exercise the complete release manifest contract with synthetic assets."""

import json
import subprocess
import tempfile
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
VERSION = "9.8.7"
TAG = f"v{VERSION}"
ASSETS = (
    f"alex-cli-{VERSION}-macos-aarch64.tar.gz",
    f"alex-cli-{VERSION}-macos-x86_64.tar.gz",
    f"alex-cli-{VERSION}-linux-x86_64.tar.gz",
    f"alex-cli-{VERSION}-linux-aarch64.tar.gz",
    f"alex-cli-{VERSION}-linux-x86_64-musl.tar.gz",
    f"alex-cli-{VERSION}-linux-aarch64-musl.tar.gz",
    f"alex-cli-{VERSION}-windows-x86_64.zip",
    f"alex-cli-{VERSION}-windows-arm64.zip",
    f"Alex-{VERSION}.dmg",
)


def run(*args: str) -> None:
    subprocess.run(args, cwd=ROOT, check=True)


def main() -> None:
    with tempfile.TemporaryDirectory(prefix="alex-release-manifest-") as raw:
        work = Path(raw)
        assets = work / "assets"
        assets.mkdir()
        for index, name in enumerate(ASSETS, start=1):
            (assets / name).write_bytes(f"synthetic asset {index}\n".encode())

        # Raw static binaries remain release conveniences for curl/bootstrap,
        # but the updater must select the tar archives it knows how to unpack.
        (assets / "alex-x86_64-unknown-linux-musl").write_bytes(b"raw x86\n")
        (assets / "alex-aarch64-unknown-linux-musl").write_bytes(b"raw arm\n")

        manifest = work / "manifest.json"
        run(
            "python3",
            "scripts/gen-manifest.py",
            "--tag",
            TAG,
            "--repo",
            "example/alex",
            "--assets-dir",
            str(assets),
            "--out",
            str(manifest),
        )
        run(
            "python3",
            "scripts/verify-release-manifest.py",
            "--manifest",
            str(manifest),
            "--assets-dir",
            str(assets),
            "--tag",
            TAG,
            "--repo",
            "example/alex",
        )

        platforms = json.loads(manifest.read_text())["components"]["cli"]["platforms"]
        expected_linux = {
            "x86_64-unknown-linux-gnu",
            "aarch64-unknown-linux-gnu",
            "x86_64-unknown-linux-musl",
            "aarch64-unknown-linux-musl",
        }
        if expected_linux - platforms.keys():
            raise SystemExit("generated manifest is missing a Linux platform")
        for key in ("x86_64-unknown-linux-musl", "aarch64-unknown-linux-musl"):
            if not platforms[key]["url"].endswith("-musl.tar.gz"):
                raise SystemExit(f"{key} does not reference an updater-compatible archive")

    print("release manifest contract passed")


if __name__ == "__main__":
    main()
