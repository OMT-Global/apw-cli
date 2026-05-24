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

run_step() {
  local name="$1"
  shift
  echo
  echo "==> $name"
  "$@"
}

# Issue #40: the macOS runner does not guarantee Homebrew/pkg-config.
# Build OpenSSL through the crate's vendored feature instead, and fail early
# with a clear prerequisite message if the runner lacks source-build tools.
ensure_vendored_openssl_build_inputs() {
  [[ "${OSTYPE:-}" == darwin* || "${APW_FORCE_OPENSSL_INPUT_CHECK:-}" == "1" ]] || return 0

  local missing=()
  for tool in cc make perl; do
    if ! command -v "$tool" >/dev/null 2>&1; then
      missing+=("$tool")
    fi
  done

  if ((${#missing[@]} > 0)); then
    echo "Missing required tool(s) for vendored OpenSSL build: ${missing[*]}" >&2
    echo "Install Xcode Command Line Tools and Perl on the macOS runner." >&2
    echo "If intentionally using system OpenSSL, set OPENSSL_NO_VENDOR=1 and provide OPENSSL_DIR/PKG_CONFIG_PATH." >&2
    exit 1
  fi
}

ensure_vendored_openssl_build_inputs

if [[ "${1:-}" == "--check-build-inputs" ]]; then
  echo "Vendored OpenSSL build inputs are available."
  exit 0
fi

require_tool cargo
require_tool swift

run_step "Rust native app end-to-end tests" \
  cargo test --manifest-path rust/Cargo.toml --test native_app_e2e

run_step "Rust security regression tests" \
  cargo test --manifest-path rust/Cargo.toml --test security_regressions

run_step "Rust clippy" \
  cargo clippy --manifest-path rust/Cargo.toml --all-targets -- -D warnings

run_step "Swift native app release build" \
  ./scripts/build-native-app.sh

echo
echo "APW extended validation passed."
