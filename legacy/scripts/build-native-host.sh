#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PACKAGE_DIR="$ROOT_DIR/native-host"
APP_NAME="APWNativeHost.app"
EXECUTABLE_NAME="APWNativeHost"
DIST_DIR="$PACKAGE_DIR/dist"
APP_DIR="$DIST_DIR/$APP_NAME"
CONTENTS_DIR="$APP_DIR/Contents"
MACOS_DIR="$CONTENTS_DIR/MacOS"
PLIST_PATH="$CONTENTS_DIR/Info.plist"
EXECUTABLE_PATH="$PACKAGE_DIR/.build/release/$EXECUTABLE_NAME"
VERSION="$(sed -n 's/^version = \"\\([^\"]*\\)\"$/\\1/p' "$ROOT_DIR/rust/Cargo.toml" | head -n1)"

if [[ -z "$VERSION" ]]; then
  echo "Unable to determine APW version from rust/Cargo.toml" >&2
  exit 1
fi

swift build --package-path "$PACKAGE_DIR" -c release

rm -rf "$APP_DIR"
mkdir -p "$MACOS_DIR"
cp "$EXECUTABLE_PATH" "$MACOS_DIR/$EXECUTABLE_NAME"
chmod 0755 "$MACOS_DIR/$EXECUTABLE_NAME"

cat >"$PLIST_PATH" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key>
  <string>en</string>
  <key>CFBundleExecutable</key>
  <string>$EXECUTABLE_NAME</string>
  <key>CFBundleIdentifier</key>
  <string>dev.omt.apw.nativehost</string>
  <key>CFBundleInfoDictionaryVersion</key>
  <string>6.0</string>
  <key>CFBundleName</key>
  <string>APWNativeHost</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>$VERSION</string>
  <key>CFBundleVersion</key>
  <string>$VERSION</string>
  <key>LSBackgroundOnly</key>
  <true/>
</dict>
</plist>
EOF

if command -v codesign >/dev/null 2>&1; then
  codesign -s - --force --deep "$APP_DIR" >/dev/null 2>&1 || true
fi

echo "$APP_DIR"
