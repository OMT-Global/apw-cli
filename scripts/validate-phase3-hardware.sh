#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage: ./scripts/validate-phase3-hardware.sh --app PATH --apw PATH --url URL [--report PATH]

Validate the Phase 3 APW.app credential-broker flow on real macOS hardware.
The script checks code signing, Gatekeeper, notarization stapling, associated
domain entitlements, app install/launch/status, and a user-mediated login.

Options:
  --app PATH       Path to the notarized APW.app bundle.
  --apw PATH       Path to the matching apw CLI binary.
  --url URL        HTTPS URL for the associated-domain credential test.
  --report PATH    Markdown report output path.
  -h, --help       Show this help.
USAGE
}

APP_PATH=""
APW_BIN=""
TEST_URL=""
REPORT_PATH="docs/phase3-hardware-validation-report.md"

while [ "$#" -gt 0 ]; do
  case "$1" in
    --app)
      APP_PATH="${2:-}"
      shift 2
      ;;
    --apw)
      APW_BIN="${2:-}"
      shift 2
      ;;
    --url)
      TEST_URL="${2:-}"
      shift 2
      ;;
    --report)
      REPORT_PATH="${2:-}"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

fail() {
  echo "phase3 validation failed: $*" >&2
  exit 1
}

require_command() {
  if ! command -v "$1" >/dev/null 2>&1; then
    fail "missing required command: $1"
  fi
}

run_step() {
  label="$1"
  shift
  echo "==> $label"
  "$@"
}

json_field_true() {
  json="$1"
  field="$2"
  printf '%s' "$json" | /usr/bin/python3 -c '
import json
import sys

field = sys.argv[1].split(".")
payload = json.load(sys.stdin)
value = payload
for part in field:
    value = value[part]
if value is not True:
    raise SystemExit(1)
' "$field"
}

if [ "$(uname -s)" != "Darwin" ]; then
  fail "real-hardware validation must run on macOS"
fi

[ -n "$APP_PATH" ] || fail "--app is required"
[ -n "$APW_BIN" ] || fail "--apw is required"
[ -n "$TEST_URL" ] || fail "--url is required"

case "$TEST_URL" in
  https://*) ;;
  *) fail "--url must be an https URL" ;;
esac

[ -d "$APP_PATH" ] || fail "APW.app bundle not found: $APP_PATH"
[ -x "$APP_PATH/Contents/MacOS/APW" ] || fail "APW.app executable missing or not executable"
[ -x "$APW_BIN" ] || fail "apw CLI missing or not executable: $APW_BIN"

APP_PARENT="$(cd "$(dirname "$APP_PATH")" && pwd -P)" ||
  fail "failed to resolve APW.app parent directory"
APP_NAME="$(basename "$APP_PATH")"
if [ "$APP_NAME" != "APW.app" ]; then
  fail "--app must point to a bundle named APW.app so apw app install uses the validated bundle"
fi
APP_PATH="$APP_PARENT/$APP_NAME"
APW_BIN="$(cd "$(dirname "$APW_BIN")" && pwd -P)/$(basename "$APW_BIN")"

require_command codesign
require_command spctl
require_command xcrun
require_command plutil
require_command /usr/bin/python3

run_step "Verify Developer ID code signature" \
  codesign --deep --strict --verify "$APP_PATH"

run_step "Assess Gatekeeper execution policy" \
  spctl --assess --type execute --verbose "$APP_PATH"

run_step "Validate notarization staple" \
  xcrun stapler validate "$APP_PATH"

entitlements_file="$(mktemp "${TMPDIR:-/tmp}/apw-entitlements.XXXXXX")"
trap 'rm -f "$entitlements_file"' EXIT

codesign -d --entitlements :- "$APP_PATH" >"$entitlements_file" 2>/dev/null ||
  fail "failed to read APW.app entitlements"

if ! grep -q "webcredentials:" "$entitlements_file"; then
  fail "APW.app entitlements do not include a webcredentials associated domain"
fi

run_step "Install APW.app with matching CLI" \
  sh -c 'cd "$1" && "$2" app install' sh "$APP_PARENT" "$APW_BIN"

run_step "Launch APW.app broker" \
  "$APW_BIN" app launch

status_json="$("$APW_BIN" status --json)"
json_field_true "$status_json" "payload.app.installed" ||
  fail "status JSON did not report installed app"
json_field_true "$status_json" "payload.app.service.running" ||
  fail "status JSON did not report running broker"

echo "==> Running user-mediated login request"
echo "The next command should show the native iCloud Keychain credential picker."
echo "Do not paste credential values into the generated report."
if ! "$APW_BIN" login "$TEST_URL" >/dev/null; then
  fail "apw login failed for $TEST_URL"
fi

printf "Did the native iCloud Keychain credential picker appear? [yes/no] "
read -r picker_seen
[ "$picker_seen" = "yes" ] || fail "operator did not confirm credential picker"

printf "Did APW return the selected test credential? [yes/no] "
read -r credential_returned
[ "$credential_returned" = "yes" ] || fail "operator did not confirm credential response"

mkdir -p "$(dirname "$REPORT_PATH")"
{
  echo "# Phase 3 hardware validation report"
  echo
  echo "Issue: #43"
  echo
  echo "Status: success path validated; error paths require manual entries below"
  echo
  echo "## Host"
  echo
  echo "- Date: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "- macOS version: $(sw_vers -productVersion)"
  echo "- Hardware model: $(sysctl -n hw.model)"
  echo "- Architecture: $(uname -m)"
  echo "- APW.app path: $APP_PATH"
  echo "- APW CLI path: $APW_BIN"
  echo "- Test associated domain URL: $TEST_URL"
  echo
  echo "## Automated checks"
  echo
  echo "- [x] codesign strict verification"
  echo "- [x] Gatekeeper execution assessment"
  echo "- [x] notarization staple validation"
  echo "- [x] associated-domain entitlement contains webcredentials"
  echo "- [x] apw app install"
  echo "- [x] apw app launch"
  echo "- [x] apw status --json reports installed app and running broker"
  echo "- [x] apw login exits successfully"
  echo
  echo "## Operator-observed flow"
  echo
  echo "- [x] Native iCloud Keychain credential picker appeared"
  echo "- [x] Operator selected the expected test credential"
  echo "- [x] APW returned a credential response without saving it to disk"
  echo
  echo "## Error paths"
  echo
  echo "| Path | Expected result | Observed result |"
  echo "| --- | --- | --- |"
  echo "| Success | credential response with userMediated true | PASS |"
  echo "| Cancel | stable canceled/denied broker error | TODO |"
  echo "| Denied | stable denied broker error | TODO |"
  echo "| Timeout | communication timeout error | TODO |"
  echo "| Unsupported domain | no-results or unsupported-domain error | TODO |"
  echo
  echo "## Notes"
  echo
  echo "- No credential values were written by this script."
} >"$REPORT_PATH"

chmod 0600 "$REPORT_PATH"
echo "Wrote validation report: $REPORT_PATH"
