#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN_PATH="$ROOT_DIR/rust/target/release/apw"
SMOKE_ROOT="$ROOT_DIR/dist/host-smoke"
EXTENSION_DIR="$ROOT_DIR/browser-bridge"
EXTENSION_ID="ajefblkpgcffjgeaifmhaekckngflbak"
HOST_NAME="dev.omt.apw.bridge.chromium"
USER_CHROME_MANIFEST="$HOME/Library/Application Support/Google/Chrome/NativeMessagingHosts/${HOST_NAME}.json"
PW_DOMAIN=""
OTP_DOMAIN=""
BIND_HOST="127.0.0.1"
PORT="10000"
PIN_ENV="APW_PIN"
BRIDGE_TIMEOUT_SECONDS=120

print_help() {
  cat <<'EOF'
Usage: ./scripts/browser-host-smoke.sh --pw-domain DOMAIN [OPTIONS]

Run a local macOS browser-backed smoke path for:
  start -> bridge attach -> auth -> pw list -> otp list

Options:
  --pw-domain DOMAIN      Required domain for `apw pw list`
  --otp-domain DOMAIN     Optional domain for `apw otp list` (defaults to --pw-domain)
  --bin PATH              APW binary to test (default: rust/target/release/apw)
  --bind HOST             Daemon bind host (default: 127.0.0.1)
  --port PORT             Daemon/bridge port (default: 10000)
  --pin-env NAME          Environment variable that holds the APW PIN (default: APW_PIN)
  --bridge-timeout SEC    Seconds to wait for the Chrome bridge to attach (default: 120)
  --help                  Show this help message
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --pw-domain)
      PW_DOMAIN="${2:-}"
      shift 2
      ;;
    --otp-domain)
      OTP_DOMAIN="${2:-}"
      shift 2
      ;;
    --bin)
      BIN_PATH="${2:-}"
      shift 2
      ;;
    --bind)
      BIND_HOST="${2:-}"
      shift 2
      ;;
    --port)
      PORT="${2:-}"
      shift 2
      ;;
    --pin-env)
      PIN_ENV="${2:-}"
      shift 2
      ;;
    --bridge-timeout)
      BRIDGE_TIMEOUT_SECONDS="${2:-}"
      shift 2
      ;;
    --help)
      print_help
      exit 0
      ;;
    *)
      echo "Unknown argument: $1"
      print_help
      exit 1
      ;;
  esac
done

if [[ -z "$PW_DOMAIN" ]]; then
  echo "--pw-domain is required."
  exit 1
fi

if [[ -z "$OTP_DOMAIN" ]]; then
  OTP_DOMAIN="$PW_DOMAIN"
fi

if [[ ! -x "$BIN_PATH" ]]; then
  echo "APW binary not found or not executable: $BIN_PATH"
  exit 1
fi

mkdir -p "$SMOKE_ROOT"
TIMESTAMP="$(date -u +%Y%m%dT%H%M%SZ)"
EVIDENCE_DIR="$SMOKE_ROOT/$TIMESTAMP"
mkdir -p "$EVIDENCE_DIR"

status_value() {
  ruby -rjson -e '
    payload = JSON.parse(File.read(ARGV[0]))
    value = ARGV[1].split(".").reduce(payload) { |memo, key| memo.is_a?(Hash) ? memo[key] : nil }
    puts(value.nil? ? "null" : value)
  ' "$1" "$2"
}

command_ok_or_no_results() {
  local status_file="$1"
  local code
  code="$(ruby -rjson -e '
    payload = JSON.parse(File.read(ARGV[0]))
    puts(payload["code"] || -1)
  ' "$status_file")"
  [[ "$code" == "0" || "$code" == "3" ]]
}

manifest_value() {
  local manifest_path="$1"
  local key="$2"
  if [[ ! -f "$manifest_path" ]]; then
    echo "null"
    return 0
  fi
  ruby -rjson -e '
    payload = JSON.parse(File.read(ARGV[0]))
    value = payload[ARGV[1]]
    puts(value.nil? ? "null" : value)
  ' "$manifest_path" "$key"
}

