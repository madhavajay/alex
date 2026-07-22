#!/usr/bin/env bash
# CI recipe:
# jobs:
#   harness-mock-weekly:
#     runs-on: ubuntu-latest
#     steps:
#       - uses: actions/checkout@v4
#       - uses: actions/setup-node@v4
#         with:
#           node-version: 22
#       - uses: dtolnay/rust-toolchain@stable
#       - run: cargo build -p alex --bin alex -p alex-fakeprov --bin alex-fakeprov
#       - run: scripts/bump-harness-versions.sh
#       - run: ./test.sh harness-mock
#       - run: git diff -- crates/alex/config/harnesses.json

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CONFIG="${ALEX_BUMP_HARNESS_CONFIG:-$ROOT/crates/alex/config/harnesses.json}"
CHECK=0
PACK=0

usage() {
  cat <<'EOF'
Usage: scripts/bump-harness-versions.sh [--check] [--pack]

  --check  print current -> latest and exit 1 when any package is outdated
  --pack   after writing bumped versions, run alex harness pack for each bump
EOF
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --check) CHECK=1 ;;
    --pack) PACK=1 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown argument: $1" >&2; usage >&2; exit 2 ;;
  esac
  shift
done

if [ ! -f "$CONFIG" ]; then
  echo "harness catalog not found: $CONFIG" >&2
  exit 2
fi

rows_file="$(mktemp "${TMPDIR:-/tmp}/alex-harness-packages.XXXXXX")"
latest_file="$(mktemp "${TMPDIR:-/tmp}/alex-harness-latest.XXXXXX")"
trap 'rm -f "$rows_file" "$latest_file"' EXIT

python3 - "$CONFIG" >"$rows_file" <<'PY'
import json, sys

with open(sys.argv[1], encoding="utf-8") as fh:
    catalog = json.load(fh)

for name, spec in catalog.get("harnesses", {}).items():
    source = spec.get("source") or {}
    package = source.get("package")
    current = spec.get("default_version")
    if package and current:
        if current.startswith(package + "@"):
            current = current[len(package) + 1:]
        print("\t".join(["package", name, package, current]))
    elif package:
        print("\t".join(["skip", name, "-", "no default_version"]))
    else:
        print("\t".join(["skip", name, "-", "no package source"]))
PY

urlencode() {
  python3 - "$1" <<'PY'
import sys, urllib.parse
print(urllib.parse.quote(sys.argv[1], safe="@"))
PY
}

latest_for() {
  local package=$1 encoded payload file
  encoded=$(urlencode "$package")
  if [ -n "${ALEX_BUMP_REGISTRY_DIR:-}" ]; then
    file="$ALEX_BUMP_REGISTRY_DIR/$encoded.json"
    [ -f "$file" ] || file="$ALEX_BUMP_REGISTRY_DIR/${package##*/}.json"
    [ -f "$file" ] || { echo "missing registry fixture for $package" >&2; return 1; }
    payload=$(cat "$file")
  else
    payload=$(curl -fsSL "https://registry.npmjs.org/$encoded/latest")
  fi
  python3 -c 'import json,sys; print(json.load(sys.stdin)["version"])' <<<"$payload"
}

printf '%-22s %-34s %-18s %-18s %s\n' "harness" "package" "current" "latest" "status"
outdated=0
while IFS=$'\t' read -r kind name package current; do
  if [ "$kind" = "skip" ]; then
    printf '%-22s %-34s %-18s %-18s %s\n' "$name" "-" "-" "-" "$current"
    continue
  fi
  latest=$(latest_for "$package")
  status="current"
  if [ "$current" != "$latest" ]; then
    status="outdated"
    outdated=1
  fi
  printf '%-22s %-34s %-18s %-18s %s\n' "$name" "$package" "$current" "$latest" "$status"
  printf '%s\t%s\t%s\t%s\n' "$name" "$package" "$current" "$latest" >>"$latest_file"
done <"$rows_file"

if [ "$CHECK" -eq 1 ]; then
  exit "$outdated"
fi

if [ "$outdated" -eq 0 ]; then
  echo "all packageable harness versions are current"
  exit 0
fi

python3 - "$CONFIG" "$latest_file" <<'PY'
import json, re, sys
from collections import OrderedDict

config_path, latest_path = sys.argv[1], sys.argv[2]
raw = open(config_path, encoding="utf-8").read()
catalog = json.loads(raw, object_pairs_hook=OrderedDict)
updates = {}
for line in open(latest_path, encoding="utf-8"):
    name, package, current, latest = line.rstrip("\n").split("\t")
    if current != latest:
        updates[name] = latest

for name, latest in updates.items():
    catalog["harnesses"][name]["default_version"] = latest

json.loads(json.dumps(catalog))
updated = raw
for name, latest in updates.items():
    current = None
    for line in open(latest_path, encoding="utf-8"):
        parts = line.rstrip("\n").split("\t")
        if parts[0] == name:
            current = parts[2]
            break
    if current is None:
        raise SystemExit(f"missing current version for {name}")
    section = re.search(
        r'(?P<prefix>\n    "' + re.escape(name) + r'": \{)(?P<body>.*?)(?=\n    "[^"]+": \{|\n  \}\n\})',
        updated,
        flags=re.S,
    )
    if not section:
        raise SystemExit(f"could not find harness section {name}")
    body = section.group("body")
    replaced, count = re.subn(
        r'("default_version"\s*:\s*")' + re.escape(current) + r'(")',
        r'\g<1>' + latest + r'\2',
        body,
        count=1,
    )
    if count != 1:
        raise SystemExit(f"could not replace default_version for {name}")
    updated = updated[:section.start("body")] + replaced + updated[section.end("body"):]

with open(config_path, "w", encoding="utf-8") as fh:
    fh.write(updated)
PY

echo "bumped harness versions:"
while IFS=$'\t' read -r name package current latest; do
  [ "$current" = "$latest" ] && continue
  echo "  $name: $current -> $latest ($package)"
  if [ "$PACK" -eq 1 ]; then
    if [ -x "$ROOT/target/debug/alex" ]; then
      "$ROOT/target/debug/alex" harness pack "$name" --version "$latest"
    else
      cargo run -q -p alex --bin alex -- harness pack "$name" --version "$latest"
    fi
  fi
done <"$latest_file"
