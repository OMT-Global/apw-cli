#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FORMULA_PATH="${APW_HOMEBREW_FORMULA_PATH:-$ROOT_DIR/packaging/homebrew/apw.rb}"

usage() {
  echo "Usage: scripts/render-homebrew-formula.sh <version> <sha256>" >&2
}

if [ "$#" -ne 2 ]; then
  usage
  exit 2
fi

version="$1"
sha256="$2"

if [[ ! "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "Version must be semver without a leading v, for example 2.1.0." >&2
  exit 2
fi

if [[ ! "$sha256" =~ ^[0-9a-fA-F]{64}$ ]]; then
  echo "sha256 must be a 64-character hexadecimal digest." >&2
  exit 2
fi

if [ ! -f "$FORMULA_PATH" ]; then
  echo "Formula not found: $FORMULA_PATH" >&2
  exit 1
fi

export APW_FORMULA_VERSION="$version"
export APW_FORMULA_SHA256="$(printf '%s' "$sha256" | tr 'A-F' 'a-f')"

perl -0pi -e '
  s#(url\s+"https://github\.com/OMT-Global/apw-cli/archive/refs/tags/v)[^"]+(\.tar\.gz")#$1$ENV{APW_FORMULA_VERSION}$2#g;
  s#(^\s*version\s+")[^"]+(")#$1$ENV{APW_FORMULA_VERSION}$2#gm;
  s#(^\s*sha256\s+")[^"]+(")#$1$ENV{APW_FORMULA_SHA256}$2#gm;
' "$FORMULA_PATH"

if ! grep -q "refs/tags/v${version}.tar.gz" "$FORMULA_PATH"; then
  echo "Rendered formula is missing expected release tarball URL for v${version}." >&2
  exit 1
fi

if ! grep -Eq "^[[:space:]]*version[[:space:]]+\"${version}\"" "$FORMULA_PATH"; then
  echo "Rendered formula is missing expected version ${version}." >&2
  exit 1
fi

if ! grep -Eq "^[[:space:]]*sha256[[:space:]]+\"${APW_FORMULA_SHA256}\"" "$FORMULA_PATH"; then
  echo "Rendered formula is missing expected sha256 ${APW_FORMULA_SHA256}." >&2
  exit 1
fi

echo "Rendered $FORMULA_PATH for v${version}."
