#!/bin/sh
set -eu

REPO="${ALEX_REPO:-madhavajay/alex}"
INSTALL_DIR="${ALEX_INSTALL_DIR:-$HOME/.local/bin}"

say() {
  printf '%s\n' "$*"
}

install_macos() {
  if ! command -v brew >/dev/null 2>&1; then
    say "Homebrew is required for the macOS one-line install: https://brew.sh"
    say "After installing Homebrew, run this command again."
    exit 1
  fi

  say "Installing the Alexandria CLI/daemon and menu-bar app with Homebrew…"
  brew install madhavajay/alex/alex
  brew install --cask madhavajay/alex/alexandria

  alex_bin="$(brew --prefix)/bin/alex"
  "$alex_bin" service install
  open -a AlexandriaBar

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
