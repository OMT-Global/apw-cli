#!/usr/bin/env bash
set -euo pipefail

SCRIPT="scripts/ci/run-native-app-preflight.sh"

require_line() {
  local pattern="$1"
  local message="$2"
  if ! grep -Fq "$pattern" "$SCRIPT"; then
    echo "$message" >&2
    exit 1
  fi
}

require_line "xcodebuild" "Native app preflight must run xcodebuild tests."
require_line "codesign --verify --deep --strict" "Native app preflight must verify the signed app bundle."
require_line "codesign -d --entitlements :-" "Native app preflight must extract embedded entitlements."
require_line "Print :com.apple.developer.associated-domains" "Native app preflight must compare associated-domain entitlements."
require_line "cmp -s \"\$expected_domains_file\" \"\$actual_domains_file\"" "Native app preflight must fail on entitlement drift."

echo "Native app preflight contract test passed."
