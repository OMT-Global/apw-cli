#!/usr/bin/env bash
set -euo pipefail

print_help() {
  cat <<'HELP'
Usage: ./scripts/build-universal-release.sh

Build universal arm64 + x86_64 release binaries:
  - rust/target/release/apw
  - native-app/dist/APW.app/Contents/MacOS/APW

Requires macOS, rustup, cargo, SwiftPM, lipo, and vendored OpenSSL build tools.
HELP
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  print_help
  exit 0
fi

CHECK_BUILD_INPUTS=0
if [[ "${1:-}" == "--check-build-inputs" ]]; then
  CHECK_BUILD_INPUTS=1
  shift
fi

if [[ $# -ne 0 ]]; then
  echo "Unexpected arguments: $*" >&2
  print_help >&2
  exit 1
fi

ROOT_DIR="$(cd -- "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CARGO_MANIFEST="$ROOT_DIR/rust/Cargo.toml"
CARGO_BIN="${CARGO_BIN:-cargo}"
RUSTUP_BIN="${RUSTUP_BIN:-rustup}"
LIPO_BIN="${LIPO_BIN:-lipo}"
TARGETS=(aarch64-apple-darwin x86_64-apple-darwin)

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
    echo "Install Xcode Command Line Tools and Perl on the macOS release runner." >&2
    echo "If intentionally using system OpenSSL, set OPENSSL_NO_VENDOR=1 and provide OPENSSL_DIR/PKG_CONFIG_PATH." >&2
    exit 1
  fi
}

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "Universal macOS release builds must run on macOS." >&2
  exit 1
fi

ensure_vendored_openssl_build_inputs

if [[ "$CHECK_BUILD_INPUTS" -eq 1 ]]; then
  echo "Universal release build inputs are available."
  exit 0
fi

if ! command -v "$RUSTUP_BIN" >/dev/null 2>&1 && [[ -x "$HOME/.cargo/bin/rustup" ]]; then
  RUSTUP_BIN="$HOME/.cargo/bin/rustup"
fi

RUSTUP_PATH="$(command -v "$RUSTUP_BIN" || true)"
if [[ "$CARGO_BIN" == "cargo" && -n "$RUSTUP_PATH" && -x "$(dirname "$RUSTUP_PATH")/cargo" ]]; then
  CARGO_BIN="$(dirname "$RUSTUP_PATH")/cargo"
fi

for tool in "$CARGO_BIN" "$RUSTUP_BIN" swift "$LIPO_BIN"; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "Required tool not found: $tool" >&2
    exit 1
  fi
done

for target in "${TARGETS[@]}"; do
  "$RUSTUP_BIN" target add "$target"
  "$CARGO_BIN" build --manifest-path "$CARGO_MANIFEST" --release --target "$target"
done

"$LIPO_BIN" -create \
  "$ROOT_DIR/rust/target/aarch64-apple-darwin/release/apw" \
  "$ROOT_DIR/rust/target/x86_64-apple-darwin/release/apw" \
  -output "$ROOT_DIR/rust/target/release/apw"
chmod 0755 "$ROOT_DIR/rust/target/release/apw"

"$ROOT_DIR/scripts/build-native-app.sh" --universal
"$ROOT_DIR/scripts/verify-universal-binaries.sh"
