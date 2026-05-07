#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

echo "Running APW fast checks..."

if find . -path './.git' -prune -o -name '*.md' -print0 | xargs -0 grep -In '/Users/'; then
  echo "Found machine-local absolute paths in markdown docs." >&2
  exit 1
fi

if [ ! -x scripts/bump-version.sh ]; then
  echo "scripts/bump-version.sh must be executable." >&2
  exit 1
fi

if [ ! -x scripts/render-homebrew-formula.sh ]; then
  echo "scripts/render-homebrew-formula.sh must be executable." >&2
  exit 1
fi

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

./scripts/test-render-homebrew-formula.sh

echo "APW fast checks passed."
