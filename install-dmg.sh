#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$ROOT"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "install-dmg.sh only runs on macOS." >&2
  exit 1
fi

APP_NAME="${APP_NAME:-Alex.app}"
LEGACY_APP_STEM="AlexandriaBar"
LEGACY_APP_NAME="${LEGACY_APP_STEM}.app"
INSTALL_DIR="${INSTALL_DIR:-/Applications}"
DMG_PATH="${1:-$(ls -t macos/dist/*.dmg 2>/dev/null | head -1 || true)}"

if [[ -z "$DMG_PATH" || ! -f "$DMG_PATH" ]]; then
  echo "No DMG found. Run ./build-signed.sh first, or pass a DMG path." >&2
  exit 1
fi

MOUNT_POINT="$(mktemp -d "${TMPDIR:-/tmp}/alexandria-install-XXXXXX")"
cleanup() {
  hdiutil detach "$MOUNT_POINT" >/dev/null 2>&1 || true
  rmdir "$MOUNT_POINT" >/dev/null 2>&1 || true
}
trap cleanup EXIT

echo "Using DMG: $DMG_PATH"
hdiutil attach "$DMG_PATH" -nobrowse -mountpoint "$MOUNT_POINT" -quiet

for app in AlexandriaBar Alex; do
  osascript -e "tell application \"$app\" to quit" >/dev/null 2>&1 || true
done
pkill -x AlexandriaBar 2>/dev/null || true
pkill -x Alex 2>/dev/null || true

if [[ ! -d "$MOUNT_POINT/$APP_NAME" ]]; then
  echo "App not found in mounted DMG: $APP_NAME" >&2
  exit 1
fi

mkdir -p "$INSTALL_DIR"
if [[ -d "$INSTALL_DIR/$APP_NAME" ]]; then
  rm -rf "$INSTALL_DIR/$APP_NAME"
fi

ditto "$MOUNT_POINT/$APP_NAME" "$INSTALL_DIR/$APP_NAME"
rm -rf "$INSTALL_DIR/$LEGACY_APP_NAME"
echo "Installed $APP_NAME to $INSTALL_DIR"
