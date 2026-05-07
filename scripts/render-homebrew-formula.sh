#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

usage() {
  cat <<'USAGE'
Usage: scripts/render-homebrew-formula.sh <version> <sha256> [template] [output]

Render the Homebrew formula by replacing {{VERSION}} and {{SHA256}}
tokens. Defaults:
  template: packaging/homebrew/apw.rb.template
  output:   packaging/homebrew/apw.rb
USAGE
}

if [ "${1:-}" = "-h" ] || [ "${1:-}" = "--help" ]; then
  usage
  exit 0
fi

if [ "$#" -lt 2 ] || [ "$#" -gt 4 ]; then
  usage >&2
  exit 64
fi

version="$1"
sha256="$2"
template="${3:-$ROOT_DIR/packaging/homebrew/apw.rb.template}"
output="${4:-$ROOT_DIR/packaging/homebrew/apw.rb}"

if [[ ! "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "Version must be semver without a leading v, for example 2.1.0." >&2
  exit 65
fi

if [[ ! "$sha256" =~ ^[0-9a-fA-F]{64}$ ]]; then
  echo "sha256 must be a 64-character hexadecimal digest." >&2
  exit 65
fi

if [ ! -f "$template" ]; then
  echo "Template not found: $template" >&2
  exit 66
fi

normalized_sha256="$(printf '%s' "$sha256" | tr 'A-F' 'a-f')"
mkdir -p "$(dirname "$output")"
VERSION="$version" SHA256="$normalized_sha256" \
  perl -0pe 's/\{\{VERSION\}\}/$ENV{VERSION}/g; s/\{\{SHA256\}\}/$ENV{SHA256}/g' \
  "$template" > "$output"

if grep -qE '\{\{VERSION\}\}|\{\{SHA256\}\}' "$output"; then
  echo "Unrendered template tokens remain in $output" >&2
  exit 67
fi

if ! grep -q "refs/tags/v${version}.tar.gz" "$output"; then
  echo "Rendered formula is missing expected release tarball URL for v${version}." >&2
  exit 67
fi

if ! grep -Eq "^[[:space:]]*version[[:space:]]+\"${version}\"" "$output"; then
  echo "Rendered formula is missing expected version ${version}." >&2
  exit 67
fi

if ! grep -Eq "^[[:space:]]*sha256[[:space:]]+\"${normalized_sha256}\"" "$output"; then
  echo "Rendered formula is missing expected sha256 ${normalized_sha256}." >&2
  exit 67
fi

printf 'Rendered %s for v%s.\n' "$output" "$version"
