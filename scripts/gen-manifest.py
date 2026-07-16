#!/usr/bin/env python3
import argparse
import fnmatch
import hashlib
import json
from datetime import datetime, timezone
from pathlib import Path
from urllib.parse import quote


PATTERNS = [
    ("cli", "aarch64-apple-darwin", "alex-cli-*-macos-aarch64.tar.gz"),
    ("cli", "x86_64-apple-darwin", "alex-cli-*-macos-x86_64.tar.gz"),
    ("cli", "x86_64-unknown-linux-gnu", "alex-cli-*-linux-x86_64.tar.gz"),
    ("cli", "x86_64-pc-windows-msvc", "alex-cli-*-windows-x86_64.zip"),
    ("app", "darwin-universal", "Alex-*.dmg"),
]

COMPONENT_ORDER = ("cli", "app")
PLATFORM_ORDER = (
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "x86_64-unknown-linux-gnu",
    "x86_64-pc-windows-msvc",
    "darwin-universal",
)


def parse_args():
    parser = argparse.ArgumentParser(
        description="Generate the Alexandria release manifest.json from release assets."
    )
    parser.add_argument("--tag", required=True, help="Release tag, for example v0.1.17")
    parser.add_argument("--repo", default="madhavajay/alex", help="GitHub repo owner/name")
    parser.add_argument("--assets-dir", help="Directory containing downloaded release assets")
    parser.add_argument(
        "--asset",
        action="append",
        default=[],
        metavar="NAME=PATH",
        help="Additional asset file to include with an explicit release asset name",
    )
    parser.add_argument("--merge-existing", help="Existing manifest.json to merge from")
    parser.add_argument("--out", required=True, help="Output manifest.json path")
    return parser.parse_args()


def version_from_tag(tag):
    return tag[1:] if tag.startswith("v") else tag


def release_url(repo, tag, name):
    return f"https://github.com/{repo}/releases/download/{tag}/{quote(name)}"


def asset_digest(path):
    digest = hashlib.sha256()
    with path.open("rb") as fh:
        for chunk in iter(lambda: fh.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def classify_asset(name):
    if name.endswith(".sha256") or name == "manifest.json":
        return None
    for component, platform, pattern in PATTERNS:
        if fnmatch.fnmatchcase(name, pattern):
            return component, platform
    return None


def iter_assets(args):
    seen = set()
    if args.assets_dir:
        assets_dir = Path(args.assets_dir)
        for path in sorted(assets_dir.iterdir(), key=lambda item: item.name):
            if path.is_file():
                seen.add(path.name)
                yield path.name, path

    for asset in args.asset:
        if "=" not in asset:
            raise SystemExit(f"--asset must be NAME=PATH: {asset}")
        name, raw_path = asset.split("=", 1)
        if not name:
            raise SystemExit(f"--asset name is empty: {asset}")
        path = Path(raw_path)
        if name not in seen:
            yield name, path


def empty_manifest():
    return {"schema_version": 1, "published_at": "", "components": {}}


def load_manifest(path):
    if not path:
        return empty_manifest()
    manifest_path = Path(path)
    if not manifest_path.exists():
        return empty_manifest()
    with manifest_path.open("r", encoding="utf-8") as fh:
        data = json.load(fh)
    if not isinstance(data, dict):
        raise SystemExit(f"{path} is not a JSON object")
    data.setdefault("components", {})
    return data


def ensure_component(manifest, component, version):
    components = manifest.setdefault("components", {})
    entry = components.setdefault(component, {})
    entry["version"] = version
    if component == "cli":
        entry["notes_url"] = f"https://github.com/{manifest['_repo']}/releases/tag/{manifest['_tag']}"
    elif component == "app":
        entry["appcast"] = "https://madhavajay.github.io/alex/appcast.xml"
    entry.setdefault("platforms", {})
    return entry


def normalize_manifest(raw):
    out = {
        "schema_version": raw.get("schema_version", 1),
        "published_at": raw.get("published_at", ""),
        "components": {},
    }

    components = raw.get("components", {})
    for component in COMPONENT_ORDER:
        if component not in components:
            continue
        source = components[component]
        entry = {}
        for key in ("version", "notes_url", "appcast"):
            if key in source:
                entry[key] = source[key]
        platforms = source.get("platforms", {})
        ordered_platforms = {}
        for platform in PLATFORM_ORDER:
            if platform in platforms:
                ordered_platforms[platform] = platforms[platform]
        for platform in sorted(platforms):
            if platform not in ordered_platforms:
                ordered_platforms[platform] = platforms[platform]
        entry["platforms"] = ordered_platforms
        out["components"][component] = entry

    for component in sorted(components):
        if component not in out["components"]:
            out["components"][component] = components[component]

    return out


def main():
    args = parse_args()
    version = version_from_tag(args.tag)
    manifest = load_manifest(args.merge_existing)
    manifest["_repo"] = args.repo
    manifest["_tag"] = args.tag
    manifest["schema_version"] = 1
    manifest["published_at"] = (
        datetime.now(timezone.utc).replace(microsecond=0).isoformat().replace("+00:00", "Z")
    )

    for name, path in iter_assets(args):
        classification = classify_asset(name)
        if classification is None:
            continue
        if not path.is_file():
            raise SystemExit(f"Asset path does not exist or is not a file: {path}")

        component, platform = classification
        entry = ensure_component(manifest, component, version)
        entry["platforms"][platform] = {
            "url": release_url(args.repo, args.tag, name),
            "sha256": asset_digest(path),
            "size": path.stat().st_size,
        }

    manifest.pop("_repo", None)
    manifest.pop("_tag", None)
    output = normalize_manifest(manifest)

    out_path = Path(args.out)
    out_path.parent.mkdir(parents=True, exist_ok=True)
    with out_path.open("w", encoding="utf-8") as fh:
        json.dump(output, fh, indent=2)
        fh.write("\n")


if __name__ == "__main__":
    main()
