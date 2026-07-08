#!/usr/bin/env bash
set -euo pipefail

# Builds AlexandriaBar.app into macos/dist/.
#   CONFIGURATION=debug|release (default release)
#   IDENTITY="Developer ID Application: ..." (default adhoc "-")
# Adhoc-signed rebuilds change code identity each build, which resets
# Little Snitch rules; use a stable IDENTITY if that bites.

cd "$(dirname "$0")/.."

CONFIGURATION="${CONFIGURATION:-release}"
IDENTITY="${IDENTITY:--}"
APP_NAME="AlexandriaBar"
BUNDLE_ID="com.alexandria.bar"
VERSION="0.1.0"
DIST="dist"
APP="$DIST/$APP_NAME.app"

swift build -c "$CONFIGURATION"

BIN=".build/$CONFIGURATION/$APP_NAME"
[[ -x "$BIN" ]] || { echo "missing binary $BIN" >&2; exit 1; }

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/$APP_NAME"

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
    <key>LSMinimumSystemVersion</key><string>14.0</string>
    <key>LSUIElement</key><true/>
    <key>NSHumanReadableCopyright</key><string>Alexandria</string>
</dict>
</plist>
PLIST

codesign --force --sign "$IDENTITY" "$APP"
echo "built $APP (signed: $IDENTITY)"
