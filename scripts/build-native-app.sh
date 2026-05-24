#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PACKAGE_DIR="$ROOT_DIR/native-app"
APP_NAME="APW.app"
EXECUTABLE_NAME="APW"
DIST_DIR="$PACKAGE_DIR/dist"
APP_DIR="$DIST_DIR/$APP_NAME"
CONTENTS_DIR="$APP_DIR/Contents"
MACOS_DIR="$CONTENTS_DIR/MacOS"
RESOURCES_DIR="$CONTENTS_DIR/Resources"
FRAMEWORKS_DIR="$CONTENTS_DIR/Frameworks"
PLIST_PATH="$CONTENTS_DIR/Info.plist"
EXECUTABLE_PATH="$PACKAGE_DIR/.build/release/$EXECUTABLE_NAME"
VERSION="$(awk -F ' = ' '$1 == "version" { gsub(/"/, "", $2); print $2; exit }' "$ROOT_DIR/rust/Cargo.toml")"
PLIST_RENDERER="$ROOT_DIR/scripts/render-native-app-info-plist.sh"

if [[ -z "$VERSION" ]]; then
  echo "Unable to determine APW version from rust/Cargo.toml" >&2
  exit 1
fi

if [[ ! -x "$PLIST_RENDERER" ]]; then
  echo "Expected Info.plist renderer not found or not executable: $PLIST_RENDERER" >&2
  exit 1
fi

swift build --package-path "$PACKAGE_DIR" -c release

rm -rf "$APP_DIR"
mkdir -p "$MACOS_DIR" "$RESOURCES_DIR"
cp "$EXECUTABLE_PATH" "$MACOS_DIR/$EXECUTABLE_NAME"
chmod 0755 "$MACOS_DIR/$EXECUTABLE_NAME"

RESOURCE_BUNDLE="$(find "$PACKAGE_DIR/.build" -path '*/release/*.bundle' -type d -name '*NativeAppLib*.bundle' | head -n 1 || true)"
if [[ -n "$RESOURCE_BUNDLE" ]]; then
  cp -R "$RESOURCE_BUNDLE" "$RESOURCES_DIR/$(basename "$RESOURCE_BUNDLE")"
fi

if otool -L "$MACOS_DIR/$EXECUTABLE_NAME" | grep -q '@rpath/Sparkle.framework/'; then
  SPARKLE_FRAMEWORK="$(find "$PACKAGE_DIR/.build" -path '*/release/Sparkle.framework' -type d | head -n 1 || true)"
  if [[ -z "$SPARKLE_FRAMEWORK" ]]; then
    echo "APW links Sparkle.framework but SwiftPM did not produce a release framework." >&2
    exit 1
  fi
  mkdir -p "$FRAMEWORKS_DIR"
  if command -v ditto >/dev/null 2>&1; then
    ditto "$SPARKLE_FRAMEWORK" "$FRAMEWORKS_DIR/Sparkle.framework"
  else
    cp -R "$SPARKLE_FRAMEWORK" "$FRAMEWORKS_DIR/"
  fi
  if command -v install_name_tool >/dev/null 2>&1; then
    if ! otool -l "$MACOS_DIR/$EXECUTABLE_NAME" | grep -q '@loader_path/../Frameworks'; then
      install_name_tool -add_rpath '@loader_path/../Frameworks' "$MACOS_DIR/$EXECUTABLE_NAME"
    fi
  fi
fi

"$PLIST_RENDERER" "$PLIST_PATH" "$VERSION" "$EXECUTABLE_NAME"

if command -v codesign >/dev/null 2>&1; then
  if ! codesign -s - --force --deep "$APP_DIR" 2>/dev/null; then
    echo "Warning: ad-hoc code signing failed for $APP_DIR. The bundle may be rejected by Gatekeeper." >&2
  fi
fi

echo "$APP_DIR"
