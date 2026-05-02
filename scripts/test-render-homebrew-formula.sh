#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tmp_dir="$(mktemp -d)"
cleanup() {
  rm -rf "$tmp_dir"
}
trap cleanup EXIT

formula_output="$tmp_dir/apw.rb"

version="9.8.7"
sha256="ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789"
normalized_sha256="$(printf '%s' "$sha256" | tr 'A-F' 'a-f')"

"$ROOT_DIR/scripts/render-homebrew-formula.sh" \
  "$version" \
  "$sha256" \
  "$ROOT_DIR/packaging/homebrew/apw.rb.template" \
  "$formula_output" >/dev/null

grep -q "version \"$version\"" "$formula_output"
grep -q "refs/tags/v${version}.tar.gz" "$formula_output"
grep -q "sha256 \"$normalized_sha256\"" "$formula_output"

if "$ROOT_DIR/scripts/render-homebrew-formula.sh" v9.8.7 "$normalized_sha256" "$ROOT_DIR/packaging/homebrew/apw.rb.template" "$formula_output" >/dev/null 2>&1; then
  echo "render-homebrew-formula accepted a version with leading v." >&2
  exit 1
fi

if "$ROOT_DIR/scripts/render-homebrew-formula.sh" "$version" "not-a-sha" "$ROOT_DIR/packaging/homebrew/apw.rb.template" "$formula_output" >/dev/null 2>&1; then
  echo "render-homebrew-formula accepted an invalid sha256." >&2
  exit 1
fi

echo "Homebrew formula renderer test passed."
