#!/usr/bin/env bash
set -euo pipefail

print_help() {
  cat <<'HELP'
Usage: ./scripts/build-universal-release.sh

Build universal arm64 + x86_64 release binaries:
  - rust/target/release/apw
  - native-app/dist/APW.app/Contents/MacOS/APW

Requires macOS, rustup, cargo, SwiftPM, and lipo.
HELP
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  print_help
  exit 0
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

export_openssl_prefix() {
  local env_prefix="$1"
  local prefix="$2"

  export "${env_prefix}_OPENSSL_DIR=$prefix"
  export "${env_prefix}_OPENSSL_INCLUDE_DIR=$prefix/include"
  export "${env_prefix}_OPENSSL_LIB_DIR=$prefix/lib"
}

configure_macos_openssl() {
  local arm_prefix="${AARCH64_APPLE_DARWIN_OPENSSL_DIR:-}"
  local x86_prefix="${X86_64_APPLE_DARWIN_OPENSSL_DIR:-}"
  local brew_prefix=""

  if [[ -z "$arm_prefix" && -d "/opt/homebrew/opt/openssl@3" ]]; then
    arm_prefix="/opt/homebrew/opt/openssl@3"
  fi
  if [[ -z "$x86_prefix" && -d "/usr/local/opt/openssl@3" ]]; then
    x86_prefix="/usr/local/opt/openssl@3"
  fi
  if command -v brew >/dev/null 2>&1; then
    brew_prefix="$(brew --prefix openssl@3 2>/dev/null || true)"
  fi
  if [[ -n "$brew_prefix" && -d "$brew_prefix" ]]; then
    if [[ -z "$arm_prefix" && "$(uname -m)" == "arm64" ]]; then
      arm_prefix="$brew_prefix"
    fi
    if [[ -z "$x86_prefix" && "$(uname -m)" == "x86_64" ]]; then
      x86_prefix="$brew_prefix"
    fi
  fi

  if [[ -z "$arm_prefix" || ! -d "$arm_prefix/include" || ! -d "$arm_prefix/lib" ]]; then
    echo "arm64 OpenSSL prefix not found. Install openssl@3 or export AARCH64_APPLE_DARWIN_OPENSSL_DIR." >&2
    exit 1
  fi
  if [[ -z "$x86_prefix" || ! -d "$x86_prefix/include" || ! -d "$x86_prefix/lib" ]]; then
    echo "x86_64 OpenSSL prefix not found. Install Intel openssl@3 or export X86_64_APPLE_DARWIN_OPENSSL_DIR." >&2
    exit 1
  fi

  export_openssl_prefix AARCH64_APPLE_DARWIN "$arm_prefix"
  export_openssl_prefix X86_64_APPLE_DARWIN "$x86_prefix"
  export PKG_CONFIG_ALLOW_CROSS="${PKG_CONFIG_ALLOW_CROSS:-1}"
}

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "Universal macOS release builds must run on macOS." >&2
  exit 1
fi

configure_macos_openssl

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
