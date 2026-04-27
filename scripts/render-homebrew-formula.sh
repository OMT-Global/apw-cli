#!/usr/bin/env bash
# Render the Homebrew formula for a published APW release.
#
# Usage:
#   scripts/render-homebrew-formula.sh <version> <sha256> [tarball-url]
#
# Reads packaging/homebrew/apw.rb as the template, substitutes the
# version, tarball URL, and tarball sha256, and writes the rendered
# formula to stdout. Designed to be called from release CI (issue #6)
# or by hand for a manual tap update.
#
# Example:
#   scripts/render-homebrew-formula.sh 2.0.1 \
#     "$(shasum -a 256 dist/apw-v2.0.1.tar.gz | awk '{print $1}')" \
#     "https://github.com/OMT-Global/apw/archive/refs/tags/v2.0.1.tar.gz" \
#     > /tmp/apw.rb

set -euo pipefail

if [[ $# -lt 2 || $# -gt 3 ]]; then
  echo "usage: $(basename "$0") <version> <sha256> [tarball-url]" >&2
  exit 64
fi

version="$1"
sha256="$2"
default_url="https://github.com/OMT-Global/apw/archive/refs/tags/v${version}.tar.gz"
url="${3:-$default_url}"

if [[ ! "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[A-Za-z0-9.-]+)?$ ]]; then
  echo "render-homebrew-formula: version must be SemVer, got '$version'" >&2
  exit 65
fi

if [[ ! "$sha256" =~ ^[0-9a-fA-F]{64}$ ]]; then
  echo "render-homebrew-formula: sha256 must be a 64-char hex digest, got '$sha256'" >&2
  exit 65
fi

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
template="$repo_root/packaging/homebrew/apw.rb"

if [[ ! -f "$template" ]]; then
  echo "render-homebrew-formula: missing template at $template" >&2
  exit 66
fi

# Use awk to do the substitution so we don't depend on GNU sed semantics.
awk -v ver="$version" -v sha="$sha256" -v url="$url" '
  /^[[:space:]]*version "/  { print "  version \"" ver "\""; next }
  /^[[:space:]]*url "/      { print "  url \"" url "\"";     next }
  /^[[:space:]]*sha256 "/   { print "  sha256 \"" sha "\"";  next }
  { print }
' "$template"
