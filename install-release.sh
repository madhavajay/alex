#!/bin/sh
set -eu

REPO="${ALEX_REPO:-madhavajay/alex}"
INSTALL_DIR="${ALEX_INSTALL_DIR:-$HOME/.local/bin}"

say() {
  printf '%s\n' "$*"
}

quit_apps() {
  for app in AlexandriaBar Alex; do
    osascript -e "tell application \"$app\" to quit" >/dev/null 2>&1 || true
  done
  pkill -x AlexandriaBar 2>/dev/null || true
  pkill -x Alex 2>/dev/null || true
}

remove_legacy_app() {
  legacy_name="AlexandriaBar"
  legacy="/Applications/${legacy_name}.app"
  new_app="/Applications/Alex.app"
  # Only remove the legacy app once the renamed app is present, so we never
  # delete the freshly installed app on casks that still ship AlexandriaBar.app.
  # (thanks @khoaguin, #5)
  [ -e "$new_app" ] || return 0
  [ -e "$legacy" ] || return 0
  if [ -w "/Applications" ]; then
    rm -rf "$legacy"
  else
    sudo rm -rf "$legacy"
  fi
}

open_app() {
  # Open whichever app the cask installed (renamed Alex.app or legacy
  # AlexandriaBar.app). (thanks @khoaguin, #5)
  for app in Alex AlexandriaBar; do
    if [ -e "/Applications/${app}.app" ]; then
      open -a "$app"
      return 0
    fi
  done
  open -a Alex
}

install_macos() {
  if ! command -v brew >/dev/null 2>&1; then
    say "Homebrew is required for the macOS one-line install: https://brew.sh"
    say "After installing Homebrew, run this command again."
    exit 1
  fi

  say "Installing the Alexandria CLI/daemon and menu-bar app with Homebrew…"
  quit_apps
  brew install madhavajay/alex/alex
  # Recover from a broken/renamed cask record before installing. Casks from
  # before the Alex rename referenced AlexandriaBar.app, which no longer exists,
  # so brew's upgrade runs that dead uninstall and aborts ("App source
  # '/Applications/AlexandriaBar.app' is not there"). Clear any stale record,
  # then force-install the current cask (adopting an Alex.app a direct DMG may
  # already have dropped).
  brew uninstall --cask madhavajay/alex/alexandria --force >/dev/null 2>&1 || true
  rm -rf "$(brew --prefix)/Caskroom/alexandria" 2>/dev/null || true
  brew install --cask --force madhavajay/alex/alexandria
  remove_legacy_app

  alex_bin="$(brew --prefix)/bin/alex"
  # A busy daemon (in-flight routed requests) makes `service install` exit 1 and
  # asks for `alex service restart`. Don't let that abort the install or block
  # the app launch — surface it and carry on.
  if ! "$alex_bin" service install; then
    say "Daemon was busy, so it kept running the old build."
    say "Apply the update when idle with: alex service restart --force"
  fi
  open_app

  say "Alexandria is installed. The daemon is registered to run at login."
  say "Next: alex auth import"
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

install_linux() {
  machine="$(uname -m)"
  case "$machine" in
    x86_64|amd64) platform="linux-x86_64" ;;
    *)
      say "No precompiled Alexandria Linux binary is published for $machine yet." >&2
      exit 1
      ;;
  esac

  latest_url="$(curl -fsSL -o /dev/null -w '%{url_effective}' "https://github.com/$REPO/releases/latest")"
  tag="${latest_url##*/}"
  version="${tag#v}"
  if [ -z "$version" ] || [ "$version" = "$latest_url" ]; then
    say "Could not determine the latest Alexandria release." >&2
    exit 1
  fi

  asset="alex-cli-$version-$platform.tar.gz"
  base="https://github.com/$REPO/releases/download/$tag"
  tmp="$(mktemp -d "${TMPDIR:-/tmp}/alex-install.XXXXXX")"
  trap 'rm -rf "$tmp"' EXIT HUP INT TERM

  say "Downloading Alexandria $version for Linux x86_64…"
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

  if "$INSTALL_DIR/alex" service install; then
    say "Alexandria is installed and its user service is running."
  else
    say "Alexandria is installed, but the user service could not be registered."
    say "Start it manually with: $INSTALL_DIR/alex daemon --background"
  fi
  say "Next: $INSTALL_DIR/alex auth import"
  case ":$PATH:" in
    *":$INSTALL_DIR:"*) ;;
    *) say "Add $INSTALL_DIR to PATH to run alex directly." ;;
  esac
}

case "$(uname -s)" in
  Darwin) install_macos ;;
  Linux) install_linux ;;
  *)
    say "This installer currently supports macOS and Linux." >&2
    exit 1
    ;;
esac
