#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

echo "Running APW fast checks..."

chmod +x ./.github/scripts/verify-version-sync.sh
./.github/scripts/verify-version-sync.sh \
  rust/Cargo.toml \
  rust/src/cli.rs \
  rust/src/types.rs \
  packaging/homebrew/apw.rb \
  README.md \
  docs/INSTALLATION.md \
  docs/MIGRATION_AND_PARITY.md

while IFS= read -r -d '' script; do
  bash -n "$script"
done < <(find .github/scripts scripts -type f -name '*.sh' -print0)

echo "APW fast checks passed."
