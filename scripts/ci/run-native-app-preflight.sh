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

PLIST_BUDDY="/usr/libexec/PlistBuddy"
if [[ ! -x "$PLIST_BUDDY" ]]; then
  echo "Missing required tool: $PLIST_BUDDY" >&2
  exit 1
fi

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
source_entitlements="native-app/APW.entitlements"
entitlements_file="$(mktemp "${TMPDIR:-/tmp}/apw-entitlements.XXXXXX")"
expected_domains_file="$(mktemp "${TMPDIR:-/tmp}/apw-expected-domains.XXXXXX")"
actual_domains_file="$(mktemp "${TMPDIR:-/tmp}/apw-actual-domains.XXXXXX")"
trap 'rm -f "$entitlements_file" "$expected_domains_file" "$actual_domains_file"' EXIT

codesign --verify --deep --strict --verbose=2 "$app_dir"
codesign -d --entitlements :- "$app_dir" >"$entitlements_file" 2>/dev/null

if ! plutil -lint "$source_entitlements" >/dev/null; then
  echo "APW source entitlements are not valid plist output." >&2
  exit 1
fi

if ! plutil -lint "$entitlements_file" >/dev/null; then
  echo "APW.app entitlements are not valid plist output." >&2
  exit 1
fi

"$PLIST_BUDDY" -c "Print :com.apple.developer.associated-domains" "$source_entitlements" >"$expected_domains_file"
"$PLIST_BUDDY" -c "Print :com.apple.developer.associated-domains" "$entitlements_file" >"$actual_domains_file"

if ! cmp -s "$expected_domains_file" "$actual_domains_file"; then
  echo "APW.app associated-domain entitlements differ from native-app/APW.entitlements." >&2
  diff -u "$expected_domains_file" "$actual_domains_file" >&2 || true
  exit 1
fi

echo "Native app xcodebuild, codesign, and entitlement preflight passed."
