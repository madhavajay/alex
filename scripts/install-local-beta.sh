#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: ./scripts/install-local-beta.sh VERSION [--ref REF] [--prefix DIR]

Build and install a numbered macOS beta from one committed ref without
stamping or dirtying the source worktree. The CLI/daemon and Alex.app receive
the same VERSION. No tag or GitHub release is created.

Examples:
  ./scripts/install-local-beta.sh 0.1.29-beta.12
  ./scripts/install-local-beta.sh 0.1.29-beta.13 --ref v1/integration

The install prefix defaults to the directory of the current `alex` executable,
or ~/.local/bin when Alex is not installed.
EOF
}

VERSION=""
REF="HEAD"
PREFIX="${ALEX_LOCAL_BETA_PREFIX:-}"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --ref)
      [[ $# -ge 2 ]] || { echo "--ref requires a value" >&2; exit 2; }
      REF="$2"
      shift 2
      ;;
    --prefix)
      [[ $# -ge 2 ]] || { echo "--prefix requires a value" >&2; exit 2; }
      PREFIX="$2"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    -* )
      echo "unknown option: $1" >&2
      usage
      exit 2
      ;;
    *)
      if [[ -n "$VERSION" ]]; then
        echo "unexpected argument: $1" >&2
        usage
        exit 2
      fi
      VERSION="$1"
      shift
      ;;
  esac
done

if [[ ! "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+-beta\.[0-9]+$ ]]; then
  echo "VERSION must match X.Y.Z-beta.N" >&2
  exit 2
fi
if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "local app beta installation currently runs on macOS" >&2
  exit 1
fi

REPO_ROOT="$(git rev-parse --show-toplevel)"
if [[ -n "$(git -C "$REPO_ROOT" status --porcelain)" ]]; then
  echo "refusing to build a checkpoint from a dirty worktree" >&2
  echo "commit or remove pending changes, then retry" >&2
  exit 1
fi
git -C "$REPO_ROOT" rev-parse --verify "${REF}^{commit}" >/dev/null

if [[ -z "$PREFIX" ]]; then
  if CURRENT_ALEX="$(type -P alex 2>/dev/null)" && [[ -n "$CURRENT_ALEX" ]]; then
    PREFIX="$(dirname "$CURRENT_ALEX")"
  else
    PREFIX="$HOME/.local/bin"
  fi
fi
if [[ "$PREFIX" != /* ]]; then
  echo "install prefix must be an absolute path: $PREFIX" >&2
  exit 2
fi

TEMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/alex-local-beta.XXXXXX")"
BUILD_TREE="$TEMP_ROOT/source"
cleanup() {
  if git -C "$REPO_ROOT" worktree list --porcelain \
      | rg -Fq "worktree $BUILD_TREE"; then
    git -C "$REPO_ROOT" worktree remove --force "$BUILD_TREE" >/dev/null 2>&1 || true
  fi
  rmdir "$TEMP_ROOT" >/dev/null 2>&1 || true
}
trap cleanup EXIT

echo "◆ preparing isolated $VERSION build from $REF"
git -C "$REPO_ROOT" worktree add --detach "$BUILD_TREE" "$REF" >/dev/null

INSTALL_ARGS=(--prefix "$PREFIX")
if launchctl print "gui/$(id -u)/com.alexandria.daemon" >/dev/null 2>&1; then
  INSTALL_ARGS+=(--upgrade)
else
  INSTALL_ARGS+=(--service)
fi

(
  cd "$BUILD_TREE"
  ./scripts/stamp-version.sh "$VERSION"
  ./install-macos.sh "${INSTALL_ARGS[@]}"
)

CLI_VERSION="$($PREFIX/alex --version)"
APP_VERSION="$(defaults read "$HOME/Applications/Alex.app/Contents/Info" CFBundleShortVersionString)"
if [[ "$CLI_VERSION" != *"$VERSION"* || "$APP_VERSION" != "$VERSION" ]]; then
  echo "installed version mismatch: cli='$CLI_VERSION' app='$APP_VERSION'" >&2
  exit 1
fi

COMMIT="$(git -C "$REPO_ROOT" rev-parse "${REF}^{commit}")"
echo "◆ installed local Alex $VERSION"
echo "  commit: $COMMIT"
echo "  cli:    $PREFIX/alex"
echo "  app:    $HOME/Applications/Alex.app"

