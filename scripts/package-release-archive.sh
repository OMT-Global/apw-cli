#!/usr/bin/env bash
set -euo pipefail

print_help() {
  cat <<'HELP'
Usage: ./scripts/package-release-archive.sh VERSION_OR_TAG

Create dist/apw-macos-<VERSION_OR_TAG>.tar.gz containing:
  - apw
  - APW.app/

Prerequisites:
  - cargo release binary at rust/target/release/apw
  - native app bundle at native-app/dist/APW.app
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
STAGING_DIR="$DIST_DIR/release-archive-staging"
ARCHIVE_PATH="$DIST_DIR/apw-macos-${VERSION_OR_TAG}.tar.gz"

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

rm -rf "$STAGING_DIR"
mkdir -p "$STAGING_DIR"
cp "$BIN_PATH" "$STAGING_DIR/apw"
cp -R "$APP_DIR" "$STAGING_DIR/APW.app"

tar -czf "$ARCHIVE_PATH" -C "$STAGING_DIR" apw APW.app
rm -rf "$STAGING_DIR"

echo "$ARCHIVE_PATH"
