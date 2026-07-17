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
# It installs the CLI, replaces the running daemon, and installs the signed app.
#
# Keep this file ASCII-only. macOS /bin/sh (bash 3.2) swallowed the bytes of a
# UTF-8 ellipsis into the preceding variable name -- "$APP_PROCESS..." parsed as a
# name ending in those bytes, which under `set -u` aborted with "unbound
# variable" right before the app install. Linux bash parses it fine, so it only
# ever failed on the machine that mattered. Brace every expansion.
#
# main() is called on the last line, and every subprocess gets </dev/null: piped
# to `sh` this script *is* stdin, so a child that reads stdin would eat the rest
# of it. Defensive, not the cause of the bug above.

REPO="${ALEX_REPO:-madhavajay/alex}"
INSTALL_DIR="${ALEX_INSTALL_DIR:-$HOME/.local/bin}"
APP_DIR="${ALEX_APP_DIR:-/Applications}"
APP_NAME="Alex.app"
LEGACY_APP_STEM="AlexandriaBar"
LEGACY_APP_NAME="${LEGACY_APP_STEM}.app"
APP_BUNDLE_ID="${ALEX_APP_BUNDLE_ID:-com.madhavajay.alex}"

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

# GitHub's releases/latest never points at a prerelease, so resolve from the full
# releases list. The list is NOT reliably newest-first -- GitHub orders it by the
# tagged commit's date, so v0.1.26-beta.10 can appear BELOW beta.9. Taking the first
# prerelease therefore installs an older build. Parse every non-draft prerelease into
# a numeric key (major, minor, patch, beta) and pick the maximum, so beta.10 > beta.9.
resolve_beta_tag() {
  if [ -n "${ALEX_BETA_TAG:-}" ]; then
    printf '%s\n' "$ALEX_BETA_TAG"
    return 0
  fi
  curl -fsSL "https://api.github.com/repos/$REPO/releases?per_page=50" </dev/null | awk '
    /"tag_name":/  { if (match($0, /v[0-9]+\.[0-9]+\.[0-9]+(-beta\.[0-9]+)?/)) tag = substr($0, RSTART, RLENGTH); else tag = "" }
    /"draft":[[:space:]]*true/       { tag = "" }
    /"prerelease":[[:space:]]*true/  {
      if (tag != "" && tag ~ /-beta\./) {
        v = tag; sub(/^v/, "", v)
        n = split(v, part, /[.-]/)     # e.g. 0 1 26 beta 10  ->  part[1..3], part[5]
        key = (part[1]*1000000) + (part[2]*1000) + part[3] + (part[5]/100000)
        if (key > best) { best = key; besttag = tag }
      }
      tag = ""
    }
    END { if (besttag != "") print besttag }
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

install_cli() {
  say "Installing Alexandria beta $1 ($2)..."
  curl -fsSL "$4/$3" -o "$5/$3" </dev/null
  curl -fsSL "$4/$3.sha256" -o "$5/$3.sha256" </dev/null

  expected="$(awk 'NR == 1 {print $1}' "$5/$3.sha256")"
  actual="$(sha256_file "$5/$3")"
  if [ -z "$expected" ] || [ "$actual" != "$expected" ]; then
    say "SHA-256 verification failed for $3." >&2
    exit 1
  fi

  tar -xzf "$5/$3" -C "$5"
  mkdir -p "$INSTALL_DIR"
  install -m 0755 "$5/alex" "$INSTALL_DIR/alex"
  install -m 0755 "$5/alexandria" "$INSTALL_DIR/alexandria"
}

# Point launchd at the newly installed binary and (re)start the daemon.
#
# The launchd plist pins an ABSOLUTE binary path (current_exe at install time).
# A daemon first installed from a dev build stays pinned to that old binary, and
# neither `service install` (refuses while loaded) nor `service restart`
# (re-launches the SAME pinned path) re-points it -- so the daemon would stay on
# the old version forever while the app updates. When a daemon is already loaded
# we therefore uninstall and reinstall to force the plist to re-pin to THIS
# binary, rather than relying on restart.
replace_daemon() {
  # Evict stray, manually-started daemons first. `alex daemon` run in a terminal
  # co-binds :4100 via SO_REUSEPORT and keeps serving the OLD binary across
  # upgrades -- launchd's re-pin cannot see or stop them, so the daemon appears
  # "stuck" on an old version forever. launchd relaunches its own managed daemon
  # after service install, so clearing every running daemon here is safe.
  if pgrep -f "alexandria daemon" >/dev/null 2>&1 || pgrep -f "alex daemon" >/dev/null 2>&1; then
    say "Clearing stray daemon processes holding the port..."
    pkill -f "alexandria daemon" >/dev/null 2>&1 || true
    pkill -f "alex daemon" >/dev/null 2>&1 || true
    sleep 1
  fi
  if "$INSTALL_DIR/alex" service install >/dev/null 2>&1 </dev/null; then
    say "Daemon service registered."
    return
  fi
  # A daemon is already loaded (possibly pinned to an older binary). Re-pin it.
  "$INSTALL_DIR/alex" service uninstall >/dev/null 2>&1 </dev/null || true
  if "$INSTALL_DIR/alex" service install >/dev/null 2>&1 </dev/null; then
    say "Daemon re-pointed to the new build and restarted."
    return
  fi
  # Fall back to the graceful in-place restart (same path) if the re-pin failed.
  if "$INSTALL_DIR/alex" service restart </dev/null; then
    say "Running daemon restarted."
  else
    say "The daemon was not replaced. Run manually: alex service uninstall && alex service install"
  fi
}

# The app bundle cannot be replaced underneath a running process, and a stale
# app keeps reporting the old version in the menu bar.
quit_app() {
  for app in AlexandriaBar Alex; do
    osascript -e "tell application \"$app\" to quit" >/dev/null 2>&1 </dev/null || true
  done
  for process in AlexandriaBar Alex; do
    if pgrep -x "$process" >/dev/null 2>&1; then
      say "Quitting the running ${process}..."
    waited=0
      while pgrep -x "$process" >/dev/null 2>&1 && [ "$waited" -lt 20 ]; do
      sleep 0.5
      waited=$((waited + 1))
    done
      if pgrep -x "$process" >/dev/null 2>&1; then
        pkill -x "$process" >/dev/null 2>&1 || true
      fi
    fi
  done
}

install_app() {
  dmg_url="$(curl -fsSL "https://api.github.com/repos/$REPO/releases/tags/$1" </dev/null \
    | awk -F'"' '/browser_download_url/ && /\.dmg/ { print $4; exit }')"
  if [ -z "$dmg_url" ]; then
    say "No DMG was attached to $1; the CLI/daemon is installed but the app is not." >&2
    exit 1
  fi

  say "Downloading the signed menu-bar app..."
  curl -fsSL "$dmg_url" -o "$2/Alex-beta.dmg" </dev/null

  quit_app

  mount_point="$(mktemp -d "${TMPDIR:-/tmp}/alex-beta-dmg.XXXXXX")"
  hdiutil attach "$2/Alex-beta.dmg" -nobrowse -quiet -mountpoint "$mount_point" </dev/null

  if [ ! -d "$mount_point/$APP_NAME" ]; then
    hdiutil detach "$mount_point" -quiet >/dev/null 2>&1 || true
    say "$APP_NAME was not found inside the DMG; the app was not installed." >&2
    exit 1
  fi
  if [ ! -w "$APP_DIR" ]; then
    hdiutil detach "$mount_point" -quiet >/dev/null 2>&1 || true
    say "$APP_DIR is not writable. Re-run as a user who can write to it." >&2
    exit 1
  fi

  rm -rf "${APP_DIR:?}/$APP_NAME"
  ditto "$mount_point/$APP_NAME" "$APP_DIR/$APP_NAME"
  rm -rf "${APP_DIR:?}/$LEGACY_APP_NAME"
  hdiutil detach "$mount_point" -quiet >/dev/null 2>&1 || true
  rmdir "$mount_point" 2>/dev/null || true

  say "Installed $APP_NAME to $APP_DIR."

  # The app's Sparkle channel is a separate setting from the daemon's, and it
  # defaults to stable -- so a freshly installed beta app would otherwise offer to
  # "update" the user DOWN to the current stable release. Set it before launching.
  # Do this while the app is quit: it reads UserDefaults at startup.
  defaults write "$APP_BUNDLE_ID" updateChannel -string beta >/dev/null 2>&1 </dev/null || \
    say "Could not set the app's release channel; set it in Settings > General > Updates."

  open -a "$APP_DIR/$APP_NAME" </dev/null || say "Launch $APP_NAME manually from $APP_DIR."
}

main() {
  tag="$(resolve_beta_tag)"
  if [ -z "$tag" ]; then
    say "Could not find any Alexandria beta prerelease." >&2
    say "Check https://github.com/$REPO/releases, or pin one: ALEX_BETA_TAG=v0.1.26-beta.4" >&2
    exit 1
  fi

  version="${tag#v}"
  platform="$(platform_asset)"
  asset="alex-cli-$version-$platform.tar.gz"
  base="https://github.com/$REPO/releases/download/$tag"

  tmp="$(mktemp -d "${TMPDIR:-/tmp}/alex-beta.XXXXXX")"
  trap 'rm -rf "$tmp"' EXIT HUP INT TERM

  install_cli "$version" "$platform" "$asset" "$base" "$tmp"

  # Follow the beta channel from now on, so the next beta is an ordinary update.
  "$INSTALL_DIR/alex" update --set-channel beta </dev/null || \
    say "Could not persist the beta channel; set it later with: alex update --set-channel beta"

  replace_daemon

  if [ "$(uname -s)" = "Darwin" ]; then
    install_app "$tag" "$tmp"
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
}

main "$@"
