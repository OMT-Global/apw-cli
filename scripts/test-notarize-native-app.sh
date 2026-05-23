#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tmp_dir="$(mktemp -d)"
cleanup() {
  rm -rf "$tmp_dir"
}
trap cleanup EXIT

fake_app="$tmp_dir/APW.app"
mkdir -p "$fake_app/Contents/MacOS"
printf '#!/usr/bin/env sh\nexit 0\n' >"$fake_app/Contents/MacOS/APW"
chmod 0755 "$fake_app/Contents/MacOS/APW"

required_log="$tmp_dir/notary-required.log"
optional_log="$tmp_dir/notary-optional.log"
dry_run_log="$tmp_dir/notary-dry-run.log"

if APW_NOTARIZE_REQUIRED=1 APW_APP_BUNDLE_PATH="$fake_app" "$ROOT_DIR/scripts/notarize-native-app.sh" >"$required_log" 2>&1; then
  echo "notarize-native-app accepted missing required credentials." >&2
  exit 1
fi
grep -q "APPLE_DEVELOPER_CERT_P12" "$required_log"

APW_APP_BUNDLE_PATH="$fake_app" "$ROOT_DIR/scripts/notarize-native-app.sh" >"$optional_log" 2>&1
grep -q "Skipping notarization" "$optional_log"

env \
  APW_NOTARIZE_DRY_RUN=1 \
  APW_APP_BUNDLE_PATH="$fake_app" \
  APPLE_DEVELOPER_CERT_P12=dGVzdA== \
  APPLE_CERT_PASSWORD=test \
  APPLE_TEAM_ID=ABCDE12345 \
  APPLE_NOTARY_KEY_ID=KEYID12345 \
  APPLE_NOTARY_KEY_ISSUER=00000000-0000-0000-0000-000000000000 \
  APPLE_NOTARY_PRIVATE_KEY=dGVzdA== \
  "$ROOT_DIR/scripts/notarize-native-app.sh" >"$dry_run_log" 2>&1
grep -q "dry run requested" "$dry_run_log"

echo "Notarization script tests passed."
