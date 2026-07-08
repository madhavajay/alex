#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: ./build-signed.sh [--clean] [--notarize|--skip-notarize]

Builds, Developer ID signs, and packages AlexandriaBar.app as a signed DMG.

Environment can be provided via .env or exported variables:
  APPLE_SIGNING_IDENTITY           Developer ID Application identity name
  SIGNING_CERTIFICATE_P12_DATA     base64 encoded Developer ID .p12 (CI)
  SIGNING_CERTIFICATE_PASSWORD     password for the .p12 (CI)
  KEYCHAIN_PASSWORD                optional keychain password
  APPLE_ID                         Apple ID for notarization
  APPLE_PASSWORD                   app-specific password for notarization
  APPLE_TEAM_ID                    Apple developer team ID

Defaults:
  BUNDLE_ID=com.madhavajay.alexandria-macos
  APP_NAME=AlexandriaBar
EOF
}

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$ROOT"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "build-signed.sh only runs on macOS." >&2
  exit 1
fi

CLEAN=false
NOTARIZE_MODE="auto"
for arg in "$@"; do
  case "$arg" in
    --clean) CLEAN=true ;;
    --notarize) NOTARIZE_MODE="force" ;;
    --skip-notarize|--no-notarize) NOTARIZE_MODE="skip" ;;
    -h|--help) usage; exit 0 ;;
    *) echo "Unknown argument: $arg" >&2; usage; exit 1 ;;
  esac
done

