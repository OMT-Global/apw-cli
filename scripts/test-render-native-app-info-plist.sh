#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK_DIR="$(mktemp -d)"
cleanup() {
  rm -rf "$WORK_DIR"
}
trap cleanup EXIT

base_plist="$WORK_DIR/base/Info.plist"
sparkle_plist="$WORK_DIR/sparkle/Info.plist"
feed_url="https://github.com/OMT-Global/apw-cli/releases/latest/download/appcast.xml"

"$ROOT_DIR/scripts/render-native-app-info-plist.sh" "$base_plist" "9.9.9" "APW"
grep -q "<key>CFBundleShortVersionString</key>" "$base_plist"
grep -q "<string>9.9.9</string>" "$base_plist"
if grep -q "<key>SUPublicEDKey</key>" "$base_plist"; then
  echo "Sparkle public key must not be rendered without APW_SPARKLE_PUBLIC_ED_KEY." >&2
  exit 1
fi

APW_SPARKLE_PUBLIC_ED_KEY="test-ed25519-public-key" \
  "$ROOT_DIR/scripts/render-native-app-info-plist.sh" "$sparkle_plist" "9.9.9" "APW"

grep -q "<key>SUFeedURL</key>" "$sparkle_plist"
grep -q "<string>$feed_url</string>" "$sparkle_plist"
grep -q "<key>SUPublicEDKey</key>" "$sparkle_plist"
grep -q "<string>test-ed25519-public-key</string>" "$sparkle_plist"
grep -q "<key>SUVerifyUpdateBeforeExtraction</key>" "$sparkle_plist"
grep -q "<key>SURequireSignedFeed</key>" "$sparkle_plist"
grep -q "<key>SUAllowsAutomaticUpdates</key>" "$sparkle_plist"
grep -q "<false/>" "$sparkle_plist"

if command -v plutil >/dev/null 2>&1; then
  plutil -lint "$base_plist" >/dev/null
  plutil -lint "$sparkle_plist" >/dev/null
fi

echo "Native app Info.plist renderer test passed."
