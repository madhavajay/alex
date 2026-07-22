#!/usr/bin/env bash
set -euo pipefail

err() { echo "ERROR: $*" >&2; }
info() { echo "INFO: $*"; }

CALLER_PWD="$PWD"
APP_INPUT="${1:-}"
if [[ -n "$APP_INPUT" && "$APP_INPUT" != /* ]]; then
  APP_INPUT="$CALLER_PWD/$APP_INPUT"
fi

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$ROOT"

if [[ "$(uname -s)" != "Darwin" ]]; then
  err "check-gatekeeper.sh only runs on macOS."
  exit 1
fi

MOUNT_POINT=""
APP_PATH=""
DMG_INPUT=""
Q_HITS=""
P_HITS=""
STATUS=""

cleanup() {
  if [[ -n "$MOUNT_POINT" && -d "$MOUNT_POINT" ]]; then
    hdiutil detach "$MOUNT_POINT" >/dev/null 2>&1 || true
    rmdir "$MOUNT_POINT" >/dev/null 2>&1 || true
  fi
  [[ -z "$Q_HITS" ]] || rm -f "$Q_HITS"
  [[ -z "$P_HITS" ]] || rm -f "$P_HITS"
  [[ -z "$STATUS" ]] || rm -f "$STATUS"
}
trap cleanup EXIT

pick_latest_artifact() {
  local dmg app
  dmg=$(ls -t macos/dist/*.dmg 2>/dev/null | head -1 || true)
  app=$(ls -td macos/dist/*.app 2>/dev/null | head -1 || true)

  if [[ -n "$dmg" ]]; then
    APP_INPUT="$ROOT/$dmg"
    return
  fi
  if [[ -n "$app" ]]; then
    APP_INPUT="$ROOT/$app"
    return
  fi

  err "No macos/dist .dmg or .app found; pass a path or build first."
  exit 1
}

mount_dmg() {
  local dmg="$1"
  DMG_INPUT="$dmg"
  MOUNT_POINT="$(mktemp -d "${TMPDIR:-/tmp}/alex-mnt-XXXXXX")"
  info "Mounting DMG: $dmg"
  hdiutil attach "$dmg" -nobrowse -mountpoint "$MOUNT_POINT" >/dev/null
  APP_PATH="$(find "$MOUNT_POINT" -maxdepth 2 -name "*.app" -type d | head -1 || true)"
  if [[ -z "$APP_PATH" ]]; then
    err "No .app found inside mounted DMG: $dmg"
    exit 1
  fi
}

if [[ -z "$APP_INPUT" ]]; then
  pick_latest_artifact
fi

if [[ -d "$APP_INPUT" && "$APP_INPUT" == *.app ]]; then
  APP_PATH="$APP_INPUT"
elif [[ -f "$APP_INPUT" && "$APP_INPUT" == *.dmg ]]; then
  mount_dmg "$APP_INPUT"
elif [[ -f "$APP_INPUT" ]]; then
  info "Checking standalone binary: $APP_INPUT"
  xattr -l "$APP_INPUT" 2>/dev/null | grep -E 'com.apple.quarantine|com.apple.provenance' || echo "No quarantine/provenance attributes."
  codesign -dv "$APP_INPUT"
  spctl --assess --type exec --verbose "$APP_INPUT"
  exit 0
else
  err "Unrecognized input: $APP_INPUT (expected .app, .dmg, or executable file)"
  exit 1
fi

scan_attrs() {
  local target="$1"
  local q_hits="$2"
  local p_hits="$3"
  local statuses="$4"

  find "$target" -type f -print0 | while IFS= read -r -d '' file; do
    local q="" p=""
    if xattr -p com.apple.quarantine "$file" >/dev/null 2>&1; then
      q="quarantine"
      echo "$file" >>"$q_hits"
    fi
    if xattr -p com.apple.provenance "$file" >/dev/null 2>&1; then
      p="provenance"
      echo "$file" >>"$p_hits"
    fi
    if [[ -n "$q$p" ]]; then
      echo "[FLAG] $file ${q:+$q }${p}" >>"$statuses"
    else
      echo "[OK]   $file" >>"$statuses"
    fi
  done
}

if [[ -n "$DMG_INPUT" ]]; then
  info "Checking DMG signature and Gatekeeper assessment..."
  codesign --verify --verbose=2 "$DMG_INPUT"
  spctl --assess --type open --context context:primary-signature --verbose "$DMG_INPUT"
fi

info "Checking app: $APP_PATH"
Q_HITS="$(mktemp)"
P_HITS="$(mktemp)"
STATUS="$(mktemp)"

if [[ -n "$DMG_INPUT" ]]; then
  scan_attrs "$DMG_INPUT" "$Q_HITS" "$P_HITS" "$STATUS"
fi
scan_attrs "$APP_PATH" "$Q_HITS" "$P_HITS" "$STATUS"

info "Per-file quarantine/provenance status:"
cat "$STATUS"

if [[ -s "$Q_HITS" ]]; then
  err "Found quarantine attributes:"
  cat "$Q_HITS"
  exit 1
fi

if [[ -s "$P_HITS" ]]; then
  info "Found provenance attributes (not fatal):"
  cat "$P_HITS"
fi

info "Verifying app codesign..."
codesign --verify --deep --strict --verbose=2 "$APP_PATH"

info "Assessing app with spctl..."
spctl --assess --type exec --verbose "$APP_PATH"

info "Gatekeeper checks passed."
