#!/usr/bin/env bash
set -euo pipefail

extract_version_from_cargo() {
  awk -F ' = ' '/^version = / {gsub(/"/, "", $2); print $2; exit}' "$1"
}

extract_version_from_rust_source() {
  if grep -q 'env!("CARGO_PKG_VERSION")' "$1"; then
    printf '%s\n' "$cargo_version"
    return
  fi

  sed -nE \
    -e 's/^[[:space:]]*#\[command\(version = "([0-9]+\.[0-9]+\.[0-9]+)"\)\].*/\1/p' \
    -e 's/^[[:space:]]*(const|pub const) [A-Z_]+: &str = "([0-9]+\.[0-9]+\.[0-9]+)".*/\2/p' \
    "$1" | head -n 1
}

extract_version_from_formula_version() {
  sed -nE 's/^[[:space:]]*version[[:space:]]+"([0-9]+\.[0-9]+\.[0-9]+)".*/\1/p' "$1" | head -n 1
}

extract_version_from_formula_url() {
  sed -nE 's#^.*refs/tags/v([0-9]+\.[0-9]+\.[0-9]+)\.tar\.gz.*#\1#p' "$1" | head -n 1
}

extract_version_from_docs() {
  sed -nE 's/.*Release reference version:[^0-9]*`?v?([0-9]+\.[0-9]+\.[0-9]+)`?.*/\1/p' "$1" | head -n 1
}

cargo_version="$(extract_version_from_cargo "$1")"
cli_version="$(extract_version_from_rust_source "$2")"
types_version="$(extract_version_from_rust_source "$3")"
formula_version="$(extract_version_from_formula_version "$4")"
formula_url_version="$(extract_version_from_formula_url "$4")"
docs_version1="$(extract_version_from_docs "$5")"
docs_version2="$(extract_version_from_docs "$6")"
docs_version3="$(extract_version_from_docs "$7")"

if [ "$cargo_version" != "$cli_version" ] || [ "$cargo_version" != "$types_version" ] || [ "$cargo_version" != "$formula_version" ] || [ "$cargo_version" != "$formula_url_version" ] || [ "$cargo_version" != "$docs_version1" ] || [ "$cargo_version" != "$docs_version2" ] || [ "$cargo_version" != "$docs_version3" ]; then
  cat <<EOF >&2
Version sync check failed:
  Cargo.toml:    $cargo_version
  cli.rs:        $cli_version
  types.rs:      $types_version
  homebrew:      $formula_version
  homebrew_url:  $formula_url_version
  docs/README:   $docs_version1
  docs/INSTALL:  $docs_version2
  docs/MIGRATION:$docs_version3
EOF
  exit 1
fi

echo "Version sync check passed: $cargo_version"
