#!/bin/sh
set -eu

# Bootstrap installer for the Alexandria BETA channel.
#
# The beta channel setting ships inside the beta build, so a stable install has
# no way to ask for one -- this script is the way in. Once it has run, the app's
# Preferences -> Updates -> Release channel picker and `alex update --set-channel
# beta` take over, and later betas arrive as ordinary updates.
#
# Unlike install-release.sh, this does not use Homebrew: there is no beta cask.
# It installs the CLI from the prerelease tarball and opens the signed DMG.

REPO="${ALEX_REPO:-madhavajay/alex}"
INSTALL_DIR="${ALEX_INSTALL_DIR:-$HOME/.local/bin}"
APP_DIR="${ALEX_APP_DIR:-/Applications}"
APP_NAME="AlexandriaBar.app"
APP_PROCESS="AlexandriaBar"

say() {
  printf '%s\n' "$*"
}

sha256_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | awk '{print $1}'
  else
    say "A SHA-256 tool (sha256sum or shasum) is required." >&2
    exit 1
  fi
}

# GitHub's releases/latest never points at a prerelease, so resolve the newest
# prerelease from the releases list instead. tag_name precedes prerelease within
# each release object, so remember the tag and print it when the flag turns true.
resolve_beta_tag() {
  if [ -n "${ALEX_BETA_TAG:-}" ]; then
    printf '%s\n' "$ALEX_BETA_TAG"
    return 0
  fi
  curl -fsSL "https://api.github.com/repos/$REPO/releases?per_page=30" | awk -F'"' '
    /"tag_name":/ { tag = $4 }
    /"draft":[[:space:]]*true/ { tag = "" }
    /"prerelease":[[:space:]]*true/ { if (tag != "") { print tag; exit } }
  '
}

platform_asset() {
  case "$(uname -s)" in
    Darwin)
      case "$(uname -m)" in
        arm64) printf 'macos-aarch64\n' ;;
        x86_64) printf 'macos-x86_64\n' ;;
        *) say "No Alexandria beta binary is published for $(uname -m) macOS." >&2; exit 1 ;;
      esac
      ;;
    Linux)
      case "$(uname -m)" in
        x86_64|amd64) printf 'linux-x86_64\n' ;;
        *) say "No Alexandria beta binary is published for $(uname -m) Linux." >&2; exit 1 ;;
      esac
      ;;
    *) say "This installer supports macOS and Linux." >&2; exit 1 ;;
  esac
}

tag="$(resolve_beta_tag)"
if [ -z "$tag" ]; then
  say "Could not find any Alexandria beta prerelease." >&2
  say "Check https://github.com/$REPO/releases, or pin one: ALEX_BETA_TAG=v0.1.26-beta.1" >&2
  exit 1
fi

version="${tag#v}"
platform="$(platform_asset)"
asset="alex-cli-$version-$platform.tar.gz"
base="https://github.com/$REPO/releases/download/$tag"

tmp="$(mktemp -d "${TMPDIR:-/tmp}/alex-beta.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT HUP INT TERM

say "Installing Alexandria beta $version ($platform)…"
curl -fsSL "$base/$asset" -o "$tmp/$asset"
curl -fsSL "$base/$asset.sha256" -o "$tmp/$asset.sha256"

expected="$(awk 'NR == 1 {print $1}' "$tmp/$asset.sha256")"
actual="$(sha256_file "$tmp/$asset")"
if [ -z "$expected" ] || [ "$actual" != "$expected" ]; then
  say "SHA-256 verification failed for $asset." >&2
  exit 1
fi

tar -xzf "$tmp/$asset" -C "$tmp"
mkdir -p "$INSTALL_DIR"
install -m 0755 "$tmp/alex" "$INSTALL_DIR/alex"
install -m 0755 "$tmp/alexandria" "$INSTALL_DIR/alexandria"

# Follow the beta channel from now on, so the next beta is an ordinary update.
"$INSTALL_DIR/alex" update --set-channel beta || \
  say "Could not persist the beta channel; set it later with: alex update --set-channel beta"

# `service install` deliberately refuses to hot-swap a daemon that is already
# loaded -- replacing it is `service restart`, which waits for in-flight routed
# requests rather than cutting a live session off mid-response.
if "$INSTALL_DIR/alex" service install; then
  say "Daemon service registered."
elif "$INSTALL_DIR/alex" service restart; then
  say "Running daemon replaced with the beta build."
else
  say "The running daemon was not replaced (routed requests may still be in flight)."
  say "Re-run when idle: alex service restart"
fi

if [ "$(uname -s)" = "Darwin" ]; then
  dmg_url="$(curl -fsSL "https://api.github.com/repos/$REPO/releases/tags/$tag" \
    | awk -F'"' '/browser_download_url/ && /\.dmg/ { print $4; exit }')"
  if [ -z "$dmg_url" ]; then
    say "No DMG was attached to $tag; the CLI/daemon is installed but the app is not."
  else
    say "Downloading the signed menu-bar app…"
    curl -fsSL "$dmg_url" -o "$tmp/AlexandriaBar-beta.dmg"

    # The app bundle cannot be replaced underneath a running process, and a stale
    # AlexandriaBar would keep showing the old version in the menu.
    if pgrep -x "$APP_PROCESS" >/dev/null 2>&1; then
      say "Quitting the running $APP_PROCESS…"
      osascript -e "tell application \"$APP_PROCESS\" to quit" >/dev/null 2>&1 || true
      waited=0
      while pgrep -x "$APP_PROCESS" >/dev/null 2>&1 && [ "$waited" -lt 20 ]; do
        sleep 0.5
        waited=$((waited + 1))
      done
      if pgrep -x "$APP_PROCESS" >/dev/null 2>&1; then
        pkill -x "$APP_PROCESS" >/dev/null 2>&1 || true
      fi
    fi

    mount_point="$(mktemp -d "${TMPDIR:-/tmp}/alex-beta-dmg.XXXXXX")"
    hdiutil attach "$tmp/AlexandriaBar-beta.dmg" -nobrowse -quiet -mountpoint "$mount_point"
    if [ ! -d "$mount_point/$APP_NAME" ]; then
      hdiutil detach "$mount_point" -quiet >/dev/null 2>&1 || true
      rmdir "$mount_point" 2>/dev/null || true
      say "$APP_NAME was not found inside the DMG; the app was not installed." >&2
      exit 1
    fi
    if [ ! -w "$APP_DIR" ]; then
      hdiutil detach "$mount_point" -quiet >/dev/null 2>&1 || true
      rmdir "$mount_point" 2>/dev/null || true
      say "$APP_DIR is not writable. Re-run with: sudo sh -c 'curl -fsSL <url> | sh'" >&2
      exit 1
    fi
    rm -rf "${APP_DIR:?}/$APP_NAME"
    ditto "$mount_point/$APP_NAME" "$APP_DIR/$APP_NAME"
    hdiutil detach "$mount_point" -quiet >/dev/null 2>&1 || true
    rmdir "$mount_point" 2>/dev/null || true
    say "Installed $APP_NAME to $APP_DIR."
    open -a "$APP_DIR/$APP_NAME" || say "Launch $APP_NAME manually from $APP_DIR."
  fi
fi

say ""
say "Alexandria beta $version installed. Later betas arrive automatically:"
say "  CLI: alex update            (channel is now 'beta')"
say "  App: Preferences -> Updates -> Release channel"
say "Back to stable at any time: alex update --set-channel stable"
case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *) say "Add $INSTALL_DIR to PATH to run alex directly." ;;
esac
