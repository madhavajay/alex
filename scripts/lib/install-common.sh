#!/bin/sh
# Shared installer helpers. Keep this file POSIX sh so every installer can source it.

sha256_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | awk '{print $1}'
  else
    printf '%s\n' "A SHA-256 tool (sha256sum or shasum) is required." >&2
    exit 1
  fi
}

quit_alex_apps() {
  wait_steps="${1:-0}"
  for app in Alex; do
    osascript -e "tell application \"$app\" to quit" >/dev/null 2>&1 || true
  done
  for process in Alex; do
    if pgrep -x "$process" >/dev/null 2>&1; then
      printf 'Quitting the running %s...\n' "$process"
      waited=0
      while pgrep -x "$process" >/dev/null 2>&1 && [ "$waited" -lt "$wait_steps" ]; do
        sleep 0.5
        waited=$((waited + 1))
      done
      if pgrep -x "$process" >/dev/null 2>&1; then
        pkill -x "$process" >/dev/null 2>&1 || true
      fi
    fi
  done
}

INSTALL_COMMON_MOUNT_POINT=""

install_common_cleanup_mount() {
  if [ -n "${INSTALL_COMMON_MOUNT_POINT:-}" ]; then
    hdiutil detach "$INSTALL_COMMON_MOUNT_POINT" -quiet >/dev/null 2>&1 || true
    rmdir "$INSTALL_COMMON_MOUNT_POINT" >/dev/null 2>&1 || true
    INSTALL_COMMON_MOUNT_POINT=""
  fi
}

install_app_from_dmg() {
  dmg_path="$1"
  app_name="$2"
  install_dir="$3"

  [ -n "$install_dir" ] || {
    printf '%s\n' "The app install directory is empty." >&2
    return 1
  }
  INSTALL_COMMON_MOUNT_POINT="$(mktemp -d "${TMPDIR:-/tmp}/alex-install-XXXXXX")"
  hdiutil attach "$dmg_path" -nobrowse -quiet -mountpoint "$INSTALL_COMMON_MOUNT_POINT" </dev/null

  if [ ! -d "$INSTALL_COMMON_MOUNT_POINT/$app_name" ]; then
    printf '%s\n' "$app_name was not found inside the DMG; the app was not installed." >&2
    return 1
  fi
  mkdir -p "$install_dir"
  if [ ! -w "$install_dir" ]; then
    printf '%s\n' "$install_dir is not writable. Re-run as a user who can write to it." >&2
    return 1
  fi

  rm -rf "$install_dir/$app_name"
  ditto "$INSTALL_COMMON_MOUNT_POINT/$app_name" "$install_dir/$app_name"
  install_common_cleanup_mount
}
