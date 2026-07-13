#!/usr/bin/env bash
# Stamp a (pre)release version, e.g. 0.1.24-beta.1, into the workspace
# before building beta assets. Never commits — CI builds from the stamped
# tree and throws it away.
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: $0 <version>" >&2
  exit 1
fi

version="$1"
if [[ ! "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-beta\.[0-9]+)?$ ]]; then
  echo "invalid version '$version' (expected X.Y.Z or X.Y.Z-beta.N)" >&2
  exit 1
fi

current=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
if [[ -z "$current" ]]; then
  echo "could not read current workspace version from Cargo.toml" >&2
  exit 1
fi

if [[ "$current" == "$version" ]]; then
  echo "workspace already at ${version}"
  exit 0
fi

# perl -pi is portable across GNU (Linux CI) and BSD (macOS CI) userlands
perl -pi -e "s/^version = \"\Q${current}\E\"/version = \"${version}\"/" Cargo.toml
perl -pi -e "s/version = \"\Q${current}\E\" \}/version = \"${version}\" }/g" Cargo.toml
cargo update --workspace --offline 2>/dev/null || cargo update --workspace
echo "stamped ${current} -> ${version}"
