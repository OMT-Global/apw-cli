#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

if grep -Eq 'command -v brew|brew install|brew --prefix|brew list|command -v pkg-config|pkg-config --|AARCH64_APPLE_DARWIN_OPENSSL_DIR|X86_64_APPLE_DARWIN_OPENSSL_DIR' scripts/build-universal-release.sh; then
  echo "Universal release build must not depend on Homebrew/pkg-config OpenSSL discovery." >&2
  exit 1
fi

tmp_bin="$(mktemp -d)"
trap 'rm -rf "$tmp_bin"' EXIT

for tool in cc make perl uname; do
  cat >"$tmp_bin/$tool" <<'EOF'
#!/bin/sh
if [ "$(basename "$0")" = "uname" ]; then
  if [ "${1:-}" = "-s" ]; then
    echo Darwin
  elif [ "${1:-}" = "-m" ]; then
    echo arm64
  else
    echo Darwin
  fi
  exit 0
fi
exit 0
EOF
  chmod +x "$tmp_bin/$tool"
done

PATH="$tmp_bin:$PATH" APW_FORCE_OPENSSL_INPUT_CHECK=1 \
  bash scripts/build-universal-release.sh --check-build-inputs >/dev/null

echo "Universal release configuration is deterministic."
