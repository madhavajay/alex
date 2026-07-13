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

if "$INSTALL_DIR/alex" service install; then
  say "Alexandria beta CLI installed and its service is registered."
else
  say "Alexandria beta CLI installed, but the service could not be registered."
  say "Start it manually with: $INSTALL_DIR/alex daemon --background"
fi

if [ "$(uname -s)" = "Darwin" ]; then
  dmg_url="$(curl -fsSL "https://api.github.com/repos/$REPO/releases/tags/$tag" \
    | awk -F'"' '/browser_download_url/ && /\.dmg/ { print $4; exit }')"
  if [ -n "$dmg_url" ]; then
    say "Downloading the signed menu-bar app…"
    curl -fsSL "$dmg_url" -o "$tmp/AlexandriaBar-beta.dmg"
    open "$tmp/AlexandriaBar-beta.dmg" || \
      say "Open $tmp/AlexandriaBar-beta.dmg manually and drag AlexandriaBar to Applications."
    say "Drag AlexandriaBar to Applications to finish the app install."
  else
    say "No DMG was attached to $tag; the CLI/daemon is installed but the app is not."
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
