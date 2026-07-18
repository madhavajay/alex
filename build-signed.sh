#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: ./build-signed.sh [--clean] [--notarize|--skip-notarize]

Builds, Developer ID signs, and packages Alex.app as a signed DMG.

Environment can be provided via .env or exported variables:
  APPLE_SIGNING_IDENTITY           Developer ID Application identity name
  SIGNING_CERTIFICATE_P12_DATA     base64 encoded Developer ID .p12 (CI)
  SIGNING_CERTIFICATE_PASSWORD     password for the .p12 (CI)
  KEYCHAIN_PASSWORD                optional keychain password
  APPLE_ID                         Apple ID for notarization
  APPLE_PASSWORD                   app-specific password for notarization
  APPLE_TEAM_ID                    Apple developer team ID
  SPARKLE_FEED_URL                 Sparkle appcast URL
  SPARKLE_PUBLIC_ED_KEY            Sparkle EdDSA public key

Defaults:
  BUNDLE_ID=com.madhavajay.alex
  APP_DISPLAY=Alex
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
APP_DISPLAY="Alex"
BUNDLE_ID="${BUNDLE_ID:-com.madhavajay.alex}"
VERSION="${VERSION:-$(awk -F'"' '/^version =/ { print $2; exit }' Cargo.toml 2>/dev/null || true)}"
VERSION="${VERSION:-0.1.0}"
DIST_DIR="${DIST_DIR:-macos/dist}"
DMG_NAME="${DMG_NAME:-${APP_DISPLAY}-${VERSION}.dmg}"
DMG_PATH="$DIST_DIR/$DMG_NAME"

