#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

workflow=".github/workflows/claude.yml"
trusted_associations='MEMBER.*OWNER.*COLLABORATOR'

for event in issue_comment pull_request_review_comment; do
  if ! grep -Eq "github\.event_name == '${event}'.*author_association" "$workflow"; then
    echo "Claude workflow must gate ${event} by author association." >&2
    exit 1
  fi
done

if ! grep -Eq "github\.event_name == 'pull_request_review'.*review\.author_association" "$workflow"; then
  echo "Claude workflow must gate pull_request_review by author association." >&2
  exit 1
fi

if ! grep -Eq "fromJSON\('\[\"${trusted_associations}\"\]'\)" "$workflow"; then
  echo "Claude workflow must allow only trusted author associations." >&2
  exit 1
fi

echo "Claude workflow authorization contract passed."