if [[ -f .env ]]; then
  while IFS='=' read -r key value || [[ -n "$key" ]]; do
    [[ "$key" =~ ^[A-Za-z_][A-Za-z0-9_]*$ ]] || continue
    [[ "$key" == \#* ]] && continue
    value="${value%$'\r'}"
    if [[ "$value" == \"*\" && "$value" == *\" ]]; then
      value="${value:1:${#value}-2}"
    elif [[ "$value" == \'*\' && "$value" == *\' ]]; then
      value="${value:1:${#value}-2}"
    fi
    if [[ -n "$value" && -z "${!key:-}" ]]; then
      export "$key=$value"
    fi
  done < .env
fi

APP_NAME="${APP_NAME:-AlexandriaBar}"
BUNDLE_ID="${BUNDLE_ID:-com.madhavajay.alexandria-macos}"
VERSION="${VERSION:-$(awk -F'"' '/^version =/ { print $2; exit }' Cargo.toml 2>/dev/null || true)}"
VERSION="${VERSION:-0.1.0}"
DIST_DIR="${DIST_DIR:-macos/dist}"
DMG_NAME="${DMG_NAME:-${APP_NAME}-${VERSION}.dmg}"
DMG_PATH="$DIST_DIR/$DMG_NAME"

TRAP_P12=""
TEMP_KEYCHAIN=""
DMG_STAGE=""
cleanup() {
  if [[ -n "$DMG_STAGE" && -d "$DMG_STAGE" ]]; then
    rm -rf "$DMG_STAGE" || true
  fi
  if [[ -n "$TRAP_P12" && -f "$TRAP_P12" ]]; then
    rm -f "$TRAP_P12" || true
  fi
  if [[ -n "$TEMP_KEYCHAIN" && -f "$TEMP_KEYCHAIN" ]]; then
    security delete-keychain "$TEMP_KEYCHAIN" >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT

decode_p12() {
  if printf '%s' "$SIGNING_CERTIFICATE_P12_DATA" | base64 --decode >"$TRAP_P12" 2>/dev/null; then
    return 0
  fi
  if printf '%s' "$SIGNING_CERTIFICATE_P12_DATA" | base64 -D >"$TRAP_P12" 2>/dev/null; then
    return 0
  fi
  printf '%s' "$SIGNING_CERTIFICATE_P12_DATA" | base64 -d >"$TRAP_P12"
}

prepare_import_keychain() {
  local keychain
  if [[ -n "${GITHUB_ACTIONS:-}" || -n "${CI:-}" ]]; then
    KEYCHAIN_PASSWORD="${KEYCHAIN_PASSWORD:-$(uuidgen)}"
    TEMP_KEYCHAIN="${RUNNER_TEMP:-${TMPDIR:-/tmp}}/alexandria-signing.keychain-db"
    security create-keychain -p "$KEYCHAIN_PASSWORD" "$TEMP_KEYCHAIN"
    security set-keychain-settings -lut 21600 "$TEMP_KEYCHAIN"
    security unlock-keychain -p "$KEYCHAIN_PASSWORD" "$TEMP_KEYCHAIN"
    existing_keychains="$(security list-keychains -d user | tr -d '"')"
    # shellcheck disable=SC2086
    security list-keychains -d user -s "$TEMP_KEYCHAIN" $existing_keychains
    keychain="$TEMP_KEYCHAIN"
  else
    keychain="${KEYCHAIN_PATH:-$HOME/Library/Keychains/login.keychain-db}"
    if [[ -n "${KEYCHAIN_PASSWORD:-}" ]]; then
      security unlock-keychain -p "$KEYCHAIN_PASSWORD" "$keychain" >/dev/null 2>&1 || true
    fi
  fi
  printf '%s\n' "$keychain"
}

CERT_LIST="$(security find-identity -p codesigning -v || true)"
NEEDS_IMPORT=false
if [[ -n "${SIGNING_CERTIFICATE_P12_DATA:-}" ]]; then
  if [[ -n "${APPLE_SIGNING_IDENTITY:-}" ]]; then
    if ! grep -Fq "$APPLE_SIGNING_IDENTITY" <<<"$CERT_LIST"; then
      NEEDS_IMPORT=true
    fi
  elif ! grep -q "Developer ID Application" <<<"$CERT_LIST"; then
    NEEDS_IMPORT=true
  fi
fi

if [[ "$NEEDS_IMPORT" == "true" ]]; then
  if [[ -z "${SIGNING_CERTIFICATE_PASSWORD:-}" ]]; then
    echo "SIGNING_CERTIFICATE_PASSWORD is required when SIGNING_CERTIFICATE_P12_DATA is set." >&2
    exit 1
  fi
  echo "Importing Developer ID certificate from SIGNING_CERTIFICATE_P12_DATA..."
  TRAP_P12="$(mktemp "${TMPDIR:-/tmp}/alexandria-cert-XXXXXX.p12")"
  decode_p12
  IMPORT_KEYCHAIN="$(prepare_import_keychain)"
  security import "$TRAP_P12" -k "$IMPORT_KEYCHAIN" \
    -P "$SIGNING_CERTIFICATE_PASSWORD" \
    -T /usr/bin/codesign -T /usr/bin/security >/dev/null
  if [[ -n "${KEYCHAIN_PASSWORD:-}" ]]; then
    security set-key-partition-list -S apple-tool:,apple: -s -k "$KEYCHAIN_PASSWORD" "$IMPORT_KEYCHAIN" >/dev/null 2>&1 || true
  fi
else
  if grep -q "Developer ID Application" <<<"$CERT_LIST"; then
    echo "Developer ID certificate already available, skipping import."
  fi
fi

if [[ -z "${APPLE_SIGNING_IDENTITY:-}" ]]; then
  if [[ -n "${APPLE_TEAM_ID:-}" ]]; then
    APPLE_SIGNING_IDENTITY="$(security find-identity -p codesigning -v | awk -F\" -v team="(${APPLE_TEAM_ID})" '/Developer ID Application/ && index($2, team) { print $2; exit }')"
  else
    mapfile -t developer_id_identities < <(security find-identity -p codesigning -v | awk -F\" '/Developer ID Application/ { print $2 }')
    if [[ "${#developer_id_identities[@]}" -eq 1 ]]; then
      APPLE_SIGNING_IDENTITY="${developer_id_identities[0]}"
    elif [[ "${#developer_id_identities[@]}" -gt 1 ]]; then
      echo "Multiple Developer ID Application identities found. Set APPLE_SIGNING_IDENTITY explicitly:" >&2
      printf '  %s\n' "${developer_id_identities[@]}" >&2
      exit 1
    fi
  fi
fi

if [[ -z "${APPLE_SIGNING_IDENTITY:-}" ]]; then
  echo "APPLE_SIGNING_IDENTITY is not set and no Developer ID Application identity was found." >&2
  echo "Run: security find-identity -v -p codesigning" >&2
  exit 1
fi

if ! security find-identity -p codesigning -v | grep -Fq "$APPLE_SIGNING_IDENTITY"; then
  echo "Signing identity is not available in the keychain: $APPLE_SIGNING_IDENTITY" >&2
  exit 1
fi

echo "Using signing identity: $APPLE_SIGNING_IDENTITY"
echo "Using bundle identifier: $BUNDLE_ID"

if [[ "$CLEAN" == "true" ]]; then
  echo "Cleaning macOS build artifacts..."
  rm -rf "$DIST_DIR"
  (cd macos && swift package clean)
fi

echo "Building signed app bundle..."
CONFIGURATION=release \
IDENTITY="$APPLE_SIGNING_IDENTITY" \
BUNDLE_ID="$BUNDLE_ID" \
VERSION="$VERSION" \
./macos/Scripts/package_app.sh

APP_PATH="$DIST_DIR/$APP_NAME.app"
if [[ ! -d "$APP_PATH" ]]; then
  echo "Expected app was not built: $APP_PATH" >&2
  exit 1
fi

echo "Verifying app signature..."
codesign --verify --deep --strict --verbose=2 "$APP_PATH"

SHOULD_NOTARIZE=false
case "$NOTARIZE_MODE" in
  skip)
    echo "Skipping notarization by request."
    ;;
  force)
    SHOULD_NOTARIZE=true
    ;;
  auto)
    if [[ -n "${APPLE_ID:-}" && -n "${APPLE_PASSWORD:-}" && -n "${APPLE_TEAM_ID:-}" ]]; then
      SHOULD_NOTARIZE=true
    else
      echo "Skipping notarization because APPLE_ID, APPLE_PASSWORD, or APPLE_TEAM_ID is empty."
    fi
    ;;
esac

if [[ "$SHOULD_NOTARIZE" == "true" ]]; then
  missing=()
  [[ -n "${APPLE_ID:-}" ]] || missing+=(APPLE_ID)
  [[ -n "${APPLE_PASSWORD:-}" ]] || missing+=(APPLE_PASSWORD)
  [[ -n "${APPLE_TEAM_ID:-}" ]] || missing+=(APPLE_TEAM_ID)
  if [[ "${#missing[@]}" -gt 0 ]]; then
    echo "Missing notarization values: ${missing[*]}" >&2
    exit 1
  fi

  APP_ZIP="$DIST_DIR/${APP_NAME}-${VERSION}.zip"
  rm -f "$APP_ZIP"
  echo "Submitting app bundle for notarization..."
  ditto -c -k --keepParent "$APP_PATH" "$APP_ZIP"
  xcrun notarytool submit "$APP_ZIP" \
    --apple-id "$APPLE_ID" \
    --password "$APPLE_PASSWORD" \
    --team-id "$APPLE_TEAM_ID" \
    --wait
  rm -f "$APP_ZIP"

  echo "Stapling app notarization ticket..."
  xcrun stapler staple "$APP_PATH"
  xcrun stapler validate "$APP_PATH"
fi

echo "Creating DMG..."
mkdir -p "$DIST_DIR"
rm -f "$DMG_PATH"
DMG_STAGE="$(mktemp -d "${TMPDIR:-/tmp}/alexandria-dmg-XXXXXX")"
ditto "$APP_PATH" "$DMG_STAGE/$APP_NAME.app"
ln -s /Applications "$DMG_STAGE/Applications"
hdiutil create -volname "Alexandria" -srcfolder "$DMG_STAGE" -ov -format UDZO "$DMG_PATH" >/dev/null
hdiutil verify "$DMG_PATH" >/dev/null

echo "Signing DMG..."
codesign --force --timestamp --sign "$APPLE_SIGNING_IDENTITY" "$DMG_PATH"
codesign --verify --verbose=2 "$DMG_PATH"

if [[ "$SHOULD_NOTARIZE" == "true" ]]; then
  echo "Submitting DMG for notarization..."
  xcrun notarytool submit "$DMG_PATH" \
    --apple-id "$APPLE_ID" \
    --password "$APPLE_PASSWORD" \
    --team-id "$APPLE_TEAM_ID" \
    --wait

  echo "Stapling notarization ticket..."
  xcrun stapler staple "$DMG_PATH"
  xcrun stapler validate "$DMG_PATH"
  spctl --assess --type open --context context:primary-signature --verbose "$DMG_PATH"
fi

echo ""
echo "Build complete: $DMG_PATH"
echo "Check it with: ./check-gatekeeper.sh \"$DMG_PATH\""
