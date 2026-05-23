#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

require_tool() {
  local tool="$1"
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "Missing required tool: $tool" >&2
    exit 1
  fi
}

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "Native app preflight requires macOS." >&2
  exit 1
fi

require_tool xcodebuild
require_tool swift
require_tool codesign
require_tool plutil

(
  cd native-app
  xcodebuild \
    -scheme APW-Package \
    -destination 'platform=macOS' \
    -derivedDataPath .xcode-derived \
    test
)

./scripts/build-native-app.sh >/dev/null

app_dir="native-app/dist/APW.app"
entitlements_file="$(mktemp "${TMPDIR:-/tmp}/apw-entitlements.XXXXXX")"
trap 'rm -f "$entitlements_file"' EXIT

codesign --verify --deep --strict --verbose=2 "$app_dir"
codesign -d --entitlements :- "$app_dir" >"$entitlements_file" 2>/dev/null

if ! plutil -lint "$entitlements_file" >/dev/null; then
  echo "APW.app entitlements are not valid plist output." >&2
  exit 1
fi

if ! grep -q 'webcredentials:example.com' "$entitlements_file"; then
  echo "APW.app is missing the webcredentials:example.com entitlement." >&2
  exit 1
fi

echo "Native app xcodebuild, codesign, and entitlement preflight passed."
