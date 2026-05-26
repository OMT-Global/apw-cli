#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

echo "Running APW quality indicators..."

LCOV_PATH="${APW_LCOV_PATH:-}"

if [[ "${APW_RUN_COVERAGE:-0}" == "1" ]]; then
  if command -v cargo-llvm-cov >/dev/null 2>&1; then
    LCOV_PATH="${LCOV_PATH:-rust/target/apw-lcov.info}"
    cargo llvm-cov \
      --manifest-path rust/Cargo.toml \
      --lcov \
      --output-path "$LCOV_PATH"
  else
    echo "cargo-llvm-cov is required when APW_RUN_COVERAGE=1." >&2
    echo "Install it or unset APW_RUN_COVERAGE to run the source-only CRAP report." >&2
    exit 1
  fi
fi

if [[ "${APW_RUN_MUTATION:-0}" == "1" ]]; then
  if command -v cargo-mutants >/dev/null 2>&1; then
    cargo mutants \
      --manifest-path rust/Cargo.toml \
      --output rust/target/mutants \
      --timeout "${APW_MUTATION_TIMEOUT:-30}"
  else
    echo "cargo-mutants is required when APW_RUN_MUTATION=1." >&2
    echo "Install it or unset APW_RUN_MUTATION to run CRAP indicators only." >&2
    exit 1
  fi
fi

CRAP_ARGS=(rust/src --limit "${APW_CRAP_LIMIT:-20}" --threshold "${APW_CRAP_THRESHOLD:-30}")
if [[ -n "$LCOV_PATH" && -f "$LCOV_PATH" ]]; then
  CRAP_ARGS+=(--lcov "$LCOV_PATH")
else
  echo "No LCOV file supplied; CRAP report uses conservative 0% coverage." >&2
fi

python3 scripts/quality/crap_indicator.py "${CRAP_ARGS[@]}"

echo "APW quality indicators passed."
