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

require_tool cargo
require_tool swift

run_step "Rust legacy parity tests" \
  cargo test --manifest-path rust/Cargo.toml --test legacy_parity

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
