#!/bin/sh
# Alexandria one-liner bootstrap. Keep this POSIX sh and ASCII-only so it is
# safe to pipe directly into /bin/sh on a fresh Linux or macOS machine.
set -eu

REPO="${ALEX_REPO:-madhavajay/alex}"
INSTALL_DIR="${ALEX_INSTALL_DIR:-$HOME/.local/bin}"
HARNESS="pi"
URL=""
KEY=""
MODEL="alex/gpt-5.6-sol"
VERSION=""
NO_LAUNCH=0
YES=0

say() { printf '%s\n' "$*"; }

# lib: install-common.sh
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

platform() {
  case "$(uname -s)" in
    Linux)
      case "$(uname -m)" in
        x86_64|amd64) printf '%s\n' 'linux-x86_64' ;;
        aarch64|arm64) printf '%s\n' 'linux-aarch64' ;;
        *) say "No static Alexandria binary for Linux $(uname -m)." >&2; exit 1 ;;
      esac
      ;;
    Darwin)
      case "$(uname -m)" in
        arm64) printf '%s\n' 'macos-aarch64' ;;
        x86_64) printf '%s\n' 'macos-x86_64' ;;
        *) say "No static Alexandria binary for macOS $(uname -m)." >&2; exit 1 ;;
      esac
      ;;
    *) say "Alexandria bootstrap supports Linux and macOS." >&2; exit 1 ;;
  esac
}

release_tag() {
  if [ -n "${ALEX_RELEASE_TAG:-}" ]; then
    printf '%s\n' "$ALEX_RELEASE_TAG"
  else
    curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" </dev/null |
      awk -F'"' '/"tag_name"/ { print $4; exit }'
  fi
}

install_alex() {
  tag="$(release_tag)"
  if [ -z "$tag" ]; then
    say "Could not resolve the latest Alexandria release." >&2
    exit 1
  fi
  triple="$(platform)"
  asset="alex-$triple"
  base="https://github.com/$REPO/releases/download/$tag"
  tmp="$(mktemp -d "${TMPDIR:-/tmp}/alex-up.XXXXXX")"
  trap 'rm -rf "$tmp"' EXIT HUP INT TERM
  say "alex: downloading $asset from $tag"
  curl -fsSL "$base/$asset" -o "$tmp/$asset" </dev/null
  curl -fsSL "$base/checksums.txt" -o "$tmp/checksums.txt" </dev/null
  expected="$(awk -v asset="$asset" '$2 == asset || $2 == "*" asset {print $1; exit}' "$tmp/checksums.txt")"
  actual="$(sha256_file "$tmp/$asset")"
  if [ -z "$expected" ] || [ "$expected" != "$actual" ]; then
    say "SHA-256 verification failed for $asset." >&2
    exit 1
  fi
  mkdir -p "$INSTALL_DIR"
  install -m 0755 "$tmp/$asset" "$INSTALL_DIR/alex"
  ALEX_BIN="$INSTALL_DIR/alex"
  say "alex: installed verified CLI at $ALEX_BIN"
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --harness) HARNESS="${2:?--harness needs a value}"; shift 2 ;;
    --url) URL="${2:?--url needs a value}"; shift 2 ;;
    --key) KEY="${2:?--key needs a value}"; shift 2 ;;
    --model) MODEL="${2:?--model needs a value}"; shift 2 ;;
    --version) VERSION="${2:?--version needs a value}"; shift 2 ;;
    --no-launch) NO_LAUNCH=1; shift ;;
    --yes|-y) YES=1; shift ;;
    --) shift; break ;;
    *) say "Unknown option: $1" >&2; exit 2 ;;
  esac
done

if command -v alex >/dev/null 2>&1 && alex up --help >/dev/null 2>&1; then
  ALEX_BIN="$(command -v alex)"
  say "alex: using existing CLI at $ALEX_BIN"
else
  if command -v alex >/dev/null 2>&1; then
    say "alex: existing CLI does not support 'up'; installing a current CLI"
  fi
  install_alex
fi

set -- up "$HARNESS" --model "$MODEL"
[ -n "$URL" ] && set -- "$@" --url "$URL"
[ -n "$KEY" ] && set -- "$@" --key "$KEY"
[ -n "$VERSION" ] && set -- "$@" --version "$VERSION"
[ "$NO_LAUNCH" -eq 1 ] && set -- "$@" --no-launch
[ "$YES" -eq 1 ] && set -- "$@" --yes
exec "$ALEX_BIN" "$@"
