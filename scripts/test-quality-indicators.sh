#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

bash -n scripts/ci/run-quality-indicators.sh
python3 scripts/quality/crap_indicator.py --self-test

EMPTY_PATH="$(mktemp -d "${TMPDIR:-/tmp}/apw-quality-empty-path.XXXXXX")"
trap 'rm -rf "$EMPTY_PATH"' EXIT

if PATH="$EMPTY_PATH" APW_RUN_MUTATION=1 /bin/bash scripts/ci/run-quality-indicators.sh 2>"$EMPTY_PATH/mutation.err"; then
  echo "Expected APW_RUN_MUTATION=1 to fail when cargo-mutants is missing." >&2
  exit 1
fi
grep -q "cargo-mutants is required when APW_RUN_MUTATION=1" "$EMPTY_PATH/mutation.err"

if PATH="$EMPTY_PATH" APW_RUN_COVERAGE=1 /bin/bash scripts/ci/run-quality-indicators.sh 2>"$EMPTY_PATH/coverage.err"; then
  echo "Expected APW_RUN_COVERAGE=1 to fail when cargo-llvm-cov is missing." >&2
  exit 1
fi
grep -q "cargo-llvm-cov is required when APW_RUN_COVERAGE=1" "$EMPTY_PATH/coverage.err"

echo "Quality indicator tests passed."
