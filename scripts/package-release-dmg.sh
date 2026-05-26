#!/usr/bin/env bash
set -euo pipefail

print_help() {
  cat <<'HELP'
Usage: ./scripts/package-release-dmg.sh VERSION_OR_TAG

Create dist/apw-macos-<VERSION_OR_TAG>.dmg and a matching .sha256 file.

The DMG contains:
  - APW.app/
  - bin/apw
  - Applications -> /Applications

Prerequisites:
  - cargo release binary at rust/target/release/apw
  - native app bundle at native-app/dist/APW.app
  - hdiutil on macOS
HELP
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  print_help
  exit 0
fi

if [[ $# -ne 1 || -z "${1:-}" ]]; then
  echo "VERSION_OR_TAG is required." >&2
  print_help >&2
  exit 1
fi

ROOT_DIR="$(cd -- "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VERSION_OR_TAG="$1"
BIN_PATH="$ROOT_DIR/rust/target/release/apw"
APP_DIR="$ROOT_DIR/native-app/dist/APW.app"
DIST_DIR="$ROOT_DIR/dist"
STAGING_DIR="$DIST_DIR/dmg-staging"
DMG_PATH="$DIST_DIR/apw-macos-${VERSION_OR_TAG}.dmg"
CHECKSUM_PATH="$DMG_PATH.sha256"
MOUNT_DIR="$DIST_DIR/dmg-mount"
HDIUTIL_BIN="${HDIUTIL_BIN:-hdiutil}"

if ! command -v "$HDIUTIL_BIN" >/dev/null 2>&1; then
  echo "hdiutil not found. DMG packaging must run on macOS." >&2
  exit 1
fi

if [[ ! -x "$BIN_PATH" ]]; then
  echo "Missing executable release binary: $BIN_PATH" >&2
  echo "Build it first with: cargo build --manifest-path rust/Cargo.toml --release" >&2
  exit 1
fi

if [[ ! -d "$APP_DIR" ]]; then
  echo "Missing native app bundle: $APP_DIR" >&2
  echo "Build it first with: ./scripts/build-native-app.sh" >&2
  exit 1
fi

if [[ ! -f "$APP_DIR/Contents/Info.plist" || ! -x "$APP_DIR/Contents/MacOS/APW" ]]; then
  echo "Native app bundle is incomplete: $APP_DIR" >&2
  exit 1
fi

cleanup() {
  if mount | grep -q " on $MOUNT_DIR "; then
    "$HDIUTIL_BIN" detach "$MOUNT_DIR" >/dev/null 2>&1 || true
  fi
  rm -rf "$STAGING_DIR" "$MOUNT_DIR"
}
trap cleanup EXIT

rm -rf "$STAGING_DIR" "$MOUNT_DIR" "$DMG_PATH" "$CHECKSUM_PATH"
mkdir -p "$STAGING_DIR/bin" "$DIST_DIR"

cp "$BIN_PATH" "$STAGING_DIR/bin/apw"
cp -R "$APP_DIR" "$STAGING_DIR/APW.app"
ln -s /Applications "$STAGING_DIR/Applications"

"$HDIUTIL_BIN" create \
  -volname "APW ${VERSION_OR_TAG}" \
  -srcfolder "$STAGING_DIR" \
  -ov \
  -format UDZO \
  "$DMG_PATH"

if [[ "${APW_SKIP_DMG_MOUNT_SMOKE:-0}" != "1" ]]; then
  mkdir -p "$MOUNT_DIR"
  "$HDIUTIL_BIN" attach -nobrowse -readonly -mountpoint "$MOUNT_DIR" "$DMG_PATH" >/dev/null
  test -x "$MOUNT_DIR/bin/apw"
  test -f "$MOUNT_DIR/APW.app/Contents/Info.plist"
  test -x "$MOUNT_DIR/APW.app/Contents/MacOS/APW"
  "$HDIUTIL_BIN" detach "$MOUNT_DIR" >/dev/null
fi

(
  cd "$DIST_DIR"
  shasum -a 256 "$(basename "$DMG_PATH")" > "$(basename "$CHECKSUM_PATH")"
)

echo "$DMG_PATH"
echo "$CHECKSUM_PATH"
