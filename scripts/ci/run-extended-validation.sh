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

# Issue #40: openssl-sys discovery on macOS requires pkg-config and a
# locatable OpenSSL prefix. The workflow caller exports these via
# $GITHUB_ENV; we mirror the discovery here so direct shell invocations on a
# clean macOS host (or runner steps that bypass the workflow) still succeed
# with a clear error if the toolchain is unavailable.
ensure_openssl_on_macos() {
  [[ "${OSTYPE:-}" == darwin* ]] || return 0
  if [[ -n "${OPENSSL_DIR:-}" ]] && command -v pkg-config >/dev/null 2>&1; then
    return 0
  fi
  if ! command -v brew >/dev/null 2>&1; then
    echo "Homebrew is required on macOS to locate OpenSSL for openssl-sys." >&2
    echo "Install Homebrew or export OPENSSL_DIR/PKG_CONFIG_PATH manually." >&2
    exit 1
  fi
  if ! command -v pkg-config >/dev/null 2>&1; then
    brew list pkg-config >/dev/null 2>&1 || brew install pkg-config
  fi
  local prefix
  prefix="$(brew --prefix openssl@3 2>/dev/null || true)"
  if [[ -z "$prefix" || ! -d "$prefix" ]]; then
    brew install openssl@3
    prefix="$(brew --prefix openssl@3)"
  fi
  export OPENSSL_DIR="$prefix"
  export OPENSSL_INCLUDE_DIR="$prefix/include"
  export OPENSSL_LIB_DIR="$prefix/lib"
  export PKG_CONFIG_PATH="$prefix/lib/pkgconfig${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}"
}

ensure_openssl_on_macos

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