TRAP_P12=""
TEMP_KEYCHAIN=""
DMG_STAGE=""
ORIGINAL_DEFAULT_KEYCHAIN=""
NOTARY_RESULT=""
cleanup() {
  if [[ -n "$DMG_STAGE" && -d "$DMG_STAGE" ]]; then
    rm -rf "$DMG_STAGE" || true
  fi
  if [[ -n "$TRAP_P12" && -f "$TRAP_P12" ]]; then
    rm -f "$TRAP_P12" || true
  fi
  if [[ -n "$TEMP_KEYCHAIN" && -f "$TEMP_KEYCHAIN" ]]; then
    if [[ -n "$ORIGINAL_DEFAULT_KEYCHAIN" ]]; then
      security default-keychain -s "$ORIGINAL_DEFAULT_KEYCHAIN" >/dev/null 2>&1 || true
    fi
    security delete-keychain "$TEMP_KEYCHAIN" >/dev/null 2>&1 || true
  fi
  if [[ -n "$NOTARY_RESULT" && -f "$NOTARY_RESULT" ]]; then
    rm -f "$NOTARY_RESULT" || true
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
    ORIGINAL_DEFAULT_KEYCHAIN="$(security default-keychain | tr -d '"')"
    security create-keychain -p "$KEYCHAIN_PASSWORD" "$TEMP_KEYCHAIN"
    security set-keychain-settings -lut 21600 "$TEMP_KEYCHAIN"
    security unlock-keychain -p "$KEYCHAIN_PASSWORD" "$TEMP_KEYCHAIN"
    security default-keychain -s "$TEMP_KEYCHAIN"
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
    security set-key-partition-list -S apple-tool:,apple: -s -k "$KEYCHAIN_PASSWORD" "$IMPORT_KEYCHAIN" >/dev/null 2>&1
  fi
else
  if grep -q "Developer ID Application" <<<"$CERT_LIST"; then
    echo "Developer ID certificate already available, skipping import."
  fi
fi

IDENTITY_LIST="$(security find-identity -p codesigning -v || true)"

if [[ "${APPLE_SIGNING_IDENTITY:-}" =~ ^[A-Fa-f0-9]{40}$ ]]; then
  CODESIGN_IDENTITY="$APPLE_SIGNING_IDENTITY"
else
  CODESIGN_IDENTITY=""
  RESOLVED_SIGNING_IDENTITY=""

  if [[ -n "${APPLE_TEAM_ID:-}" ]]; then
    identity_line="$(printf '%s\n' "$IDENTITY_LIST" | awk -v team="(${APPLE_TEAM_ID})" '/Developer ID Application/ && index($0, team) { print; exit }')"
    if [[ -n "$identity_line" ]]; then
      CODESIGN_IDENTITY="$(awk '{ print $2 }' <<<"$identity_line")"
      RESOLVED_SIGNING_IDENTITY="$(awk -F\" '{ print $2 }' <<<"$identity_line")"
    fi
  fi

  if [[ -z "$CODESIGN_IDENTITY" && -n "${APPLE_SIGNING_IDENTITY:-}" ]]; then
    identity_line="$(printf '%s\n' "$IDENTITY_LIST" | awk -v identity="$APPLE_SIGNING_IDENTITY" '/Developer ID Application/ && index($0, identity) { print; exit }')"
    if [[ -n "$identity_line" ]]; then
      CODESIGN_IDENTITY="$(awk '{ print $2 }' <<<"$identity_line")"
      RESOLVED_SIGNING_IDENTITY="$(awk -F\" '{ print $2 }' <<<"$identity_line")"
    fi
  fi

  if [[ -z "$CODESIGN_IDENTITY" ]]; then
    mapfile -t developer_id_lines < <(printf '%s\n' "$IDENTITY_LIST" | awk '/Developer ID Application/ { print }')
    if [[ "${#developer_id_lines[@]}" -eq 1 ]]; then
      CODESIGN_IDENTITY="$(awk '{ print $2 }' <<<"${developer_id_lines[0]}")"
      RESOLVED_SIGNING_IDENTITY="$(awk -F\" '{ print $2 }' <<<"${developer_id_lines[0]}")"
    elif [[ "${#developer_id_lines[@]}" -gt 1 ]]; then
      echo "Multiple Developer ID Application identities found. Set APPLE_TEAM_ID or APPLE_SIGNING_IDENTITY explicitly." >&2
      exit 1
    fi
  fi

  if [[ -n "${RESOLVED_SIGNING_IDENTITY:-}" ]]; then
    APPLE_SIGNING_IDENTITY="$RESOLVED_SIGNING_IDENTITY"
  fi
fi

if [[ -z "${CODESIGN_IDENTITY:-}" ]]; then
  echo "Unable to resolve a Developer ID Application signing identity after certificate import." >&2
  echo "Check APPLE_TEAM_ID, APPLE_SIGNING_IDENTITY, and SIGNING_CERTIFICATE_P12_DATA." >&2
  exit 1
fi

echo "Using signing identity: $APPLE_SIGNING_IDENTITY"
echo "Using bundle identifier: $BUNDLE_ID"

submit_for_notarization() {
  local artifact="$1"
  local verdict
  NOTARY_RESULT="$(mktemp "${TMPDIR:-/tmp}/alexandria-notary-XXXXXX.json")"
  xcrun notarytool submit "$artifact" \
    --apple-id "$APPLE_ID" \
    --password "$APPLE_PASSWORD" \
    --team-id "$APPLE_TEAM_ID" \
    --wait \
    --output-format json | tee "$NOTARY_RESULT"
  verdict="$(plutil -extract status raw -o - "$NOTARY_RESULT")"
  if [[ "$verdict" != "Accepted" ]]; then
    echo "Apple notarization rejected $artifact (status: ${verdict:-unknown})." >&2
    exit 1
  fi
  rm -f "$NOTARY_RESULT"
  NOTARY_RESULT=""
}

if [[ "$CLEAN" == "true" ]]; then
  echo "Cleaning macOS build artifacts..."
  rm -rf "$DIST_DIR"
  (cd macos && swift package clean)
fi

echo "Building signed app bundle..."
CONFIGURATION=release \
IDENTITY="$CODESIGN_IDENTITY" \
BUNDLE_ID="$BUNDLE_ID" \
VERSION="$VERSION" \
APP_DISPLAY="$APP_DISPLAY" \
./macos/Scripts/package_app.sh

APP_PATH="$DIST_DIR/$APP_DISPLAY.app"
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

  APP_ZIP="$DIST_DIR/${APP_DISPLAY}-${VERSION}.zip"
  rm -f "$APP_ZIP"
  echo "Submitting app bundle for notarization..."
  ditto -c -k --keepParent "$APP_PATH" "$APP_ZIP"
  submit_for_notarization "$APP_ZIP"
  rm -f "$APP_ZIP"

  echo "Stapling app notarization ticket..."
  xcrun stapler staple "$APP_PATH"
  xcrun stapler validate "$APP_PATH"
fi

echo "Creating DMG..."
mkdir -p "$DIST_DIR"
rm -f "$DMG_PATH"
DMG_STAGE="$(mktemp -d "${TMPDIR:-/tmp}/alexandria-dmg-XXXXXX")"
ditto "$APP_PATH" "$DMG_STAGE/$APP_DISPLAY.app"
ln -s /Applications "$DMG_STAGE/Applications"
hdiutil create -volname "Alex" -srcfolder "$DMG_STAGE" -ov -format UDZO "$DMG_PATH" >/dev/null
hdiutil verify "$DMG_PATH" >/dev/null

echo "Signing DMG..."
codesign --force --timestamp --sign "$CODESIGN_IDENTITY" "$DMG_PATH"
codesign --verify --verbose=2 "$DMG_PATH"

if [[ "$SHOULD_NOTARIZE" == "true" ]]; then
  echo "Submitting DMG for notarization..."
  submit_for_notarization "$DMG_PATH"

  echo "Stapling notarization ticket..."
  xcrun stapler staple "$DMG_PATH"
  xcrun stapler validate "$DMG_PATH"
  spctl --assess --type open --context context:primary-signature --verbose "$DMG_PATH"
fi

echo ""
echo "Build complete: $DMG_PATH"
echo "Check it with: ./check-gatekeeper.sh \"$DMG_PATH\""
