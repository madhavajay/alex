#!/usr/bin/env bash
set -euo pipefail

CALLER_PWD="$PWD"
DMG_INPUT="${1:-}"
if [[ -n "$DMG_INPUT" && "$DMG_INPUT" != /* ]]; then
  DMG_INPUT="$CALLER_PWD/$DMG_INPUT"
fi

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$ROOT"
. "$ROOT/scripts/lib/install-common.sh"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "install-dmg.sh only runs on macOS." >&2
  exit 1
fi

APP_NAME="${APP_NAME:-Alex.app}"
INSTALL_DIR="${INSTALL_DIR:-/Applications}"
DMG_PATH="${DMG_INPUT:-$(ls -t macos/dist/*.dmg 2>/dev/null | head -1 || true)}"

if [[ -z "$DMG_PATH" || ! -f "$DMG_PATH" ]]; then
  echo "No DMG found. Run ./build-signed.sh first, or pass a DMG path." >&2
  exit 1
fi

trap install_common_cleanup_mount EXIT

echo "Using DMG: $DMG_PATH"
quit_alex_apps
install_app_from_dmg "$DMG_PATH" "$APP_NAME" "$INSTALL_DIR"
echo "Installed $APP_NAME to $INSTALL_DIR"
