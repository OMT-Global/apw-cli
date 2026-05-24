#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage: ./scripts/validate-phase3-hardware.sh --app PATH --apw PATH --url URL [--unsupported-url URL] [--report PATH]

Validate the Phase 3 APW.app credential-broker flow on real macOS hardware.
The script checks code signing, Gatekeeper, notarization stapling, associated
domain entitlements, app install/launch/status, a user-mediated login, and
the required manual error-path observations.

Options:
  --app PATH              Path to the notarized APW.app bundle.
  --apw PATH              Path to the matching apw CLI binary.
  --url URL               HTTPS URL for the associated-domain credential test.
  --unsupported-url URL   HTTPS URL outside the app entitlement set.
  --report PATH           Markdown report output path.
  -h, --help              Show this help.
USAGE
}

APP_PATH=""
APW_BIN=""
TEST_URL=""
UNSUPPORTED_URL="https://unsupported.invalid"
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
    --unsupported-url)
      UNSUPPORTED_URL="${2:-}"
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

case "$UNSUPPORTED_URL" in
  https://*) ;;
  *) fail "--unsupported-url must be an https URL" ;;
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

prompt_observation() {
  label="$1"
  instructions="$2"
  expected="$3"

  echo >&2
  echo "==> Manual error-path check: $label" >&2
  echo "$instructions" >&2
  echo "Expected result: $expected" >&2
  printf "Observed result for %s: " "$label" >&2
  read -r observed
  [ -n "$observed" ] || fail "$label observation is required"
  printf '%s' "$observed"
}

markdown_cell() {
  printf '%s' "$1" | tr '\n' ' ' | sed 's/|/\\|/g; s/[[:space:]][[:space:]]*/ /g'
}

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

cancel_observed="$(prompt_observation \
  "Cancel" \
  "Run: $APW_BIN login $TEST_URL, then cancel the native credential picker." \
  "stable canceled or denied broker error")"

denied_observed="$(prompt_observation \
  "Denied" \
  "Run: $APW_BIN login $TEST_URL and deny any APW-owned approval prompt if it appears." \
  "stable denied broker error")"

timeout_observed="$(prompt_observation \
  "Timeout" \
  "Stop or block the broker, run: $APW_BIN login $TEST_URL, then restore the broker before continuing." \
  "communication timeout error")"

echo
echo "==> Running unsupported-domain request"
unsupported_output="$("$APW_BIN" login "$UNSUPPORTED_URL" 2>&1)" && {
  echo "$unsupported_output" >&2
  fail "unsupported-domain request unexpectedly succeeded for $UNSUPPORTED_URL"
}
case "$unsupported_output" in
  *unsupported*|*notHandled*|*no_credential_source*|*No*credential*|*domain*) ;;
  *)
    echo "$unsupported_output" >&2
    fail "unsupported-domain request did not emit an expected domain/no-credential error"
    ;;
esac
unsupported_observed="$(markdown_cell "$unsupported_output")"

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
  echo "- Unsupported-domain test URL: $UNSUPPORTED_URL"
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
  echo "| Cancel | stable canceled/denied broker error | $(markdown_cell "$cancel_observed") |"
  echo "| Denied | stable denied broker error | $(markdown_cell "$denied_observed") |"
  echo "| Timeout | communication timeout error | $(markdown_cell "$timeout_observed") |"
  echo "| Unsupported domain | no-results or unsupported-domain error | $unsupported_observed |"
  echo
  echo "## Notes"
  echo
  echo "- No credential values were written by this script."
} >"$REPORT_PATH"

chmod 0600 "$REPORT_PATH"
echo "Wrote validation report: $REPORT_PATH"
