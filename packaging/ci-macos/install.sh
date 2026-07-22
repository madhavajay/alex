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
APP_DEST="${ALEX_APP_DEST:-/Applications/Alex.app}"
APP_BACKUP="${APP_DEST%.app}.pre-alex-ci"
BIN_DEST="${ALEX_BIN_DEST:-$HOME/.local/bin}"

usage() {
  cat <<'EOF'
Usage: ./install.sh [--restore] [--no-open] [--no-service]

With no option, installs this CI build, registers/restarts the user daemon,
and opens Alex. --restore puts back the app and binaries saved by
the first CI install.

--no-open       Do not launch the menu app after install or restore.
--no-service    Do not install/restart the user daemon after copying files.
EOF
}

ACTION="install"
OPEN_APP=1
MANAGE_SERVICE=1

while [[ "$#" -gt 0 ]]; do
  case "$1" in
    --restore)
      ACTION="restore"
      ;;
    --no-open)
      OPEN_APP=0
      ;;
    --no-service)
      MANAGE_SERVICE=0
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      usage >&2
      exit 2
      ;;
  esac
  shift
done

run_for_applications() {
  if [[ -w "$(dirname "$APP_DEST")" ]]; then
    "$@"
  else
    sudo "$@"
  fi
}

restart_service() {
  if [[ "$MANAGE_SERVICE" -eq 1 ]]; then
    "$BIN_DEST/alex" service install
  else
    echo "Skipped daemon registration (--no-service)."
  fi
}

open_app() {
  if [[ "$OPEN_APP" -eq 1 ]]; then
    open "$APP_DEST"
  else
    echo "Skipped app launch (--no-open)."
  fi
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
  for name in alex; do
    backup="$BIN_DEST/$name.pre-alex-ci"
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
  open_app
  echo "Restored the pre-CI Alex app and daemon."
}

install_ci_build() {
  if [[ "$(uname -s)" != "Darwin" ]]; then
    echo "This test bundle is for macOS only." >&2
    exit 1
  fi
  if [[ ! -d "$APP_SOURCE" || ! -x "$BIN_SOURCE/alex" ]]; then
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

  for name in alex; do
    if [[ -f "$BIN_DEST/$name" && ! -e "$BIN_DEST/$name.pre-alex-ci" ]]; then
      cp -p "$BIN_DEST/$name" "$BIN_DEST/$name.pre-alex-ci"
    fi
  done

  run_for_applications ditto "$APP_SOURCE" "$APP_DEST"
  run_for_applications xattr -dr com.apple.quarantine "$APP_DEST" 2>/dev/null || true
  cp "$BIN_SOURCE/alex" "$BIN_DEST/"
  chmod +x "$BIN_DEST/alex"
  xattr -d com.apple.quarantine "$BIN_DEST/alex" 2>/dev/null || true

  restart_service
  open_app

  echo
  "$BIN_DEST/alex" --version
  echo "Installed the CI app at $APP_DEST"
  echo "Installed the CI daemon/CLI in $BIN_DEST"
  echo "Your existing ~/.alex configuration and accounts were preserved."
  echo "Run '$SCRIPT_DIR/install.sh --restore' to switch back to the saved build."
}

case "$ACTION" in
  install) install_ci_build ;;
  restore) restore_previous ;;
esac
