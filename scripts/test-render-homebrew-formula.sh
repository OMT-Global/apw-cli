#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tmp_dir="$(mktemp -d)"
cleanup() {
  rm -rf "$tmp_dir"
}
trap cleanup EXIT

formula_copy="$tmp_dir/apw.rb"
cp "$ROOT_DIR/packaging/homebrew/apw.rb" "$formula_copy"

version="9.8.7"
sha256="ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789"
normalized_sha256="$(printf '%s' "$sha256" | tr 'A-F' 'a-f')"

APW_HOMEBREW_FORMULA_PATH="$formula_copy" \
  "$ROOT_DIR/scripts/render-homebrew-formula.sh" "$version" "$sha256" >/dev/null

grep -q "version \"$version\"" "$formula_copy"
grep -q "refs/tags/v${version}.tar.gz" "$formula_copy"
grep -q "sha256 \"$normalized_sha256\"" "$formula_copy"

if APW_HOMEBREW_FORMULA_PATH="$formula_copy" \
  "$ROOT_DIR/scripts/render-homebrew-formula.sh" v9.8.7 "$normalized_sha256" >/dev/null 2>&1; then
  echo "render-homebrew-formula accepted a version with leading v." >&2
  exit 1
fi

if APW_HOMEBREW_FORMULA_PATH="$formula_copy" \
  "$ROOT_DIR/scripts/render-homebrew-formula.sh" "$version" "not-a-sha" >/dev/null 2>&1; then
  echo "render-homebrew-formula accepted an invalid sha256." >&2
  exit 1
fi

echo "Homebrew formula renderer test passed."
