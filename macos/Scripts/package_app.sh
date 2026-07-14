#!/usr/bin/env bash
set -euo pipefail

# Builds AlexandriaBar.app into macos/dist/.
#   CONFIGURATION=debug|release (default release)
#   IDENTITY="Developer ID Application: ..." (default adhoc "-")
#   BUNDLE_ID=com.example.app (default com.madhavajay.alexandria-macos)
#   VERSION=1.2.3 (default 0.1.0)
# Adhoc-signed rebuilds change code identity each build, which resets
# Little Snitch rules; use a stable IDENTITY if that bites.

cd "$(dirname "$0")/.."

CONFIGURATION="${CONFIGURATION:-release}"
IDENTITY="${IDENTITY:--}"
APP_NAME="AlexandriaBar"
BUNDLE_ID="${BUNDLE_ID:-com.madhavajay.alexandria-macos}"
VERSION="${VERSION:-0.1.0}"
SPARKLE_FEED_URL="${SPARKLE_FEED_URL:-https://madhavajay.github.io/alex/appcast.xml}"

# The Sparkle EdDSA *public* key. This is not a secret -- a copy ships inside the
# Info.plist of every DMG we publish, so anyone can already read it. Defaulting it
# here matters: without SUPublicEDKey the app has nothing to validate an update
# against, so EVERY update fails with "improperly signed" and the build is a
# dead end -- it can never update itself back to an official release. This used to
# default to empty and the key was then silently omitted, which is exactly how a
# local dev build ended up unable to escape itself.
SPARKLE_PUBLIC_ED_KEY="${SPARKLE_PUBLIC_ED_KEY:-WifIRZCxYKIrh/Jb40HOtNapTS5ZCXpXrv7LdDWHlCw=}"
DIST="dist"
APP="$DIST/$APP_NAME.app"

swift build -c "$CONFIGURATION"

BIN=".build/$CONFIGURATION/$APP_NAME"
[[ -x "$BIN" ]] || { echo "missing binary $BIN" >&2; exit 1; }

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources" "$APP/Contents/Frameworks"
cp "$BIN" "$APP/Contents/MacOS/$APP_NAME"

SPARKLE_FRAMEWORK="$(find .build -type d -name Sparkle.framework -path '*artifacts*' -print -quit)"
if otool -L "$BIN" | grep -q "Sparkle.framework"; then
  if [[ -z "$SPARKLE_FRAMEWORK" ]]; then
    echo "Swift build linked Sparkle, but Sparkle.framework was not found under .build artifacts." >&2
    exit 1
  fi
  cp -R "$SPARKLE_FRAMEWORK" "$APP/Contents/Frameworks/Sparkle.framework"
fi

RES_BUNDLE=".build/$CONFIGURATION/${APP_NAME}_${APP_NAME}.bundle"
if [[ -d "$RES_BUNDLE" ]]; then
  cp -R "$RES_BUNDLE" "$APP/Contents/Resources/"
fi

ICON_SRC="Resources/icon.png"
if [[ -f "$ICON_SRC" ]]; then
  ICONSET="$DIST/AppIcon.iconset"
  rm -rf "$ICONSET"
  mkdir -p "$ICONSET"
  for s in 16 32 128 256 512; do
    sips -z "$s" "$s" "$ICON_SRC" --out "$ICONSET/icon_${s}x${s}.png" >/dev/null
    d=$((s * 2))
    sips -z "$d" "$d" "$ICON_SRC" --out "$ICONSET/icon_${s}x${s}@2x.png" >/dev/null
  done
  iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/AppIcon.icns"
  rm -rf "$ICONSET"
  cp "$ICON_SRC" "$APP/Contents/Resources/icon.png"
fi

plist_escape() {
  printf '%s' "$1" | sed \
    -e 's/&/\&amp;/g' \
    -e 's/</\&lt;/g' \
    -e 's/>/\&gt;/g' \
    -e 's/"/\&quot;/g' \
    -e "s/'/\&apos;/g"
}

