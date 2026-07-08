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
DIST="dist"
APP="$DIST/$APP_NAME.app"

swift build -c "$CONFIGURATION"

BIN=".build/$CONFIGURATION/$APP_NAME"
[[ -x "$BIN" ]] || { echo "missing binary $BIN" >&2; exit 1; }

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/$APP_NAME"

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
</dict>
</plist>
PLIST

codesign_args=(--force --sign "$IDENTITY")
if [[ "$IDENTITY" != "-" ]]; then
  codesign_args+=(--options runtime --timestamp)
fi

while IFS= read -r -d '' bundled_file; do
  xattr -d com.apple.quarantine "$bundled_file" 2>/dev/null || true
done < <(find "$APP" -type f -print0)

codesign "${codesign_args[@]}" "$APP/Contents/MacOS/$APP_NAME"
codesign "${codesign_args[@]}" "$APP"
echo "built $APP (signed: $IDENTITY)"
