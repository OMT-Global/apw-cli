#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORKFLOW_DIR="${1:-$ROOT_DIR/.github/workflows}"
RETIRED_SELECTOR="runs-on: ['self-hosted', 'linux', 'shell-only', 'public']"
HOSTED_SELECTOR="runs-on: ubuntu-24.04"
PRIVATE_MAC_SELECTOR="runs-on: ['self-hosted', 'private', 'macOS', 'ARM64', 'xcode']"
SIGNING_SELECTOR="runs-on: ['self-hosted', 'private', 'macOS', 'ARM64', 'xcode', 'sparkle-release']"

fail() {
  echo "$1" >&2
  return 1
}

job_block() {
  local workflow="$1"
  local job_id="$2"
  awk -v heading="  ${job_id}:" '
    /^  [A-Za-z0-9_-]+:/ {
      if (inside && $0 != heading) {
        exit
      }
      inside = ($0 == heading)
    }
    inside { print }
  ' "$workflow"
}

require_literal() {
  local file="$1"
  local literal="$2"
  local message="$3"
  grep -Fq -- "$literal" "$file" || fail "$message"
}

require_job_runner() {
  local workflow="$1"
  local job_id="$2"
  local selector="$3"
  local block
  block="$(job_block "$workflow" "$job_id")"
  [[ -n "$block" ]] || fail "Missing job ${job_id} in ${workflow}."
  grep -Fq -- "$selector" <<<"$block" ||
    fail "Job ${job_id} in ${workflow} must retain ${selector}."
}

validate_policy() {
  local workflows="$1"
  local workflow job_id

  if grep -R -Fq --include='*.yml' --include='*.yaml' "$RETIRED_SELECTOR" "$workflows"; then
    fail "Retired public self-hosted Linux selector is forbidden."
  fi

  while IFS='|' read -r workflow job_id; do
    require_job_runner "$workflows/$workflow" "$job_id" "$HOSTED_SELECTOR"
  done <<'EOF'
extended-validation.yml|changes
extended-validation.yml|fast-checks
extended-validation.yml|validate-secrets
extended-validation.yml|extended-validation-gate
issue-hygiene.yml|report
pr-fast-ci.yml|changes
pr-fast-ci.yml|fast-checks
pr-fast-ci.yml|validate-secrets
pr-fast-ci.yml|ci-gate
release.yml|publish-homebrew-tap
EOF

  require_job_runner "$workflows/extended-validation.yml" "extended-checks" "$PRIVATE_MAC_SELECTOR"
  require_job_runner "$workflows/release.yml" "release" "$SIGNING_SELECTOR"

  require_literal "$workflows/issue-hygiene.yml" "actions/checkout@11d5960a326750d5838078e36cf38b85af677262 # v4" "Issue Hygiene checkout pin changed."
  require_literal "$workflows/issue-hygiene.yml" "actions/setup-node@49933ea5288caeca8642d1e84afbd3f7d6820020 # v4" "Issue Hygiene setup-node pin changed."
  require_literal "$workflows/issue-hygiene.yml" "actions/upload-artifact@ea165f8d65b6e75b540449e92b4886f43607fa02 # v4" "Issue Hygiene upload-artifact pin changed."
  require_literal "$workflows/extended-validation.yml" "name: Extended Validation Gate" "Extended Validation Gate name changed."
  require_literal "$workflows/extended-validation.yml" "- extended-checks" "Extended Validation Gate must retain the private extended-checks dependency."
  require_literal "$workflows/pr-fast-ci.yml" "name: CI Gate" "CI Gate name changed."
  require_literal "$workflows/pr-fast-ci.yml" "- native-app-swift-tests" "CI Gate must retain native-app-swift-tests."
  require_literal "$workflows/pr-fast-ci.yml" "- rust-native-e2e" "CI Gate must retain rust-native-e2e."
  require_literal "$workflows/release.yml" "uses: softprops/action-gh-release@v2" "Release publishing action changed."
}

validate_policy "$WORKFLOW_DIR"

if [[ "$WORKFLOW_DIR" == "$ROOT_DIR/.github/workflows" ]]; then
  TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/apw-runner-policy.XXXXXX")"
  cleanup() {
    [[ "$TMP_DIR" == "${TMPDIR:-/tmp}"/apw-runner-policy.* ]] && rm -rf -- "$TMP_DIR"
  }
  trap cleanup EXIT

  cp -R "$WORKFLOW_DIR" "$TMP_DIR/retired"
  sed -i "0,/runs-on: ubuntu-24\\.04/s//$RETIRED_SELECTOR/" "$TMP_DIR/retired/extended-validation.yml"
  if bash "$0" "$TMP_DIR/retired" >/dev/null 2>&1; then
    fail "Negative test accepted the retired public self-hosted selector."
  fi

  cp -R "$WORKFLOW_DIR" "$TMP_DIR/private"
  sed -i "0,/runs-on: \['self-hosted', 'private', 'macOS', 'ARM64', 'xcode'\]/s//$HOSTED_SELECTOR/" "$TMP_DIR/private/extended-validation.yml"
  if bash "$0" "$TMP_DIR/private" >/dev/null 2>&1; then
    fail "Negative test accepted migration of the private macOS selector."
  fi

  cp -R "$WORKFLOW_DIR" "$TMP_DIR/pin"
  sed -i 's/actions\/setup-node@49933ea5288caeca8642d1e84afbd3f7d6820020/actions\/setup-node@v4/' "$TMP_DIR/pin/issue-hygiene.yml"
  if bash "$0" "$TMP_DIR/pin" >/dev/null 2>&1; then
    fail "Negative test accepted an action pin change."
  fi
fi

echo "Public CI runner policy contract test passed."