SPARKLE_FEED_URL_ESCAPED="$(plist_escape "$SPARKLE_FEED_URL")"
SPARKLE_PUBLIC_ED_KEY_PLIST=""
if [[ -n "$SPARKLE_PUBLIC_ED_KEY" ]]; then
  SPARKLE_PUBLIC_ED_KEY_ESCAPED="$(plist_escape "$SPARKLE_PUBLIC_ED_KEY")"
  SPARKLE_PUBLIC_ED_KEY_PLIST="    <key>SUPublicEDKey</key><string>$SPARKLE_PUBLIC_ED_KEY_ESCAPED</string>"
else
  # Never omit it silently: the resulting app rejects every update as "improperly
  # signed" and can never replace itself with an official build.
  echo "WARNING: SPARKLE_PUBLIC_ED_KEY is empty." >&2
  echo "  This app will REJECT every update ('improperly signed') and cannot update itself." >&2
  echo "  Unset the variable to use the default public key, or pass the correct one." >&2
fi

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key><string>$APP_NAME</string>
    <key>CFBundleIdentifier</key><string>$BUNDLE_ID</string>
    <key>CFBundleName</key><string>$APP_NAME</string>
    <key>CFBundleDisplayName</key><string>AlexandriaBar</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>$VERSION</string>
    <key>CFBundleVersion</key><string>$VERSION</string>
    <key>CFBundleIconFile</key><string>AppIcon</string>
    <key>LSMinimumSystemVersion</key><string>14.0</string>
    <key>LSUIElement</key><true/>
    <key>NSHumanReadableCopyright</key><string>Alexandria</string>
    <key>SUFeedURL</key><string>$SPARKLE_FEED_URL_ESCAPED</string>
$SPARKLE_PUBLIC_ED_KEY_PLIST
    <key>SUEnableAutomaticChecks</key><true/>
    <key>SUScheduledCheckInterval</key><integer>86400</integer>
    <key>NSAppTransportSecurity</key>
    <dict>
        <key>NSAllowsLocalNetworking</key><true/>
    </dict>
</dict>
</plist>
PLIST

codesign_args=(--force --sign "$IDENTITY")
if [[ "$IDENTITY" != "-" ]]; then
  codesign_args+=(--options runtime --timestamp)
fi

sign_sparkle_xpc() {
  local xpc="$1"
  local xpc_args=("${codesign_args[@]}")
  case "$(basename "$xpc")" in
    Downloader.xpc|Installer.xpc)
      if codesign --display --entitlements :- "$xpc" >/dev/null 2>&1; then
        xpc_args+=(--preserve-metadata=entitlements)
      fi
      ;;
  esac
  codesign "${xpc_args[@]}" "$xpc"
}

while IFS= read -r -d '' bundled_file; do
  xattr -d com.apple.quarantine "$bundled_file" 2>/dev/null || true
done < <(find "$APP" -type f -print0)

SPARKLE_VERSION_DIR="$APP/Contents/Frameworks/Sparkle.framework/Versions/B"
if [[ -d "$SPARKLE_VERSION_DIR" ]]; then
  shopt -s nullglob
  for xpc in "$SPARKLE_VERSION_DIR"/XPCServices/*.xpc; do
    sign_sparkle_xpc "$xpc"
  done
  shopt -u nullglob
  codesign "${codesign_args[@]}" "$SPARKLE_VERSION_DIR/Autoupdate"
  codesign "${codesign_args[@]}" "$SPARKLE_VERSION_DIR/Updater.app"
  codesign "${codesign_args[@]}" "$APP/Contents/Frameworks/Sparkle.framework"
fi
codesign "${codesign_args[@]}" "$APP/Contents/MacOS/$APP_NAME"
codesign "${codesign_args[@]}" "$APP"
echo "built $APP (signed: $IDENTITY)"
