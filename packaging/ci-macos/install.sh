#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
if [[ -f "$SCRIPT_DIR/install-common.sh" ]]; then
  . "$SCRIPT_DIR/install-common.sh"
else
  . "$SCRIPT_DIR/../../scripts/lib/install-common.sh"
fi
APP_SOURCE="$SCRIPT_DIR/Alex.app"
BIN_SOURCE="$SCRIPT_DIR/bin"
APP_DEST="${ALEXANDRIA_APP_DEST:-/Applications/Alex.app}"
LEGACY_APP_STEM="AlexandriaBar"
LEGACY_APP_DEST="$(dirname "$APP_DEST")/${LEGACY_APP_STEM}.app"
APP_BACKUP="${APP_DEST%.app}.pre-alexandria-ci"
BIN_DEST="${ALEXANDRIA_BIN_DEST:-$HOME/.local/bin}"

usage() {
  cat <<'EOF'
Usage: ./install.sh [--restore]

With no option, installs this CI build, registers/restarts the user daemon,
and opens Alex. --restore puts back the app and binaries saved by
the first CI install.
EOF
}

run_for_applications() {
  if [[ -w "$(dirname "$APP_DEST")" ]]; then
    "$@"
  else
    sudo "$@"
  fi
}

restart_service() {
  "$BIN_DEST/alex" service install
}

restore_previous() {
  quit_alex_apps
  if [[ ! -e "$APP_BACKUP" ]]; then
    echo "No saved app exists at $APP_BACKUP" >&2
    exit 1
  fi

  run_for_applications rm -rf "$APP_DEST"
  run_for_applications mv "$APP_BACKUP" "$APP_DEST"

  restored=0
  for name in alex alexandria; do
    backup="$BIN_DEST/$name.pre-alexandria-ci"
    if [[ -f "$backup" ]]; then
      mv "$backup" "$BIN_DEST/$name"
      chmod +x "$BIN_DEST/$name"
      restored=1
    fi
  done
  if [[ "$restored" -eq 0 ]]; then
    echo "The previous app was restored, but no saved CLI binaries were found." >&2
    echo "Reinstall the release CLI before restarting the daemon." >&2
    exit 1
  fi

  restart_service
  open "$APP_DEST"
  echo "Restored the pre-CI Alexandria app and daemon."
}

install_ci_build() {
  if [[ "$(uname -s)" != "Darwin" ]]; then
    echo "This test bundle is for macOS only." >&2
    exit 1
  fi
  if [[ ! -d "$APP_SOURCE" || ! -x "$BIN_SOURCE/alex" || ! -x "$BIN_SOURCE/alexandria" ]]; then
    echo "The CI bundle is incomplete; keep install.sh beside Alex.app and bin/." >&2
    exit 1
  fi

  quit_alex_apps
  mkdir -p "$BIN_DEST"

  if [[ -d "$APP_DEST" && ! -e "$APP_BACKUP" ]]; then
    run_for_applications mv "$APP_DEST" "$APP_BACKUP"
  elif [[ -e "$APP_DEST" ]]; then
    run_for_applications rm -rf "$APP_DEST"
  fi

  for name in alex alexandria; do
    if [[ -f "$BIN_DEST/$name" && ! -e "$BIN_DEST/$name.pre-alexandria-ci" ]]; then
      cp -p "$BIN_DEST/$name" "$BIN_DEST/$name.pre-alexandria-ci"
    fi
  done

  run_for_applications ditto "$APP_SOURCE" "$APP_DEST"
  remove_legacy_app "$(dirname "$APP_DEST")" "$(basename "$APP_DEST")" "$(basename "$LEGACY_APP_DEST")"
  run_for_applications xattr -dr com.apple.quarantine "$APP_DEST" 2>/dev/null || true
  cp "$BIN_SOURCE/alex" "$BIN_SOURCE/alexandria" "$BIN_DEST/"
  chmod +x "$BIN_DEST/alex" "$BIN_DEST/alexandria"
  xattr -d com.apple.quarantine "$BIN_DEST/alex" "$BIN_DEST/alexandria" 2>/dev/null || true

  restart_service
  open "$APP_DEST"

  echo
  "$BIN_DEST/alex" --version
  echo "Installed the CI app at $APP_DEST"
  echo "Installed the CI daemon/CLI in $BIN_DEST"
  echo "Your existing ~/.alexandria configuration and accounts were preserved."
  echo "Run '$SCRIPT_DIR/install.sh --restore' to switch back to the saved build."
}

case "${1:-}" in
  "") install_ci_build ;;
  --restore) restore_previous ;;
  -h|--help) usage ;;
  *) usage >&2; exit 2 ;;
esac
