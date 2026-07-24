#!/usr/bin/env python3
"""Fail closed unless a stable Alex manifest matches every packaged asset."""

import argparse
import hashlib
import json
from pathlib import Path
from urllib.parse import unquote, urlparse


REQUIRED = {
    ("cli", "aarch64-apple-darwin"): "alex-cli-{version}-macos-aarch64.tar.gz",
    ("cli", "x86_64-apple-darwin"): "alex-cli-{version}-macos-x86_64.tar.gz",
    ("cli", "x86_64-unknown-linux-gnu"): "alex-cli-{version}-linux-x86_64.tar.gz",
    ("cli", "aarch64-unknown-linux-gnu"): "alex-cli-{version}-linux-aarch64.tar.gz",
    ("cli", "x86_64-unknown-linux-musl"): "alex-cli-{version}-linux-x86_64-musl.tar.gz",
    ("cli", "aarch64-unknown-linux-musl"): "alex-cli-{version}-linux-aarch64-musl.tar.gz",
    ("cli", "x86_64-pc-windows-msvc"): "alex-cli-{version}-windows-x86_64.zip",
    ("cli", "aarch64-pc-windows-msvc"): "alex-cli-{version}-windows-arm64.zip",
    ("app", "darwin-universal"): "Alex-{version}.dmg",
}


def parse_args():
    parser = argparse.ArgumentParser()
    parser.add_argument("--manifest", required=True, type=Path)
    parser.add_argument("--assets-dir", required=True, type=Path)
    parser.add_argument("--tag", required=True)
    parser.add_argument("--repo", required=True)
    return parser.parse_args()


def digest(path):
    value = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            value.update(chunk)
    return value.hexdigest()


def fail(message):
    raise SystemExit(f"release manifest verification failed: {message}")


def main():
    args = parse_args()
    with args.manifest.open("r", encoding="utf-8") as stream:
        manifest = json.load(stream)

    if manifest.get("schema_version") != 1:
        fail("schema_version is not 1")

    expected_version = args.tag.removeprefix("v")
    components = manifest.get("components", {})
    present = {
        (component, platform)
        for component, entry in components.items()
        for platform in entry.get("platforms", {})
    }
    missing = sorted(set(REQUIRED) - present)
    if missing:
        fail(f"missing required component/platform entries: {missing}")

    expected_prefix = f"https://github.com/{args.repo}/releases/download/{args.tag}/"
    for component, platform in sorted(REQUIRED):
        component_entry = components[component]
        if component_entry.get("version") != expected_version:
            fail(f"{component} version is not {expected_version}")
        entry = component_entry["platforms"][platform]
        url = entry.get("url", "")
        if not url.startswith(expected_prefix):
            fail(f"unexpected URL for {component}/{platform}: {url}")
        name = unquote(Path(urlparse(url).path).name)
        expected_name = REQUIRED[(component, platform)].format(version=expected_version)
        if name != expected_name:
            fail(f"unexpected asset for {component}/{platform}: {name}")
        asset = args.assets_dir / name
        if not asset.is_file():
            fail(f"asset referenced by {component}/{platform} is missing: {name}")
        if entry.get("size") != asset.stat().st_size:
            fail(f"size mismatch for {name}")
        if entry.get("sha256") != digest(asset):
            fail(f"SHA-256 mismatch for {name}")

    print(f"verified {len(REQUIRED)} stable release assets against {args.manifest}")


if __name__ == "__main__":
    main()
