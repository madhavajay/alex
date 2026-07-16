#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: ./install-macos.sh [--bar-only] [--no-bar] [...install.sh flags]

Installs the full Alexandria macOS experience:
  1. the alexandria binary + daemon      (delegates to ./install.sh)
  2. AlexandriaBar menu-bar app          (macos/, builds + installs to ~/Applications)

  --bar-only   only build/install the menu bar app
  --no-bar     only run install.sh (skip the menu bar app)
  everything else is passed through to ./install.sh
  (--service, --upgrade, --prefix DIR, --nosplash)
EOF
}

ROOT="$(cd "$(dirname "$0")" && pwd)"
cd "$ROOT"

if [ "$(uname)" != "Darwin" ]; then
  echo "install-macos.sh is for macOS — use ./install.sh on this platform" >&2
  exit 1
fi

BAR_ONLY=0
NO_BAR=0
PASS=()
for arg in "$@"; do
  case "$arg" in
    --bar-only) BAR_ONLY=1 ;;
    --no-bar) NO_BAR=1 ;;
    -h|--help) usage; exit 0 ;;
    *) PASS+=("$arg") ;;
  esac
done

if [ "$BAR_ONLY" = "0" ]; then
  ./install.sh ${PASS[@]+"${PASS[@]}"}
fi

if [ "$NO_BAR" = "0" ]; then
  LEGACY_APP_NAME="AlexandriaBar"
  echo "☥ building Alex (menu bar app)…"
  (cd macos && ./Scripts/package_app.sh)
  for app in AlexandriaBar Alex; do
    osascript -e "tell application \"$app\" to quit" >/dev/null 2>&1 || true
  done
  pkill -x AlexandriaBar 2>/dev/null || true
  pkill -x Alex 2>/dev/null || true
  mkdir -p ~/Applications
  rm -rf ~/Applications/Alex.app
  cp -R macos/dist/Alex.app ~/Applications/
  rm -rf ~/Applications/"${LEGACY_APP_NAME}.app"
  open ~/Applications/Alex.app
  echo "☥ Alex installed to ~/Applications and launched"
fi
