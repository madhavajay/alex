#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMP="$(mktemp -d "${TMPDIR:-/tmp}/alex-linux-installers.XXXXXX")"
trap 'rm -rf "$TMP"' EXIT

MOCK_BIN="$TMP/mock-bin"
ASSETS="$TMP/assets"
INSTALL_DIR="$TMP/install"
mkdir -p "$MOCK_BIN" "$ASSETS" "$INSTALL_DIR"

printf '%s\n' \
  '#!/bin/sh' \
  'case "$1" in' \
  '  -s) printf "Linux\n" ;;' \
  '  -m) printf "aarch64\n" ;;' \
  '  *) printf "Linux\n" ;;' \
  'esac' > "$MOCK_BIN/uname"
chmod +x "$MOCK_BIN/uname"
TEST_PATH="$MOCK_BIN:/usr/bin:/bin:/usr/sbin:/sbin"

sha256_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1"
  else
    shasum -a 256 "$1"
  fi
}

make_asset() {
  local version="$1" platform="$2" marker="$3"
  local stage="$TMP/stage-$marker"
  local asset="alex-cli-$version-$platform.tar.gz"
  mkdir -p "$stage"
  printf '#!/bin/sh\n# %s\nexit 0\n' "$marker" > "$stage/alex"
  chmod +x "$stage/alex"
  tar -C "$stage" -czf "$ASSETS/$asset" alex
  sha256_file "$ASSETS/$asset" > "$ASSETS/$asset.sha256"
}

assert_marker() {
  local marker="$1"
  grep -Fq "# $marker" "$INSTALL_DIR/alex" || {
    echo "installer did not select $marker" >&2
    exit 1
  }
}

stable_version="9.8.7"
beta_version="9.8.8-beta.1"
for libc in gnu musl; do
  suffix="linux-aarch64"
  [[ "$libc" == musl ]] && suffix="$suffix-musl"
  make_asset "$stable_version" "$suffix" "stable-$libc"
  make_asset "$beta_version" "$suffix" "beta-$libc"

  rm -f "$INSTALL_DIR/alex"
  PATH="$TEST_PATH" \
    ALEX_VERSION="$stable_version" \
    ALEX_ASSET_BASE_URL="file://$ASSETS" \
    ALEX_INSTALL_DIR="$INSTALL_DIR" \
    ALEX_LINUX_LIBC="$libc" \
    ALEX_NO_SERVICE=1 \
    "$ROOT/install-release.sh" >/dev/null
  assert_marker "stable-$libc"

  rm -f "$INSTALL_DIR/alex"
  PATH="$TEST_PATH" \
    ALEX_BETA_TAG="v$beta_version" \
    ALEX_ASSET_BASE_URL="file://$ASSETS" \
    ALEX_INSTALL_DIR="$INSTALL_DIR" \
    ALEX_LINUX_LIBC="$libc" \
    "$ROOT/install-beta.sh" >/dev/null
  assert_marker "beta-$libc"
done

raw_asset="alex-aarch64-unknown-linux-musl"
printf '#!/bin/sh\n# bootstrap-arm64-musl\nexit 0\n' > "$ASSETS/$raw_asset"
chmod +x "$ASSETS/$raw_asset"
raw_sha="$(sha256_file "$ASSETS/$raw_asset" | awk '{print $1}')"
printf '%s  %s\n' "$raw_sha" "$raw_asset" > "$ASSETS/checksums.txt"
rm -f "$INSTALL_DIR/alex"
PATH="$TEST_PATH" \
  ALEX_RELEASE_TAG="v$stable_version" \
  ALEX_ASSET_BASE_URL="file://$ASSETS" \
  ALEX_INSTALL_DIR="$INSTALL_DIR" \
  sh "$ROOT/up.sh" --no-launch >/dev/null
assert_marker "bootstrap-arm64-musl"

echo "Linux ARM64 GNU and musl installer selection passed"