capture_command() {
  local name="$1"
  shift
  set +e
  "$@" >"$EVIDENCE_DIR/${name}.stdout.json" 2>"$EVIDENCE_DIR/${name}.stderr.json"
  local exit_code=$?
  set -e
  printf '%s\n' "$exit_code" >"$EVIDENCE_DIR/${name}.exit"
  if [[ -s "$EVIDENCE_DIR/${name}.stdout.json" ]]; then
    cp "$EVIDENCE_DIR/${name}.stdout.json" "$EVIDENCE_DIR/${name}.json"
  else
    cp "$EVIDENCE_DIR/${name}.stderr.json" "$EVIDENCE_DIR/${name}.json"
  fi
  return 0
}

list_helper_crashes() {
  ls -1t "$HOME/Library/Logs/DiagnosticReports" 2>/dev/null \
    | grep 'PasswordManagerBrowserExtensionHelper' || true
}

cleanup() {
  if [[ -n "${DAEMON_PID:-}" ]]; then
    kill "$DAEMON_PID" >/dev/null 2>&1 || true
    wait "$DAEMON_PID" >/dev/null 2>&1 || true
  fi
}

trap cleanup EXIT

list_helper_crashes >"$EVIDENCE_DIR/helper-crashes.before.txt"

"$BIN_PATH" start --runtime-mode browser --bind "$BIND_HOST" --port "$PORT" \
  >"$EVIDENCE_DIR/daemon.stdout.log" \
  2>"$EVIDENCE_DIR/daemon.stderr.log" &
DAEMON_PID=$!

sleep 1
"$BIN_PATH" status --json >"$EVIDENCE_DIR/status.pre-bridge.json"

echo "Waiting for Chrome bridge attachment. Load the unpacked extension from browser-bridge/ if it is not already active."
deadline=$((SECONDS + BRIDGE_TIMEOUT_SECONDS))
while (( SECONDS < deadline )); do
  "$BIN_PATH" status --json >"$EVIDENCE_DIR/status.bridge-poll.json"
  if [[ "$(status_value "$EVIDENCE_DIR/status.bridge-poll.json" "payload.bridge.status")" == "attached" ]]; then
    cp "$EVIDENCE_DIR/status.bridge-poll.json" "$EVIDENCE_DIR/status.bridge-attached.json"
    break
  fi
  sleep 2
done

if [[ ! -f "$EVIDENCE_DIR/status.bridge-attached.json" ]]; then
  "$BIN_PATH" status --json >"$EVIDENCE_DIR/status.bridge-timeout.json"
  bridge_status="$(status_value "$EVIDENCE_DIR/status.bridge-timeout.json" "payload.bridge.status")"
  preflight_status="$(status_value "$EVIDENCE_DIR/status.bridge-timeout.json" "payload.daemon.preflight.status")"
  manifest_host_name="$(manifest_value "$USER_CHROME_MANIFEST" "name")"
  manifest_helper_path="$(manifest_value "$USER_CHROME_MANIFEST" "path")"
  manifest_exists=0
  helper_executable=0
  if [[ -f "$USER_CHROME_MANIFEST" ]]; then
    manifest_exists=1
  fi
  if [[ "$manifest_helper_path" != "null" && -x "$manifest_helper_path" ]]; then
    helper_executable=1
  fi
  {
    echo "Chrome bridge did not attach within ${BRIDGE_TIMEOUT_SECONDS}s."
    echo "Evidence directory: $EVIDENCE_DIR"
    echo "Bridge status: $bridge_status"
    echo "daemon.preflight.status: $preflight_status"
    echo "User Chrome native messaging manifest exists: $manifest_exists"
    echo "User Chrome native messaging manifest path: $USER_CHROME_MANIFEST"
    echo "User manifest host name: $manifest_host_name"
    echo "User manifest helper path: $manifest_helper_path"
    echo "User manifest helper executable: $helper_executable"
    echo "Expected unpacked extension dir: $EXTENSION_DIR"
    echo "Expected extension ID: $EXTENSION_ID"
    echo
    echo "Remediation:"
    echo "  1. Open chrome://extensions"
    echo "  2. Enable Developer mode"
    echo "  3. Load unpacked extension from: $EXTENSION_DIR"
    echo "  4. Confirm the extension ID is: $EXTENSION_ID"
    echo "  5. Open the extension popup and confirm host=$BIND_HOST port=$PORT"
    echo "  6. Re-run this smoke script"
  } | tee "$EVIDENCE_DIR/summary.txt"
  exit 1
