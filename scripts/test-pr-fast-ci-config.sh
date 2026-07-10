#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORKFLOW="$ROOT_DIR/.github/workflows/pr-fast-ci.yml"
FAST_CHECKS="$ROOT_DIR/scripts/ci/run-fast-checks.sh"

fail() {
  echo "$1" >&2
  exit 1
}

job_block() {
  local job_id="$1"
  awk -v heading="  ${job_id}:" '
    /^  [A-Za-z0-9_-]+:/ {
      if (inside && $0 != heading) {
        exit
      }
      inside = ($0 == heading)
    }
    inside { print }
  ' "$WORKFLOW"
}

require_in_block() {
  local block="$1"
  local literal="$2"
  local message="$3"
  grep -Fq -- "$literal" <<<"$block" || fail "$message"
}

[[ -f "$WORKFLOW" ]] || fail "Missing PR Fast CI workflow: $WORKFLOW"

changes_block="$(job_block changes)"
e2e_block="$(job_block rust-native-e2e)"
gate_block="$(job_block ci-gate)"

[[ -n "$changes_block" ]] || fail "PR Fast CI must retain the changes job."
[[ -n "$e2e_block" ]] || fail "PR Fast CI must define the rust-native-e2e job."
[[ -n "$gate_block" ]] || fail "PR Fast CI must retain the ci-gate job."

require_in_block "$changes_block" \
  'rust_native_e2e: ${{ steps.filter.outputs.rust_native_e2e }}' \
  "The changes job must expose the rust_native_e2e path-filter output."

for path in \
  rust/src/bundle.rs \
  rust/src/main.rs \
  rust/src/native_app.rs \
  rust/src/state_root.rs \
  rust/src/utils.rs \
  rust/tests/native_app_e2e.rs; do
  require_in_block "$changes_block" "- '$path'" \
    "The rust_native_e2e filter must include $path."
done

require_in_block "$e2e_block" "name: Rust Native App E2E" \
  "The Rust native E2E job name must remain stable."
require_in_block "$e2e_block" \
  "runs-on: macos-latest" \
  "Rust native E2E must run on an isolated hosted macOS runner."
require_in_block "$e2e_block" \
  "needs.changes.outputs.rust_native_e2e == 'true'" \
  "Rust native E2E must stay conditional on its narrow path filter."
require_in_block "$e2e_block" \
  "cargo test --manifest-path rust/Cargo.toml --test native_app_e2e" \
  "Rust native E2E must execute the native_app_e2e test target."

require_in_block "$gate_block" "name: CI Gate" \
  "The required CI Gate check name must remain stable."
require_in_block "$gate_block" "- rust-native-e2e" \
  "CI Gate must depend on rust-native-e2e."
require_in_block "$gate_block" \
  'rust-native-e2e=${{ needs.rust-native-e2e.result }}' \
  "CI Gate must evaluate the rust-native-e2e result."
require_in_block "$gate_block" \
  '[[ "$status" == "success" || "$status" == "skipped" ]]' \
  "CI Gate must continue accepting skipped conditional jobs."

grep -Fq 'bash ./scripts/test-pr-fast-ci-config.sh' "$FAST_CHECKS" ||
  fail "Fast checks must run the PR Fast CI contract test."

echo "PR Fast CI contract test passed."
