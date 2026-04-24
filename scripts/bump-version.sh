#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

usage() {
  echo "Usage: scripts/bump-version.sh <semver>" >&2
}

if [ "$#" -ne 1 ]; then
  usage
  exit 2
fi

new_version="$1"
if [[ ! "$new_version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "Version must be semver without a leading v, for example 2.1.0." >&2
  exit 2
fi
export NEW_VERSION="$new_version"

files=(
  rust/Cargo.toml
  rust/src/cli.rs
  rust/src/types.rs
  packaging/homebrew/apw.rb
  README.md
  docs/INSTALLATION.md
  docs/MIGRATION_AND_PARITY.md
)

backup_dir="$(mktemp -d)"
restore_backups() {
  for file in "${files[@]}"; do
    if [ -f "$backup_dir/$file" ]; then
      cp "$backup_dir/$file" "$file"
    fi
  done
  rm -rf "$backup_dir"
}
trap restore_backups ERR
trap 'rm -rf "$backup_dir"' EXIT

for file in "${files[@]}"; do
  mkdir -p "$backup_dir/$(dirname "$file")"
  cp "$file" "$backup_dir/$file"
done

perl -0pi -e 's/^version = "\d+\.\d+\.\d+"/version = "$ENV{NEW_VERSION}"/m' rust/Cargo.toml
perl -0pi -e 's/(refs\/tags\/v)\d+\.\d+\.\d+(\.tar\.gz)/$1$ENV{NEW_VERSION}$2/g; s/(version\s+")\d+\.\d+\.\d+(")/$1$ENV{NEW_VERSION}$2/g' packaging/homebrew/apw.rb
perl -0pi -e 's/v\d+\.\d+\.\d+/v$ENV{NEW_VERSION}/g' README.md
perl -0pi -e 's/v\d+\.\d+\.\d+/v$ENV{NEW_VERSION}/g' docs/INSTALLATION.md
perl -0pi -e 's/v\d+\.\d+\.\d+/v$ENV{NEW_VERSION}/g' docs/MIGRATION_AND_PARITY.md

./.github/scripts/verify-version-sync.sh \
  rust/Cargo.toml \
  rust/src/cli.rs \
  rust/src/types.rs \
  packaging/homebrew/apw.rb \
  README.md \
  docs/INSTALLATION.md \
  docs/MIGRATION_AND_PARITY.md

for file in "${files[@]}"; do
  if cmp -s "$backup_dir/$file" "$file"; then
    echo "unchanged $file"
  else
    echo "updated $file"
    diff -u "$backup_dir/$file" "$file" | sed -nE '/^[+-][^+-]/p' || true
  fi
done