fi

PIN_VALUE="${!PIN_ENV:-}"
if [[ -z "$PIN_VALUE" ]]; then
  read -r -s -p "Enter APW PIN: " PIN_VALUE
  printf '\n'
fi

capture_command auth "$BIN_PATH" --json auth --pin "$PIN_VALUE"
"$BIN_PATH" status --json >"$EVIDENCE_DIR/status.post-auth.json"
capture_command pw-list "$BIN_PATH" --json pw list "$PW_DOMAIN"
capture_command otp-list "$BIN_PATH" --json otp list "$OTP_DOMAIN"
"$BIN_PATH" status --json >"$EVIDENCE_DIR/status.final.json"

list_helper_crashes >"$EVIDENCE_DIR/helper-crashes.after.txt"
comm -13 \
  <(sort "$EVIDENCE_DIR/helper-crashes.before.txt") \
  <(sort "$EVIDENCE_DIR/helper-crashes.after.txt") \
  >"$EVIDENCE_DIR/helper-crashes.new.txt" || true

{
  while IFS= read -r report; do
    [[ -z "$report" ]] && continue
    echo "===== $report ====="
    grep -E '"parentProc"|SIGKILL|Code Signature' "$HOME/Library/Logs/DiagnosticReports/$report" || true
    echo
  done <"$EVIDENCE_DIR/helper-crashes.new.txt"
} >"$EVIDENCE_DIR/helper-crash-diff.txt"

bridge_status="$(status_value "$EVIDENCE_DIR/status.bridge-attached.json" "payload.bridge.status")"
bridge_browser="$(status_value "$EVIDENCE_DIR/status.bridge-attached.json" "payload.bridge.browser")"
session_authenticated="$(status_value "$EVIDENCE_DIR/status.post-auth.json" "payload.session.authenticated")"

pw_ok=0
otp_ok=0
if command_ok_or_no_results "$EVIDENCE_DIR/pw-list.json"; then
  pw_ok=1
fi
if command_ok_or_no_results "$EVIDENCE_DIR/otp-list.json"; then
  otp_ok=1
fi

new_apw_crashes=0
if grep -q '"parentProc"[[:space:]]*:[[:space:]]*"apw"' "$EVIDENCE_DIR/helper-crash-diff.txt"; then
  new_apw_crashes=1
fi

{
  echo "Evidence directory: $EVIDENCE_DIR"
  echo "Bridge status: $bridge_status"
  echo "Bridge browser: $bridge_browser"
  echo "Session authenticated after auth: $session_authenticated"
  echo "pw list acceptable exit/code: $pw_ok"
  echo "otp list acceptable exit/code: $otp_ok"
  echo "New helper crash with parentProc=apw: $new_apw_crashes"
} | tee "$EVIDENCE_DIR/summary.txt"

if [[ "$bridge_status" != "attached" || "$bridge_browser" != "chrome" ]]; then
  echo "Bridge attach success criteria failed." >&2
  exit 1
fi

if [[ "$session_authenticated" != "true" ]]; then
  echo "Auth success criteria failed." >&2
  exit 1
fi

if [[ "$pw_ok" != "1" || "$otp_ok" != "1" ]]; then
  echo "pw/otp success criteria failed." >&2
  exit 1
fi

if [[ "$new_apw_crashes" != "0" ]]; then
  echo "Detected new helper crash reports with parentProc=apw." >&2
  exit 1
fi

echo "Browser host smoke completed successfully."
