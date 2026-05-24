#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

if ! grep -Fq 'openssl = { version = "0.10.68", features = ["vendored"] }' rust/Cargo.toml; then
  echo "rust/Cargo.toml must enable the openssl vendored feature." >&2
  exit 1
fi

if grep -Eq 'command -v brew|brew install|brew --prefix|brew list|command -v pkg-config|pkg-config --' .github/workflows/extended-validation.yml scripts/ci/run-extended-validation.sh; then
  echo "Extended validation must not depend on Homebrew/pkg-config OpenSSL discovery." >&2
  exit 1
fi

tmp_bin="$(mktemp -d)"
trap 'rm -rf "$tmp_bin"' EXIT
for tool in cc make perl; do
  printf '#!/bin/sh\nexit 0\n' > "$tmp_bin/$tool"
  chmod +x "$tmp_bin/$tool"
done

PATH="$tmp_bin:$PATH" APW_FORCE_OPENSSL_INPUT_CHECK=1 bash scripts/ci/run-extended-validation.sh --check-build-inputs >/dev/null

echo "Extended validation OpenSSL configuration is deterministic."
