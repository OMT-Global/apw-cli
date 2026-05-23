#!/usr/bin/env bash
set -euo pipefail

print_help() {
  cat <<'HELP'
Usage: ./scripts/verify-universal-binaries.sh [CLI_PATH] [APP_EXECUTABLE_PATH]

Verify that APW release Mach-O binaries contain both arm64 and x86_64 slices.

Defaults:
  CLI_PATH: rust/target/release/apw
  APP_EXECUTABLE_PATH: native-app/dist/APW.app/Contents/MacOS/APW
HELP
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  print_help
  exit 0
fi

ROOT_DIR="$(cd -- "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CLI_PATH="${1:-$ROOT_DIR/rust/target/release/apw}"
APP_EXECUTABLE_PATH="${2:-$ROOT_DIR/native-app/dist/APW.app/Contents/MacOS/APW}"
LIPO_BIN="${LIPO_BIN:-lipo}"

if ! command -v "$LIPO_BIN" >/dev/null 2>&1; then
  echo "lipo not found. Universal binary verification must run on macOS with Xcode command line tools." >&2
  exit 1
fi

verify_binary() {
  local label="$1"
  local path="$2"

  if [[ ! -x "$path" ]]; then
    echo "$label binary is missing or not executable: $path" >&2
    exit 1
  fi

  local archs
  archs="$("$LIPO_BIN" -archs "$path")"
  for required in arm64 x86_64; do
    if [[ " $archs " != *" $required "* ]]; then
      echo "$label binary is not universal; expected arm64 and x86_64, got: $archs" >&2
      exit 1
    fi
  done
  echo "$label: $archs"
}

verify_binary "apw" "$CLI_PATH"
verify_binary "APW.app" "$APP_EXECUTABLE_PATH"
